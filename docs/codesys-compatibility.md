# CODESYS Compatibility Plan

Status: proposal. Nothing here is implemented yet.

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

**Default to CODESYS behaviour** — that is what real code expects — and maintain a table
in this document of each divergence, which way we went, and why. Strict mode should warn
where the two differ, since that is exactly the portability question strict mode exists
to answer.

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
