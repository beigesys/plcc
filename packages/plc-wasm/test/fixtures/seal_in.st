(* SPDX-License-Identifier: MPL-2.0 *)
(* Motor seal-in: start latches the motor, stop drops it. *)
PROGRAM SealIn
VAR
    start AT %IX0.0 : BOOL;
    stop  AT %IX0.1 : BOOL;
    motor AT %QX0.0 : BOOL;
    starts AT %MW0  : UINT;
    runs  : DINT;
    level : REAL := 1.5;
    wave  : REAL;
    last  : BOOL;
    name  : STRING[16] := 'seal';
END_VAR
motor := (start OR motor) AND NOT stop;
IF motor AND NOT last THEN
    starts := starts + 1;
    PRINT('motor on');
END_IF;
last := motor;
runs := runs + 1;
wave := SIN(level);
END_PROGRAM
