(* SPDX-License-Identifier: MPL-2.0 *)
(*
 * IEC 61131-3 section 2.5.2.3 — edge detection.
 *
 * The bodies below are the standard's own ST definitions, including the
 * asymmetric initial value of the edge memory: R_TRIG's M starts FALSE, so a
 * CLK that is already TRUE on the first call produces a pulse; F_TRIG's M
 * starts TRUE, so a CLK that is already FALSE on the first call does not.
 *)

(* R_TRIG — rising edge detector.
 * Q is TRUE for exactly one scan following a FALSE -> TRUE transition of CLK. *)
FUNCTION_BLOCK R_TRIG
VAR_INPUT
    CLK : BOOL;
END_VAR
VAR_OUTPUT
    Q : BOOL;
END_VAR
VAR
    M : BOOL := FALSE;
END_VAR
    Q := CLK AND NOT M;
    M := CLK;
END_FUNCTION_BLOCK

(* F_TRIG — falling edge detector.
 * Q is TRUE for exactly one scan following a TRUE -> FALSE transition of CLK. *)
FUNCTION_BLOCK F_TRIG
VAR_INPUT
    CLK : BOOL;
END_VAR
VAR_OUTPUT
    Q : BOOL;
END_VAR
VAR
    M : BOOL := TRUE;
END_VAR
    Q := NOT CLK AND NOT M;
    M := NOT CLK;
END_FUNCTION_BLOCK
