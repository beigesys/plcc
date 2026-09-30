(* SPDX-License-Identifier: MPL-2.0 *)
(* TON on %IX0.0 with PT = 500 ms; Q on %QX0.0, ET mirrored to %MD0 in ms. *)
CONFIGURATION Sim
    RESOURCE Cpu ON Wasm
        TASK Fast (INTERVAL := T#10ms, PRIORITY := 1);
        TASK Slow (INTERVAL := T#100ms, PRIORITY := 2);
        PROGRAM Main WITH Fast : Delay;
        PROGRAM Count WITH Slow : Counter;
    END_RESOURCE
END_CONFIGURATION

PROGRAM Delay
VAR
    go    AT %IX0.0 : BOOL;
    done  AT %QX0.0 : BOOL;
    et_ms AT %MD0   : UDINT;
    t : TON;
END_VAR
t(IN := go, PT := T#500ms);
done := t.Q;
et_ms := TIME_TO_UDINT(t.ET);
END_PROGRAM

PROGRAM Counter
VAR
    n AT %MW4 : UINT;
END_VAR
n := n + 1;
END_PROGRAM
