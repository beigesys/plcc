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
