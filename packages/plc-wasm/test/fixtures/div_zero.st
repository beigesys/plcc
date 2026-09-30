(* SPDX-License-Identifier: MPL-2.0 *)
(* %MW0 is the divisor: 0 faults (integer division by zero). *)
PROGRAM Ratio
VAR
    divisor AT %MW0 : INT;
    lamp    AT %QX0.0 : BOOL;
    q       AT %MW1 : INT;
    after   : INT;
END_VAR
lamp := TRUE;
q := 100 / divisor;
after := after + 1;
END_PROGRAM
