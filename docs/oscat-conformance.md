# What blocks OSCAT

Measured with the debug `plcc` (816 workspace tests passing), 20 s timeout per
invocation, 4 GiB cap (16 GiB for the merged run), default `--stdlib`.

```
parse:                                   559 / 559   (100%)
compile, each file alone:                278 / 559   (50%)   no panics (exit 101): 0
compile, each file + 2 global constants: 306 / 559   (55%)
compile, whole corpus in one invocation: 559 / 559   (100%)  with OSCAT's global variable list
                                         (4.4 s, 97 MB peak RSS)
```

**The whole corpus compiles.** OSCAT is a library of siblings, and the .EXP export
omits its global variable list (`MATH`, `PHYS`, `LANGUAGE`, `SETUP`, `LOCATION` of the
`CONSTANTS_*` types it does contain, and the constants `STRING_LENGTH`/`LIST_LENGTH`).
With that list supplied (one 14-line file declaring exactly those seven globals), a
single `plcc compile` of all 559 files type-checks and builds, also at `-O3`. Without
it, the type checker reports the 209 uses of those undeclared globals and stops.

It also *runs*: a generated program calling all 267 OSCAT FUNCTIONs whose parameters
are scalars (math, bit, string, date/time, conversion families) executes, spot-checked
against hand-computed values (`GCD(12, 18)` = 6, `CAPITALIZE('hello world')`, `DAY_OF_YEAR`,
`MONTH_TO_STRING(3, 0, 0)` = 'March' through the LANGUAGE global's STRUCT defaults,
`REAL_TO_STRF(3.14159, 2, '.')` = '3.14', ...), and gives bit-identical results at `-O0`
and `-O3`.

Per file, every remaining failure is a cross-file reference: a sibling FUNCTION
(`T_PLC_MS` 45, `INC1` 7, `YEAR_OF_DATE` 5, `T_PLC_US` 5, ...), a sibling type (`complex`
24, `Vector_3` 14, `INTEGRATE` 5, ...), or one of the global variables above. The "alone"
number went *down* from 299 to 278 during this round for a good reason: a
`STRING(STRING_LENGTH)` or `ARRAY[1..LIST_LENGTH]` whose constant is not declared is now an
error instead of silently becoming a 255-character string or a one-element array.

What changed since 242 (each with execution tests; see docs/codesys-compatibility.md):
all elementary `X_TO_Y` conversions including DATE/TOD/DT (CODESYS units), integer and
REAL `_TO_STRING` / `STRING_TO_`, STRING values anywhere in an expression, CODESYS bit
access `x.3`, enumerators (bare, `E#V`, `E.V`), constant-folded array bounds and string
lengths, FUNCTIONs callable before their declaration, nested comments, `S=`/`R=`,
REFERENCE TO, EXTENDS/THIS^/SUPER^, INTERFACE references.

## Earlier state: 241, and why it was lower than the 268 below

The earlier **268 was inflated**. An assignment whose right-hand side produced no value
skipped the store *silently*, so a file like `BIN_TO_BYTE` "compiled" with
`pt := ADR(bin);` emitting nothing at all — `ADR` did not exist. Making that a diagnostic
(it is the same silent-drop class as the argument diagnostic below) took the honest count
to **194**. The three fixes since then brought it to 241:

| Change | Effect |
|---|---|
| Non-integer operands lowered or reported instead of `into_int_value()` panics: CODESYS byte-addressed pointer arithmetic (`pt := pt + 1`), TRUNC → DINT, `X_TO_REAL` of a REAL | 13 panics → 0 |
| Typed literals (`DWORD#1`, `BYTE#255`, `INT#-5`) in every value position | 9 of the 11 affected files compile; the other 2 now stop on `DATE_TO_UDINT` |
| `pt^` as value and target, `pt^[i]`, `ADR()`, `SIZEOF()`; FUNCTION locals and result zeroed per call | the pointer bucket (50) and the `ADR` bucket (49) are gone |

Remaining failures, by the **first** error in each of the 318 files:

| Files | Cause | Classification |
|---:|---|---|
| 155 | unknown function — **122 are OSCAT siblings** (`T_PLC_MS` 47, `INC1` 7, `T_PLC_US` 5, `DAY_OF_YEAR` 5, `TO_UPPER` 4, `YEAR_OF_DATE`, `INC`, `CHR_TO_STRING`, …); **33 are missing conversions**: the DATE/TOD/DT family (`DATE_TO_DWORD` 6, `DT_TO_DWORD` 5, `TOD_TO_DWORD` 3, `DWORD_TO_DATE` 3, `DATE_TO_UDINT` 3, `DWORD_TO_TOD` 2, `DT_TO_DATE` 2, `DT_TO_UDINT` 1), plus `UINT_TO_DWORD` 2, `DWORD_TO_WORD`, `DINT_TO_WORD`, `BYTE_TO_REAL`, `DWORD_TO_STRING`, and the CODESYS `TIME()` 2 | mostly cross-file; 33 real |
| 83 | unknown type in a declaration (`complex` 24, `Vector_3` 14, `INTEGRATE` 5, `esr_data` 4, …) | cross-file |
| 36 | unknown identifier — OSCAT globals `math` 11, `LANGUAGE` 7, `LIST_LENGTH` 7, `setup` 6, `STRING_LENGTH` 3, `phys` 2 | cross-file |
| 27 | member access: CODESYS bit access on a scalar (`in.0`, `cnt.0`, `SX[i].0`; ~18), a field of a cross-file STRUCT/global (`math.FACTS`, `setup.DECADES`, `complex.re`; ~9) | vendor extension / cross-file |
| 17 | STRING in a scalar position: string literal as a value (9), `MID`/`RIGHT`/`REPLACE` nested in an expression (8) | **real gap** |

So of the 318, roughly 250 are this measurement's artifact (one file at a time, where
OSCAT is a library of siblings and globals), and the real compiler gaps left are the
DATE/TOD/DT conversions (needs a units decision), STRING values in expressions, and the
CODESYS bit-access dialect question.

### Silent wrong results

Fixed, each with JIT tests in `crates/plcc-codegen/tests/silent_miscompiles.rs` that fail
without the fix: `RETURN` did nothing; `NOT` on BYTE/SINT was a boolean NOT; USINT/UINT
were stored one size up and never wrapped; integer `**` returned its left operand (now
EXPT with a REAL result, per the standard); STRING initial values, string literal
assignments and string literal FB inputs were dropped; non-literal VAR_GLOBAL initializers
were zeroed; a longer STRING assigned to a shorter one overwrote the following variables;
BYTE/WORD/USINT values converted to REAL as signed; DATE/TOD/DT literals were 0; user
FUNCTION arguments were evaluated twice (and builtins shadowed user FUNCTIONs); `$`
escapes were not decoded; a call statement to an unknown function compiled to nothing.

Found and **not** fixed (semantics decisions, not one-liners):

- ~~`REAL_TO_INT` / `REAL_TO_DINT` / `LREAL_TO_*` / assignment of a REAL to an integer
  **truncate**.~~ Fixed: they round to nearest, halves away from zero, as CODESYS does
  (`crates/plcc-codegen/tests/float_to_int.rs`; decision in codesys-compatibility.md).
- ~~Float-to-integer conversion uses a plain `fptosi`, which is LLVM *poison* for an
  out-of-range value.~~ Fixed: it saturates, NaN → 0 (`llvm.fpto[su]i.sat`).
- ~~`plcc compile` never runs the HIR type checker; `plcc check` does.~~ Fixed:
  `compile` runs the same check first (`--no-typecheck` skips it), after its false
  positives were fixed — untyped literals now take their context's type, so
  `x := 2.0 * x` on a REAL is clean. Checking all 559 files individually: 0 rejected,
  0 warnings. See codesys-compatibility.md.


The sections below are the history of the earlier measurements.

## The diagnostic lies, and it cost me an hour

`failed to compile argument for MIN` does **not** mean `MIN` is unimplemented. `MIN` is
implemented. The message means an *argument expression* returned `Ok(None)`, and the
builtin reported the failure against itself rather than against the thing that actually
failed.

Three of the top buckets are this message, and the real causes are elsewhere:

```
MIN(L, LANGUAGE.LMAX)        LANGUAGE is a VAR_GLOBAL in another file
DWORD_TO_TIME(T_PLC_MS())    T_PLC_MS is not defined anywhere in the corpus
SHR(TIME_TO_DWORD(PT), 1)    TIME_TO_DWORD is genuinely missing
```

Checked against the source: `DWORD_TO_INT`, `DWORD_TO_DINT`, `DINT_TO_REAL`, `MIN`, `SHR`,
`SHL`, `FIND` and `SQRT` are all **present**. Their appearances in the failure counts are
argument failures, not missing functions.

**Fixed.** Every `compile_expression` leaf that returns `Ok(None)` now records why, and
the site that needed a value (a builtin argument, an IF/ELSIF/WHILE/UNTIL condition, a
FOR bound, a CASE selector, an array index) reports the slot, the expression, and the
innermost culprit:

```
argument 1 of `DWORD_TO_TIME` (`T_PLC_MS(..)`) produced no value: unknown function
`T_PLC_MS` (no builtin and no FUNCTION of that name in this compilation unit)
```

(`T_PLC_MS` does exist — `T_PLC_MS.EXP` — it is just a sibling file.) Re-measured with
the new messages, still 246 / 559; the 313 failures by real cause (counts are files; the
full tally is produced by the per-file run, see *Method note*):

| Files | Cause | Classification |
|---:|---|---|
| 95 | unknown function — 52 are OSCAT siblings (`T_PLC_MS` alone 31, `LEAP_OF_DATE`, `YEAR_OF_DATE`, …); 43 are missing conversions (`TIME_TO_DWORD` 9, `DATE_TO_DWORD` 6, `REAL_TO_DWORD` 6, `DT_TO_DWORD` 5, `INT_TO_DWORD` 4, `UINT_TO_INT` 3, `TOD_TO_DWORD` 3, `DWORD_TO_REAL` 3, …) | mixed |
| 83 | unknown type in a declaration (`complex`, `LANGUAGE`, …) | cross-file |
| 50 | pointer dereference: `pt^[i]` untyped (30), `pt^` as a value (11), `pt^` as an lvalue (9) | **real gap** |
| 25 | unknown identifier — OSCAT globals `LANGUAGE`, `setup`, `math`, `LIST_LENGTH`, `STRING_LENGTH` | cross-file |
| 16 | STRING in a scalar position: string literal (9), `MID`/`LEFT` nested in an expression (7) | **real gap** |
| 13 | codegen **panic** — `into_int_value` on a pointer/float operand in a binary op (`*_TO_BYTE`, `*_TO_DWORD`, `_BUFFER_*`, `CMP`) | **real bug** |
| 12 | typed literal in an expression (`DWORD#1`, `BYTE#255`, `INT#10`) | **real gap** |
| 17 | member access that does not resolve: CODESYS bit access (`in.0`, `N.0`; 10), a field the STRUCT lacks (`complex.re`, `SDT.YEAR`; 7) | vendor extension / unclassified |

The old `failed to compile condition` / `to` / `from` bucket (37) dissolved into the rows
above — it was the same misattribution.

## Failure taxonomy

Counts are files, and overlap: a file can hit several. This table predates the diagnostic
fix; the re-measured table in the previous section supersedes it.

| Cause | Files | Classification |
|---|---:|---|
| Unknown type (`complex`, `LANGUAGE`, …) | 83 | **cross-file artifact** — resolves when compiled together |
| Argument failed to compile (misattributed, see above) | ~109 | mixed: mostly cross-file, some real |
| Pointer dereference as an lvalue (`pt^[i] := x`, `pt^ := x`) | 39 | **real gap** |
| `failed to compile condition` / `to` / `from` | 37 | **unclassified** — needs the diagnostic fix to see through |
| CODESYS bit access on a scalar (`X.0 := A0`) | 13 | **vendor extension** |

Most of the corpus is not blocked by compiler defects. It is blocked by being compiled one
file at a time, which OSCAT is not written for — `T_PLC_MS`, `LANGUAGE`, and friends live
in sibling files or are vendor-supplied. The merged whole-corpus compile gets much further
and dies on `REAL_TO_DWORD`.

## Genuinely missing standard functions

The five that were missing — `DWORD_TO_TIME`, `TIME_TO_DWORD`, `REAL_TO_DWORD`,
`DWORD_TO_REAL`, `REPLACE` — are now implemented, together with the siblings OSCAT
uses from the same families:

```
TIME_TO_UDINT  TIME_TO_DINT  TIME_TO_REAL  TIME_TO_LREAL
UDINT_TO_TIME  DINT_TO_TIME  REAL_TO_TIME  LREAL_TO_TIME
REAL_TO_UDINT  LREAL_TO_DWORD  LREAL_TO_UDINT
WORD_TO_REAL   DWORD_TO_LREAL  UDINT_TO_LREAL
INT_TO_DWORD   INT_TO_UDINT  UINT_TO_INT  DWORD_TO_BYTE  BOOL_TO_DWORD
```

Semantics (tests in `crates/plcc-codegen/tests/oscat_conversions.rs`):

- TIME is i64 nanoseconds internally; every TIME⇄number conversion is in
  **milliseconds**, the IEC/CODESYS convention OSCAT relies on. TIME→integer
  truncates sub-millisecond parts toward zero.
- DWORD/UDINT sources are unsigned: `DWORD_TO_TIME(16#FFFFFFFF)` is ~49.7 days, not
  -1 ms, and `DWORD_TO_REAL` is `uitofp`.
- `REAL_TO_DWORD` rounds to nearest like every REAL → integer conversion (it
  truncated when this was written). It goes through i64 so values above 2^31 survive.
- `REPLACE(IN1, IN2, L, P)` clamps P into `1..LEN(IN1)+1` and L to what remains,
  truncates to the destination's length, and is safe when the destination is also
  IN1 (`s := REPLACE(s, '', 1, pos)`, the OSCAT idiom). Like CONCAT/LEFT/RIGHT/MID,
  it only works as the whole right-hand side of an assignment to a STRING
  variable, and its string arguments must be variables or literals — a nested call
  (`REPLACE(s, MID(..), ..)`, in `REPLACE_CHARS`) is not supported.

Effect: **268 / 559** compile individually (was 246) — an overcount, see the top: the
silent store drop let files with an unknown right-hand side pass. Most remaining
`DWORD_TO_TIME` argument failures (30 files) are `DWORD_TO_TIME(T_PLC_MS())`, where
`T_PLC_MS` lives in the sibling `T_PLC_MS.EXP` — a cross-file artifact, not a
missing conversion.

Still absent from the dispatch, and next in line by OSCAT usage: the DATE/TOD/DT
family (`DATE_TO_DWORD` 18, `DT_TO_DWORD` 14, `TOD_TO_DWORD` 10, `DWORD_TO_DT` 8,
`DT_TO_DATE`, `DT_TO_TOD`, `DWORD_TO_DATE`, `DWORD_TO_TOD`, `TOD_TO_DINT`) — these
need a decision on units (seconds since epoch vs. ms since midnight) — and the
string conversions (`INT_TO_STRING`, `REAL_TO_STRING`, `STRING_TO_INT`).

Separately, 13 files currently *panic* rather than report an error, all in
pre-existing code that calls `into_int_value()` on a value that is not an integer:
11 in `compile_binary_op`'s integer path (`BIN_TO_BYTE`, `HEX_TO_DWORD`,
`_BUFFER_INIT`, …), and 2 in older conversion arms (`INT_TO_REAL` in `CMP`, the
narrowing arm in `RANGE_TO_BYTE`) handed a float. Crashes, not missing functions.
**Fixed** — see the top.

## Build order, by measured impact (as of 268; items 1–3 are done)

1. **Fix the argument diagnostic.** Cheap, and every number above is unreliable until it
   lands. It is also the same silent-drop class that has produced most of this project's
   bugs — the value is there, the caller just does not say what went wrong.
2. **Add the five missing conversions.** `DWORD_TO_TIME` and `TIME_TO_DWORD` first — 35+
   files, and they are mechanical additions to an existing dispatch table.
3. **Pointer dereference as an lvalue.** 39 files, a real language feature, and `POINTER TO`
   is already claimed as "Full" in the README.
4. **Re-measure**, then classify the `failed to compile condition` bucket, which should be
   readable once (1) lands.
5. **Decide on CODESYS bit access** (`X.0 := A0`). ~18 files now. A dialect question, not a bug
   — see `docs/codesys-compatibility.md`.

Next, from the 241 measurement: the DATE/TOD/DT conversions (33 files once the units
are decided), STRING values in expressions (17), then bit access. Everything else in
the remaining table resolves only by compiling OSCAT as the library it is.

## Method note

Compile each file individually, capped, never the whole corpus in one process without a
limit. A merged 559-file LLVM module is enormous, and an uncapped run of exactly this
measurement previously exhausted a 95 GiB machine. See the memory section of CLAUDE.md.
