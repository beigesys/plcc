# What blocks OSCAT

Measured against `main` (560 tests), compiling each of the 559 `tests/external/oscat/*.EXP`
files individually with the default `--stdlib`, 20s timeout, 4 GiB cap.

```
parse:    559 / 559   (100%)
compile:  246 / 559   (44%)
```

Parsing has been solved for a while. Compilation is the real number, and nobody had
measured it.

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
argument failures, not missing functions. Fixing this message is the highest-value change
in this document, because every count below is distorted by it.

**Fix**: report the failing argument expression, not the enclosing call. Same principle as
the `Ok(None)` audit in `compile_lvalue_inner` — a builtin whose argument yields no value
should name the argument.

## Failure taxonomy

Counts are files, and overlap: a file can hit several.

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
- `REAL_TO_DWORD` truncates toward zero, the same as the existing `REAL_TO_DINT`
  (IEC says round; that is a separate, pre-existing discrepancy for the whole
  float→int family). It goes through i64 so values above 2^31 survive.
- `REPLACE(IN1, IN2, L, P)` clamps P into `1..LEN(IN1)+1` and L to what remains,
  truncates to the destination's length, and is safe when the destination is also
  IN1 (`s := REPLACE(s, '', 1, pos)`, the OSCAT idiom). Like CONCAT/LEFT/RIGHT/MID,
  it only works as the whole right-hand side of an assignment to a STRING
  variable, and its string arguments must be variables or literals — a nested call
  (`REPLACE(s, MID(..), ..)`, in `REPLACE_CHARS`) is not supported.

Effect: **268 / 559** compile individually (was 246). Most remaining
`DWORD_TO_TIME` argument failures (30 files) are `DWORD_TO_TIME(T_PLC_MS())`, where
`T_PLC_MS` is defined nowhere in the corpus — a cross-file/vendor artifact, not a
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

## Build order, by measured impact

1. **Fix the argument diagnostic.** Cheap, and every number above is unreliable until it
   lands. It is also the same silent-drop class that has produced most of this project's
   bugs — the value is there, the caller just does not say what went wrong.
2. **Add the five missing conversions.** `DWORD_TO_TIME` and `TIME_TO_DWORD` first — 35+
   files, and they are mechanical additions to an existing dispatch table.
3. **Pointer dereference as an lvalue.** 39 files, a real language feature, and `POINTER TO`
   is already claimed as "Full" in the README.
4. **Re-measure**, then classify the `failed to compile condition` bucket, which should be
   readable once (1) lands.
5. **Decide on CODESYS bit access** (`X.0 := A0`). 13 files. A dialect question, not a bug
   — see `docs/codesys-compatibility.md`.

## Method note

Compile each file individually, capped, never the whole corpus in one process without a
limit. A merged 559-file LLVM module is enormous, and an uncapped run of exactly this
measurement previously exhausted a 95 GiB machine. See the memory section of CLAUDE.md.
