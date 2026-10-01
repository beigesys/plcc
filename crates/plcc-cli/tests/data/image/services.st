(* SPDX-License-Identifier: MPL-2.0
   Program-image test (tests/image.rs): exercises runtime services through the
   veneers (64-bit division, libm, 64-bit <-> float conversions, plcc_print)
   and leaves the results in %M for the test to check. *)
PROGRAM Svc
VAR
    q AT %ML1 : LINT;
    r AT %ML2 : LINT;
    s AT %ML3 : LREAL;
    f AT %MD8 : REAL;
    g AT %MD9 : REAL;
    big AT %ML5 : LINT;
    n : LINT := 1234567890123;
    d : LINT := 1000;
    x : LREAL := 0.5;
    y : REAL := 0.25;
    done : BOOL;
END_VAR
IF NOT done THEN
    q := n / d;
    r := n MOD d;
    s := SIN(x) + EXPT(x, 2.5);
    f := LINT_TO_REAL(n);
    g := COS(y);
    big := n * 3;
    PRINT('services ok');
    done := TRUE;
END_IF
END_PROGRAM
