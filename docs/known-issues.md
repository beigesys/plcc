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

## TIME and date values to and from STRING

`TIME_TO_STRING`, `DT_TO_STRING`, `STRING_TO_TIME` (and the other date/time
forms) are not implemented: each is a compile error ("unknown function"). The
integer, bit-string, BOOL, REAL and LREAL forms are. Expected, per CODESYS:
`TIME_TO_STRING(T#1500ms)` = `'T#1s500ms'`, `DT_TO_STRING(DT#2024-03-15-13:45:30)`
= `'DT#2024-03-15-13:45:30'`.
