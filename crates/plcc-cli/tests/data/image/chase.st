(* SPDX-License-Identifier: MPL-2.0
   Program-image test (tests/image.rs): a TON-timed chase over three relays
   while %IX0.0 is on; the time comes from plcc_monotonic_ns, a service. *)
PROGRAM Chase
VAR
    t : TON;
    step : INT;
    run AT %IX0.0 : BOOL;
    r1 AT %QX0.0 : BOOL;
    r2 AT %QX0.1 : BOOL;
    r3 AT %QX0.2 : BOOL;
END_VAR
t(IN := run AND NOT t.Q, PT := T#100ms);
IF t.Q THEN
    step := (step + 1) MOD 3;
END_IF
r1 := run AND step = 0;
r2 := run AND step = 1;
r3 := run AND step = 2;
END_PROGRAM
