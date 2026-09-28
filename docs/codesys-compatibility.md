# CODESYS Compatibility Plan

Status: proposal. What is implemented is recorded in "Decided behaviours" below.

## Goal

Compile real-world PLC source. In practice that means CODESYS-flavoured ST, because
CODESYS is not one vendor's product — it is an OEM kernel rebranded by roughly 400
device manufacturers (WAGO, Festo, Lenze, Eaton, ifm, Bosch Rexroth, Schneider
SoMachine, ABB, Delta, …). Beckhoff TwinCAT rides broadly along with it. Supporting one
dialect reaches most of the ST anyone actually writes.

Siemens SCL and Rockwell ST are explicitly **not** dialects of this — see Non-Goals.

## The core design decision

**Parse the union. Diagnose the difference.**

One grammar accepts IEC 61131-3 *and* CODESYS extensions, always. Each extension
production carries a feature tag. Strict mode is a diagnostic pass *after* parsing, not
a second grammar.

```
error: PROPERTY is a CODESYS extension, not IEC 61131-3
  --> pump.st:14:5
   |
14 |     PROPERTY Level : REAL
   |     ^^^^^^^^
help: allowed by default; --std=iec61131-3 rejects it
```

This is what clang does with GNU extensions. The payoff:

- One parser, one test matrix. No dual-grammar maintenance.
- Strict mode becomes a **product feature** — "will this run on a non-CODESYS PLC?" is a
  question integrators genuinely have, and nothing else answers it well.
- Cheap now (one tag per production), genuinely painful to retrofit after forty
  productions exist and each has to be audited for standards provenance.

CODESYS is *not* a strict superset, so the flag cannot simply be deleted:

- IEC's `CONFIGURATION` / `RESOURCE` / `VAR_ACCESS` live in source; CODESYS keeps that in
  its device tree and largely rejects the source forms. The superset relation breaks in
  that direction.
- Overload resolution and implicit conversion rules do not match the standard's tables.
- Some constructs share syntax and differ in meaning (see Semantic Divergences).

### Two independent axes

Do not collapse these into one `--flavor`:

| Flag | Controls | Values |
|---|---|---|
| `--std` | what the front-end accepts / diagnoses | `codesys` (default), `iec61131-3` |
| `--profile` | what the back-end emits and links against | `freestanding`, `hosted`, `jit` |
| `--stdlib` | where standard FBs come from | `bundled-st`, `none`, `vendor` |

They are genuinely orthogonal: CODESYS syntax on bare-metal thumbv7em is a real
combination, as is strict IEC on a hosted sim. Welding them together forces a redesign
the first time a vendor wants one without the other.

Granular overrides (`-F fb-init`, `-F no-properties`) let a project adopt one extension
without buying the whole dialect.

## Blockers — these land first

Two confirmed defects make CODESYS work premature. Both were verified by execution, not
by reading.

### B1. FB member initializers are silently dropped (critical)

`compile_function_block` (`crates/plcc-codegen/src/compiler.rs:2943`) never reads
`decl.initializer` and never emits an `<fb>_init`. The program init path explicitly
skips FB instance fields:

```rust
// compiler.rs:2787
// Skip FB instance fields in init — they are zeroed which is fine
// (FB internal vars with initializers would need their own _init, but
// zero-init is correct default for IEC FBs)
```

The parenthetical is the bug describing itself. Zero-init is correct for *undeclared*
initial values; IEC requires declared ones to be applied. Measured: `VAR`, `VAR_INPUT`
and `VAR_OUTPUT` members all read back `0` regardless of source. `compile_class`
(`:3029`) has the same hole.

Impact is not theoretical — it breaks the repo's own demos. `pid_simple.st` has
`dt : REAL := 0.01`, which becomes `0.0`, so `(err - prev_err) / dt` yields infinity and
then NaN on the first scan; the output clamp cannot recover it because NaN comparisons
are false. `motor_control.st` has `overload_limit : REAL := 15.0` → `0.0`, latching a
spurious fault.

**`FB_Init` is meaningless until this works.** Two broken initialization paths is worse
than one.

### B2. Standard function blocks are unreachable (critical)

`plcc-runtime` implements and unit-tests all 11 standard FBs (TON, TOF, TP, CTU, CTD,
CTUD, R_TRIG, F_TRIG, SR, RS, RTC) in Rust. Nothing depends on it — not `plcc-codegen`,
not `plcc-cli`. The codegen declares exactly two families of external symbols: libm trig
(`compiler.rs:689`) and `plcc_print` (`compiler.rs:1207`). Nothing timer-related.

The failure mode is silent. A program declaring `t : TON;` compiles, links, and produces
`main_scan` as a **single `ret` instruction** — the entire body discarded, no undefined
symbols, no diagnostic.

**Resolution: `--stdlib bundled-st`.** Ship the standard FBs as ST source compiled
alongside the user program. One FB implementation model, LLVM sees through all of it, no
Rust ABI bridge on bare metal. `plcc-runtime` then narrows to what CLAUDE.md already says
it should be — the *interface* (time, I/O hooks, memory conventions), not the
implementation. It also makes vendor flavours fall out for free: swap the prelude.

Until then, an unresolved FB instance type must be a hard error, never silent deletion.

## Current state

Already present, reusable:

| | Lexer | Parser | Codegen |
|---|---|---|---|
| `CLASS` | ✓ | ✓ | `compile_class` :3029 |
| `METHOD` | ✓ | ✓ | `compile_method` :3095, `compile_method_call` :3482 |
| `INTERFACE` | ✓ | ✓ | **none** (0 refs to `InterfaceDecl`) |
| `EXTENDS` / `IMPLEMENTS` | ✓ | ✓ | partial |
| `ABSTRACT` / `FINAL` | ✓ | — | — |
| `PUBLIC` / `PRIVATE` | ✓ | — | — |
| `POINTER TO` | ✓ | ✓ `TypeSpecKind::Pointer` | `IecType::Pointer` exists |
| `REFERENCE TO` | ✓ | ✓ | — |

Absent entirely: `PROPERTY`, `THIS`, `SUPER`, `VAR_INST`.

The program-level `_init` machinery (`compile_program`, `:2776`–`:2806`) already does
exactly what FBs need. B1 is largely a matter of applying existing logic in a second
place and making it recursive, not inventing a mechanism.

## Phasing

### Phase 0 — unblock (prerequisite)

1. B1: emit `<fb>_init` / `<cls>_init`; recurse into nested FB fields from the parent's
   init; handle `VAR_GLOBAL` FB instances.
2. B2: decide `--stdlib bundled-st`; make unresolved FB types a hard error immediately,
   even before the prelude exists.
3. Regression tests with **non-zero** initializers. Every FB member initializer in
   `fb_execution.rs` is currently `:= 0`, which is indistinguishable from zeroing — the
   suite cannot catch B1 and did not.

### Phase 1 — lexical extensions (cheap, high value)

Both remaining OSCAT failures are one-line lexer fixes, each verified by execution:

- `FUNCTIONBLOCK` (no underscore) as an alias on the `FunctionBlock` token,
  `token.rs:38`. CODESYS 2.3 spelling; the same files still close with the standard
  `END_FUNCTION_BLOCK`. Clears 7 of 8 OSCAT failures — measured 8 → 1 on a patched crate
  across all 559 files.
- Widen `TodLiteral`, `token.rs:345`: make the seconds field optional (`TOD#12:00`) and
  add the long-form prefixes. Clears the 8th.

The `TodLiteral` fix also closes a genuine **conformance** gap, not just an extension:
plcc currently rejects the fully standard `TIME_OF_DAY#12:00:00` and `LTOD#`, because
only `TOD#` is in the regex. The sibling regexes have the same hole — `TimeLiteral`
accepts only `T#` (not `TIME#`/`LTIME#`), `DateLiteral` only `D#`, `DtLiteral` only
`DT#`. Fix them together.

### Phase 2 — FB lifecycle

`FB_Init` first, and note it is a *parser* change before it is a codegen one:
declaration-site argument lists (`inst : MyFB(depth := 5);`) do not exist in the grammar.

```
METHOD FB_Init : BOOL
VAR_INPUT
    bInitRetains : BOOL;   (* cold start vs retained state preserved *)
    bInCopyCode  : BOOL;   (* online change, not a genuine startup *)
END_VAR
```

Those two implicit parameters carry the real semantics and are easy to overlook. You
branch on `bInitRetains` to avoid clobbering retained state, and on `bInCopyCode` to know
you must *not* re-open that socket.

Defer `FB_Exit` and `FB_Reinit`. Both only earn their weight alongside `__NEW`/`__DELETE`
or online change, and `FB_Reinit` is unimplementable without a state-layout manifest.

### Phase 3 — OOP surface

`PROPERTY` with GET/SET, `THIS^`, `SUPER^`, access specifiers wired through to
name resolution, `VAR_INST`. Interface codegen (currently absent) belongs here.

### Phase 4 — memory and lifetime

`REFERENCE TO` semantics, `__NEW` / `__DELETE`, `__QUERYINTERFACE` / `__QUERYPOINTER` /
`__ISVALIDREF`, `PERSISTENT` as distinct from `RETAIN`, and the attribute pragmas
(`{attribute 'call_after_init'}`, `{attribute 'no_copy'}`).

`{attribute 'call_after_init'}` exists because `FB_Init` runs before declaration-site
initializers are applied — it is the hook for setup that needs to see the instance's own
initial values.

## Semantic divergences

These share syntax with the standard and differ in meaning, so no amount of parser
permissiveness resolves them. Each needs an explicit decision, and the decision needs to
be written down:

- `MOD` on negative operands
- integer overflow behaviour
- string index base
- division by zero
- overload resolution and implicit conversion tables
- direct-address units (`%IW1`) and task defaults — decided: CODESYS size-indexed
  addressing and a T#20ms `MainTask`; the full table is in
  [process-image.md](process-image.md#implementation-defined-choices)

**Default to CODESYS behaviour** — that is what real code expects — and maintain a table
in this document of each divergence, which way we went, and why. Strict mode should warn
where the two differ, since that is exactly the portability question strict mode exists
to answer.

## Decided behaviours

Each row is a place where behaviour was a choice. The rule: do what CODESYS (and
TwinCAT, which is CODESYS-based; Siemens SCL where relevant) does, provided it agrees
with IEC 61131-3 3rd edition. Every row has a regression test.

| Behaviour | Decision | Source | Test |
|---|---|---|---|
| Output binding `fb(Q => x)` | `x` receives the output **after** the call, with ordinary assignment conversion. Works on FB, FUNCTION and METHOD calls; targets may be any assignable location (`arr[i]`, `s.f`). FUNCTION/METHOD VAR_OUTPUTs are passed as a pointer to a caller temporary and start each call at their initial value. | IEC 61131-3 §6.6.1.4 (`=>` connects an output to a variable); CODESYS help, "Function" object: `fun(in1 := 1, in2 := 2, out1 => loc1, out2 => loc2);`. | `crates/plcc-codegen/tests/output_bindings.rs` |
| `NOT Q => x` | Accepted; `x := NOT Q`. | IEC 61131-3 3rd ed. Annex A, `param_assign ::= ['NOT'] variable_name '=>' variable`. | same |
| `=>` on a VAR_INPUT / VAR_IN_OUT / local, `:=` on a VAR_OUTPUT | Compile error. | CODESYS rejects assigning an output in a call; IEC defines `=>` only for outputs. | same |
| `F(a, o => x)` | Compile error: a call names all its arguments or none. A positional call (`F(a)`) leaves outputs unconnected. | CODESYS help, "Function" object: "You cannot mix explicit and implicit parameter assignments in function calls in CODESYS 3" ([link](https://content.helpme-codesys.com/en/CODESYS%20Development%20System/_cds_obj_function.html)). | same |
| Positional FB call `fb(1, 2)` | Binds the VAR_INPUT and VAR_IN_OUT parameters in declaration order (outputs are never bound positionally), exactly like a FUNCTION call. Fewer arguments than inputs is allowed — the rest keep their values, as in a formal call that omits them. More than there are inputs, or a mix of named and positional, is a compile error. | IEC 61131-3 3rd ed. Annex A: `param_assign ::= [variable_name ':='] expression \| ...` — the name is optional in any call. CODESYS documents only formal FB calls; it rejects the mix (same rule as the row above). Previously the unnamed arguments were silently skipped. | `crates/plcc-codegen/tests/fb_positional_args.rs` |
| REAL/LREAL → integer (`REAL_TO_INT`, `LREAL_TO_UDINT`, … every integer and bit-string target, and implicit REAL → integer stores and arguments) | Round to nearest, **halves away from zero**: 2.5 → 3, -1.5 → -2, -2.5 → -3. `TRUNC` still truncates. One helper (`float_to_int`) lowers all of them. | CODESYS help, REAL_TO_<type>: "For 1 to 4 after the decimal point, the number is rounded down. For 5 to 9, the number is rounded up", example `REAL_TO_INT(-1.5)` = -2 ([link](https://content.helpme-codesys.com/en/CODESYS%20Development%20System/_cds_operator_real_to.html)). **Divergence:** IEC 61131-3 Table 22 requires round-to-nearest by reference to IEC 60559, which breaks ties to even (2.5 → 2; Siemens S7 does this too). We follow CODESYS/TwinCAT because that is what the code we compile was tested on; `llvm.roundeven` is the one-line switch for a strict mode. | `crates/plcc-codegen/tests/float_to_int.rs` |
| REAL/LREAL → integer out of range, NaN | **Saturate** to the target's range; NaN → 0 (`llvm.fptosi.sat` / `llvm.fptoui.sat`). `REAL_TO_INT(1.0E6)` = 32767, `REAL_TO_UDINT(-5.0)` = 0. Applies to every path through `float_to_int`, including TRUNC and REAL_TO_TIME. | **Deliberate choice, not CODESYS-derived.** CODESYS: "If the rounded value is outside of the integer value range, then an undefined, target system-dependent value is returned. An exception error is also possible then." (same page as above). IEC 61131-3 leaves it implementation-dependent. Saturation is defined on every LLVM target; the previous `fptosi` was poison, i.e. undefined behaviour under optimization. | `float_to_int.rs` (incl. an `-O3` JIT test) |
| TIME / date ⇄ number units | TIME and TIME_OF_DAY convert to and from numbers in **milliseconds** (TOD: since midnight); DATE and DATE_AND_TIME in **seconds since 1970-01-01**; LTIME, LTOD, LDATE, LDT in **nanoseconds**. Storage is i64 ns for all of them. `DT_TO_DATE` / `DT_TO_TOD` split at midnight (also for dates before 1970). Integer results truncate toward zero. `TIME()` is the time since start, whole milliseconds. | CODESYS: DATE/DT are "seconds since 1970-01-01" as a DWORD and TOD "milliseconds since 00:00" (help, "Data type: DATE_AND_TIME", "TIME_OF_DAY"); LTIME has ns resolution. IEC 61131-3 leaves the numeric representation implementation-dependent. OSCAT depends on exactly these units (`DT_TO_DWORD`, `TOD_TO_DWORD`, `DWORD_TO_DATE`). | `crates/plcc-codegen/tests/datetime_conversions.rs` |
| `X_TO_Y(v)` where `v` is not an X | `v` is first implicitly converted to X (so `DINT_TO_INT(r)` of a REAL rounds r to DINT), then X to Y. | IEC: a typed conversion function's input is typed X; CODESYS converts implicitly with a warning, as for any lossy conversion (row below). | same |
| Enumerators | `Idle`, `Mode#Idle` (IEC) and `Mode.Idle` (CODESYS) all work. A bare name must be unambiguous (or mean the same value in every enumeration that has it), otherwise it is an error asking for qualification; a variable of the same name wins over a bare enumerator. Unvalued enumerators are previous + 1, the first 0. The base type is the declared one (`TYPE C : DINT (..)`), INT by default. `{attribute 'qualified_only'}` is not enforced. | IEC 61131-3 §6.4.4.3 (typed enumerated values, `Type#Value`); CODESYS help "Enumerations": INT base by default, `E.Value` access, `qualified_only` attribute. | `crates/plcc-codegen/tests/enums_and_case.rs` |
| CASE labels | Any constant expression of the selector's type: literals (also negative and typed), enumerators, named constants, ranges of any size. Branches are tested in order; the first match wins. The selector must be an integer or enumeration. | IEC 61131-3 Annex A `case_list_elem ::= subrange \| constant_expr`; CODESYS accepts constants and enumerators. | same |
| Integer division / MOD by zero, `MIN / -1` | `x / 0` = 0 and `x MOD 0` = 0; `MIN / -1` = MIN (wraps, like all integer overflow), `MIN MOD -1` = 0. `MOD` takes the dividend's sign (`-7 MOD 3` = -1). REAL division by zero is IEEE (±inf/NaN). | **Deliberate choice.** CODESYS raises a target-dependent runtime exception (the application stops) unless the project adds the `CheckDivInt`/`CheckDivDInt` implicit-check POUs, which typically substitute a divisor of 1. IEC 61131-3 calls it an error with implementation-dependent handling. The previous lowering (`sdiv`) was UB: SIGFPE killed the process on x86, 0 on ARM, anything under optimization. A defined value is the same on every target and cannot take down the runtime. | `crates/plcc-codegen/tests/int_ub_edges.rs` |
| `SHL` / `SHR` by N ≥ width of IN (or negative N) | 0 — every bit has been shifted out. SHR is logical (zero-fill) for every IN type. ROL/ROR counts are modulo the width. | IEC 61131-3 Table 26: SHL/SHR shift left/right N bits, zero-filled. CODESYS leaves N ≥ width unspecified; LLVM makes it poison, and x86 masked the count (`SHL(dint, 32)` returned its input). | same |
| Bit access `x.3`, `x.%X3` | Reads bit n (0 = least significant) of any integer or bit-string variable as a BOOL; `x.3 := b` is a read-modify-write of x. Works on struct fields, array elements and output bindings (`f(q => w.4)`). A bit number ≥ the width is a compile error. | CODESYS help, "Bit access in variables" (`<variable>.<bit number>`); IEC 61131-3 3rd ed. Table 16 partial access `%X`. | `crates/plcc-codegen/tests/bit_access.rs` |
| STRING functions and comparisons | `MID(STR, LEN, POS)`, `LEFT(STR, SIZE)`, `RIGHT(STR, SIZE)`, `INSERT(STR1, STR2, POS)` (after position POS), `DELETE(STR, LEN, POS)`, `REPLACE(STR1, STR2, LEN, POS)`, `FIND(STR1, STR2)` (1-based, 0 if absent), n-ary `CONCAT`. Positions are 1-based; out-of-range positions and lengths are clamped (never read or write outside a buffer). Results are computed in a temporary and truncated to the destination. STRING comparison is byte-wise, unsigned, lexicographic (a prefix sorts first). | CODESYS help, string functions (argument order `MID(STR, LEN, POS)`; examples `INSERT('SUXY','TUXY',2)` = 'SUTUXYXY', `DELETE('SUXY',2,2)` = 'SY'). CODESYS leaves out-of-range positions target-dependent. | `crates/plcc-codegen/tests/strings_in_expressions.rs` |
| FOR loop whose increment passes the control variable's type limit (`FOR b := 250 TO 255` on a BYTE) | The loop **ends**; the control variable holds the wrapped value (0 here), as it would in CODESYS. TO and BY are evaluated once, before the first iteration. | **Divergence.** CODESYS help, FOR: such a loop is endless, because the counter overflows before it can exceed the end value. An endless loop in a cyclic task is never what the program means, and trips the watchdog; IEC 61131-3 does not define the overflow. | `crates/plcc-codegen/tests/for_loop_limits.rs` |
| Type checking in `compile` | `plcc compile` (and `sim`) run the HIR type checker first and refuse to build on an error; warnings do not stop the build. `plcc check` runs exactly the same check over the same merged unit (all input files plus the `--stdlib` prelude), so the two always agree. `--no-typecheck` skips it. | CODESYS does not download/build a project whose compile reports errors; warnings are listed and the build proceeds. | `crates/plcc-cli/tests/typecheck_gate.rs` |
| Undefined identifier | Type-check **error** at the identifier: not a variable in scope (including members inherited through EXTENDS), a VAR_GLOBAL (top level, CONFIGURATION or RESOURCE), an enumerator, a POU or a type. A bare callee (`nosuch(x := 1);`) is left to codegen's "unknown function". | CODESYS: "Identifier '<name>' not defined" is an error. Previously `plcc check` passed and codegen failed without a location. | `crates/plcc-hir/tests/undefined_identifiers.rs` |
| Untyped literals | Take the type their context needs (IEC 61131-3 §6.3.3: a literal without a type prefix has no fixed type): `x := 2.0 * x` is REAL arithmetic, `b := 5` stores a BYTE, `b AND 16#0F` is BYTE. An integer literal that does not fit the target (`b := 300`) warns. `q := 0` / `q := 1` into BOOL is accepted (OSCAT `TOGGLE`). | IEC; CODESYS accepts all of these. | `crates/plcc-hir/tests/codesys_acceptance.rs` |
| Lossy implicit numeric conversions | **Warning**, not error: DINT→INT, REAL→INT, LREAL→REAL, INT→UINT, DINT→REAL (24-bit mantissa), integer literal → TIME. Lossless ones (Table 11: SINT→INT, INT→REAL, DINT→LREAL, UINT→DINT, BYTE→WORD, BYTE→INT, BOOL→BYTE) are silent. | IEC allows only the lossless ones implicitly. CODESYS reports the others as "Implicit conversion from '<type 1>' to '<type 2>': possible loss of information" ([C0197](https://content.helpme-codesys.com/en/CODESYS%20Development%20System/_cds_error_c0197.html)) / "possible change of sign" (C0195) and still builds — the help page does not state the severity; that it builds is from CODESYS practice. A strict mode would make these errors. | same |
| Arithmetic on bit strings and pointers | `+ - * /` accept BYTE/WORD/DWORD/LWORD (unsigned of their width) and mix with integers and reals; unary minus on a bit string; `AND/OR/XOR/NOT` accept integers; `ptr ± int`, `int + ptr`, `ptr - ptr`; FOR over a BYTE. BOOL stays out of arithmetic, and BOOL does not mix with non-BOOL in `AND/OR/XOR`. | CODESYS extensions (IEC restricts arithmetic to ANY_NUM and logic to ANY_BIT); OSCAT relies on them (`DEC_TO_INT`, `COUNT_BR`, `BIN_TO_BYTE`, `_ARRAY_*`). | same |

## Testing

- `parse_oscat` runs in default (permissive) mode and should approach 100%.
- **Add the inverse test**: strict mode must *flag* the extensions. There is no
  conformance signal today in that direction at all.
- Every FB lifecycle feature needs an execution test, not an IR-text test. The PRINT
  path shipped untested for exactly this reason, and B1 survived 356 tests because the
  fixtures used `:= 0`.
- One regression test per semantic divergence, asserting the documented choice.

## Non-goals

**Siemens SCL** diverges lexically — `#` sigils on locals, `"` on symbolic globals,
`REGION` blocks, `S5TIME`, an OOP model that does not line up with 3rd-edition
`CLASS`/`METHOD`. That is a sibling front-end over the shared HIR (`plcc-scl`), not a
dialect flag. Conveniently the same architecture ladder needs.

**Rockwell ST** is structurally different — Add-On Instructions instead of IEC function
blocks, no pointers, a proprietary project format. Absorbing it means modelling AOIs.

**Cap it at two dialects.** Every dialect × extension pair is test matrix. If a third
vendor needs something it becomes an extension flag, never a new dialect.
