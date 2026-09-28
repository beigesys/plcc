# Known issues

Defects and gaps found by execution and not fixed yet, each with a reproduction
and the expected versus actual result. None of these is silent: each is a
compile error today unless it says otherwise.

## Calling a method through an unbound INTERFACE reference

`itf.M()` with `itf` never assigned (or assigned 0) dereferences a null method
table and faults, as it raises an exception in CODESYS. Guard with
`IF itf <> 0 THEN`. There is no `__QUERYINTERFACE` / `__QUERYPOINTER` yet.

## `SUPER^.M()` binds the base method's own inner calls statically

In `Derived.M`, `SUPER^.M()` runs `Base.M` as compiled for `Base`, so a call to
another method from inside `Base.M` reaches `Base`'s version even when `Derived`
overrides it. CODESYS would call the override. Direct calls, `THIS^.M()` and
plain `M()` calls, and inherited methods are all late-bound correctly.

## STRING to TIME and date values

`STRING_TO_TIME`, `STRING_TO_DATE`, `STRING_TO_TOD`, `STRING_TO_DT` are not
implemented (a compile error, "unknown function"). The opposite direction is:
`TIME_TO_STRING(T#1500ms)` = `'T#1s500ms'`, `DT_TO_STRING(..)` =
`'DT#2024-12-31-23:59:59'`.

## Not implemented (each is a compile error, never silent)

| Construct | Example | Status |
|---|---|---|
| PROPERTY (CODESYS GET/SET) | `PROPERTY Level : REAL` | not parsed |
| Variable-length arrays | `VAR_IN_OUT a : ARRAY[*] OF INT; END_VAR`, `LOWER_BOUND(a, 1)` | not parsed |
| WSTRING values in expressions | `LEN(ws)`, `CONCAT(ws, "x")`, `ws = "abc"` | WSTRING works in declarations, initializers and `:=` of literals/variables only |
| A member of a call's result | `Add3(a, b).x` | "does not resolve to a field" |
| `__NEW` / `__DELETE`, `__QUERYINTERFACE` | | unknown function |
| Calling a PROGRAM with a CONFIGURATION present | `Sub();` | unknown function (fine without a CONFIGURATION) |
| `SUPER^` of `SUPER^` late binding | see above | static inside the base implementation |
