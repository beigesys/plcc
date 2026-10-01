(* SPDX-License-Identifier: MPL-2.0
   Program-image test (tests/image.rs): a division by zero on the third scan
   must reach the runtime's plcc_fault service, not the object's weak default. *)
PROGRAM Div
VAR
    a : INT := 10;
    b : INT := 0;
    c AT %MW0 : INT;
    n AT %MW1 : INT;
END_VAR
n := n + 1;
IF n >= 3 THEN
    c := a / b;
END_IF
END_PROGRAM
