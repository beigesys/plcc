(* SPDX-License-Identifier: MPL-2.0 *)
(*
 * A blinker built entirely from IEC 61131-3 standard function blocks.
 *
 * Requires the bundled ST standard library, i.e. the default
 * `--stdlib bundled-st`. With `--stdlib none` this fails to compile with an
 * "unknown type `TON`" diagnostic rather than silently doing nothing.
 *
 * The timers read the platform clock via `plcc_monotonic_ns`, so `on_time` and
 * `off_time` are real durations and do not drift with scan period.
 *)
PROGRAM StdlibBlinker
VAR
    (* One-shot rearm chain: on_delay expires -> lamp off, off_delay expires -> lamp on *)
    on_delay : TON;
    off_delay : TON;
    edge : R_TRIG;
    cycles : CTU;

    lamp : BOOL := TRUE;
    cycle_count : INT;
    reset_count : BOOL := FALSE;
END_VAR
    on_delay(IN := lamp, PT := T#250ms);
    IF on_delay.Q THEN
        lamp := FALSE;
    END_IF;

    off_delay(IN := NOT lamp, PT := T#250ms);
    IF off_delay.Q THEN
        lamp := TRUE;
    END_IF;

    (* Count completed off->on transitions of the lamp. *)
    edge(CLK := lamp);
    cycles(CU := edge.Q, R := reset_count, PV := 1000);
    cycle_count := cycles.CV;
END_PROGRAM
