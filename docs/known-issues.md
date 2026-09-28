# Known issues

Defects and gaps found by execution and not fixed yet, each with a reproduction
and the expected versus actual result. None of these is silent: each is a
compile error today unless it says otherwise.

## INTERFACE references are not compiled

```iec
INTERFACE IShape METHOD Area : REAL END_METHOD END_INTERFACE
FUNCTION_BLOCK Sq IMPLEMENTS IShape
VAR w : REAL := 2.0; END_VAR
METHOD Area : REAL Area := w * w; END_METHOD
END_FUNCTION_BLOCK
PROGRAM p
VAR s : Sq; itf : IShape; a : REAL; END_VAR
itf := s;
a := itf.Area();
END_PROGRAM
```

Expected `a = 4.0`. Actual: a compile error (`itf` is laid out as a pointer, but
neither the assignment of an instance to it nor a method call through it is
lowered). Needs a (instance pointer, method table) pair per interface
variable and one method table per (POU, interface).

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
