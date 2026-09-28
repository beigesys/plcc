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

Verified absent from the codegen dispatch:

```
DWORD_TO_TIME    TIME_TO_DWORD    REAL_TO_DWORD    DWORD_TO_REAL    REPLACE
```

`DWORD_TO_TIME` alone appears in 35 failing files and is the single cheapest win
available. The DWORD⇄TIME/REAL pair is idiomatic OSCAT — it stores durations as raw
milliseconds in a DWORD.

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
