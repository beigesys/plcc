(* SPDX-License-Identifier: MPL-2.0 *)
(*
 * IEC 61131-3 section 2.5.2.2 — bistable elements.
 *
 * The bodies below are the standard's own ST definitions.
 *)

(* SR — SET-dominant bistable.
 * When S1 and R are both TRUE, Q1 is TRUE. *)
FUNCTION_BLOCK SR
VAR_INPUT
    S1 : BOOL;
    R : BOOL;
END_VAR
VAR_OUTPUT
    Q1 : BOOL;
END_VAR
    Q1 := S1 OR (NOT R AND Q1);
END_FUNCTION_BLOCK

(* RS — RESET-dominant bistable.
 * When S and R1 are both TRUE, Q1 is FALSE. *)
FUNCTION_BLOCK RS
VAR_INPUT
    S : BOOL;
    R1 : BOOL;
END_VAR
VAR_OUTPUT
    Q1 : BOOL;
END_VAR
    Q1 := NOT R1 AND (S OR Q1);
END_FUNCTION_BLOCK
