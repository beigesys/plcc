# What blocks OSCAT

Measured against `main`, compiling each of the 559 `tests/external/oscat/*.EXP` files
individually with the default `--stdlib`, 20 s timeout, 4 GiB cap.

```
parse:    559 / 559   (100%)
compile:  299 / 559   (54%)     was 246 / 559 (44%)
```

Parsing has been solved for a while. Compilation is the real number.

## The diagnostic used to lie, and it cost an hour

`failed to compile argument for MIN` did **not** mean `MIN` was unimplemented. It meant
an *argument expression* returned `Ok(None)`, and the builtin reported the failure
against itself rather than against the thing that actually failed. Three of the top
buckets were that one message.

Every diagnostic on this path now names the sub-expression and why it has no value:

```
in the call to DWORD_TO_TIME: `T_PLC_MS` is neither a standard function nor a
    FUNCTION declared in this compilation unit
in the call to MIN: `LANGUAGE.LMAX` is not a variable in scope — a VAR_GLOBAL or
    CONSTANT declared in another file is not visible when files are compiled one at a time
in the IF condition: `in.7` is CODESYS bit access on a scalar, which is not supported
```

The same treatment went to the IF/ELSIF/WHILE/UNTIL conditions, the FOR bounds and the
CASE selector, which between them accounted for 37 unreadable failures. Causes are
ranked: a name checked against the symbol table beats one inferred from spelling, so
`IF LEN(list) + LEN(ins) > LIST_LENGTH` blames the missing `LIST_LENGTH` rather than
`LEN`, which is implemented. Getting that ranking wrong is how the original message
came about.

## Failure taxonomy

260 files fail. Counts are files and overlap — a file can hit several, and only the
first is reported.

| Cause | Files | Classification |
|---|---:|---|
| Unknown type (`complex`, `Vector_3`, `SDT`, …) | 83 | **cross-file artifact** — resolves when compiled together |
| Call to something undeclared (`T_PLC_MS` ×31, date helpers, …) | 82 | mixed: mostly cross-file, `T_PLC_MS` is vendor-supplied |
| Name not in scope (`LANGUAGE.LMAX`, `math.PI2`, …) | 29 | **cross-file artifact** |
| Pointer arithmetic (`pt := pt + 1`) | 23 | **decision needed** — see below |
| String builtin nested in an expression (`FIND` inside `IF`) | 10 | **real gap** — needs a result buffer |
| CODESYS bit access on a scalar (`X.0`, `in.7`) | 11 | **vendor extension** |
| String literal used as a value or argument | 6 | **real gap** — literals have no storage |
| Miscellaneous member/type resolution | ~16 | mixed |

Most of the corpus is still not blocked by compiler defects. It is blocked by being
compiled one file at a time, which OSCAT is not written for.

## What landed since the first measurement

- Argument, condition, bound and selector diagnostics that name the real cause.
- `DWORD_TO_TIME` / `TIME_TO_DWORD` — a duration in a DWORD is milliseconds, which is
  what CODESYS means and what OSCAT assumes; TIME is nanoseconds internally, so these
  scale rather than reinterpret.
- `DWORD_TO_REAL` / `REAL_TO_DWORD`, plus a generic lowering for every remaining
  `X_TO_Y` between a number and a bit string, so the next missing pair does not have to
  be discovered by a corpus run. ANY_DATE is deliberately excluded — date literals still
  compile to a placeholder, and converting one would look like it worked.
- `REPLACE`, built through a scratch buffer so `s := REPLACE(s, t, l, p);` does not
  overwrite its own input.
- **Typed literals had no codegen at all.** `BYTE#255`, `DWORD#1`, `WORD#16#FFFF` were
  silently valueless, which is why `SHL(x, BYTE#1)` failed where `SHL(x, 1)` worked. The
  README claimed the feature was complete.
- Pointer dereference as an lvalue (`pt^ := x`, `pt^[i] := x`, `pt^.f := x`) and as a
  value, plus `ADR` — without a way to take an address there was no way to reach the
  write path at all.
- A pointer operand in a binary operator panicked the compiler (`into_int_value` on a
  `PointerValue`) in 23 files. It is a diagnostic now.

## Genuinely missing, in measured order

1. **Pointer arithmetic** (`pt := pt + 1`) — 23 files. Not a bug to fix but a semantic
   question to answer first: does `+ 1` advance one byte or one element? The two agree
   for `POINTER TO BYTE`, which is most of OSCAT, and disagree everywhere else. Guessing
   miscomputes addresses silently. See `docs/codesys-compatibility.md`.
2. **String literals with storage** — 16 files across the "no value" and "no address"
   buckets. `s := 'ready';` also compiles to nothing today, with no diagnostic.
3. **String builtins nested in expressions** — 10 files. `FIND(s, 'x')` inside an `IF`
   needs a temporary the result can live in.
4. **CODESYS bit access on a scalar** — 11 files. A dialect decision.
5. **Date and time-of-day literals** — they compile to `0`. Nothing in the corpus fails
   on this yet, because the conversions that would expose it are the ones deliberately
   left out.

## An anomaly worth chasing

A merged compile of all 559 files stops on `array_var` being undeclared, and so does the
two-file case — in one direction only:

```
plcc compile ARRAY_SDV.EXP ARRAY_VAR.EXP   # error: `array_var` is not declared
plcc compile ARRAY_VAR.EXP ARRAY_SDV.EXP   # compiles
```

A synthetic pair of files with the same shape does *not* reproduce it, so the cause is
something specific to these POUs and is not yet identified. Since compiling files
together is the way past the 83-file cross-file bucket, this is worth understanding
before the next measurement.

## Method note

Compile each file individually, capped, never the whole corpus in one process without a
limit. A merged 559-file LLVM module is enormous, and an uncapped run of exactly this
measurement previously exhausted a 95 GiB machine. See the memory section of CLAUDE.md.
