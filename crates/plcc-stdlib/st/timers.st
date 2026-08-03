(* SPDX-License-Identifier: MPL-2.0 *)
(*
 * IEC 61131-3 section 2.5.2.4 — timers.
 *
 * These read the platform clock through MONOTONIC_NS(), which compiles to a
 * call to the external `plcc_monotonic_ns() -> LINT` symbol. Elapsed time is
 * therefore measured against real time, not counted in scans, so timing does
 * not drift with scan period. See docs/runtime-symbols.md.
 *
 * TIME and LTIME are laid out as signed 64-bit nanoseconds, which is what
 * MONOTONIC_NS() returns, so no conversion is needed anywhere below.
 *)

(* TON — on-delay timer.
 *   Q goes TRUE after IN has been continuously TRUE for PT.
 *   ET counts up from 0 and is capped at PT, where it freezes.
 *   IN going FALSE resets Q and ET immediately. *)
FUNCTION_BLOCK TON
VAR_INPUT
    IN : BOOL;
    PT : TIME;
END_VAR
VAR_OUTPUT
    Q : BOOL;
    ET : TIME;
END_VAR
VAR
    RUNNING : BOOL := FALSE;
    START_NS : LINT;
    NOW_NS : LINT;
    ELAPSED : LINT;
END_VAR
    NOW_NS := MONOTONIC_NS();
    IF IN THEN
        IF NOT RUNNING THEN
            RUNNING := TRUE;
            START_NS := NOW_NS;
        END_IF;
        ELAPSED := NOW_NS - START_NS;
        IF ELAPSED >= PT THEN
            ET := PT;
            Q := TRUE;
        ELSE
            ET := ELAPSED;
            Q := FALSE;
        END_IF;
    ELSE
        RUNNING := FALSE;
        ET := T#0s;
        Q := FALSE;
    END_IF;
END_FUNCTION_BLOCK

(* TOF — off-delay timer. The mirror of TON.
 *   Q goes TRUE immediately when IN goes TRUE.
 *   On the falling edge of IN, ET counts up from 0; when it reaches PT, Q goes
 *   FALSE and ET freezes at PT.
 *   IN going TRUE again clears ET and cancels the off-delay. *)
FUNCTION_BLOCK TOF
VAR_INPUT
    IN : BOOL;
    PT : TIME;
END_VAR
VAR_OUTPUT
    Q : BOOL;
    ET : TIME;
END_VAR
VAR
    PREV_IN : BOOL := FALSE;
    RUNNING : BOOL := FALSE;
    START_NS : LINT;
    NOW_NS : LINT;
    ELAPSED : LINT;
END_VAR
    NOW_NS := MONOTONIC_NS();
    IF IN THEN
        RUNNING := FALSE;
        ET := T#0s;
        Q := TRUE;
    ELSE
        IF PREV_IN THEN
            RUNNING := TRUE;
            START_NS := NOW_NS;
        END_IF;
        IF RUNNING THEN
            ELAPSED := NOW_NS - START_NS;
            IF ELAPSED >= PT THEN
                ET := PT;
                Q := FALSE;
                RUNNING := FALSE;
            ELSE
                ET := ELAPSED;
                Q := TRUE;
            END_IF;
        END_IF;
    END_IF;
    PREV_IN := IN;
END_FUNCTION_BLOCK

(* TP — pulse timer.
 *   A rising edge of IN starts a pulse of length PT: Q is TRUE and ET counts up.
 *   The pulse is NOT retriggerable — further rising edges of IN while the pulse
 *   is running are ignored, and ET is not restarted.
 *   When the pulse ends, Q goes FALSE and ET holds at PT until IN goes FALSE. *)
FUNCTION_BLOCK TP
VAR_INPUT
    IN : BOOL;
    PT : TIME;
END_VAR
VAR_OUTPUT
    Q : BOOL;
    ET : TIME;
END_VAR
VAR
    PREV_IN : BOOL := FALSE;
    RUNNING : BOOL := FALSE;
    START_NS : LINT;
    NOW_NS : LINT;
    ELAPSED : LINT;
    IN_EDGE : BOOL;
END_VAR
    NOW_NS := MONOTONIC_NS();
    IN_EDGE := IN AND NOT PREV_IN;
    PREV_IN := IN;
    IF IN_EDGE THEN
        IF NOT RUNNING THEN
            RUNNING := TRUE;
            START_NS := NOW_NS;
        END_IF;
    END_IF;
    IF RUNNING THEN
        ELAPSED := NOW_NS - START_NS;
        IF ELAPSED >= PT THEN
            ET := PT;
            Q := FALSE;
            RUNNING := FALSE;
        ELSE
            ET := ELAPSED;
            Q := TRUE;
        END_IF;
    ELSE
        Q := FALSE;
        IF NOT IN THEN
            ET := T#0s;
        END_IF;
    END_IF;
END_FUNCTION_BLOCK
