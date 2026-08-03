(* SPDX-License-Identifier: MPL-2.0 *)
(*
 * IEC 61131-3 section 2.5.2.3.3 — counters.
 *
 * The standard declares the count inputs as `CU : BOOL R_EDGE`, i.e. the
 * counting inputs are edge-triggered. That declaration form is expressed here
 * with an explicit edge memory, which is exactly equivalent and does not rely
 * on R_EDGE support in the front end.
 *
 * CV saturates at the limits of INT. It does NOT wrap: the standard guards the
 * increment with `CV < PVmax` and the decrement with `CV > PVmin`.
 *)

(* CTU — up counter.
 * R has priority over CU. Q := (CV >= PV). *)
FUNCTION_BLOCK CTU
VAR_INPUT
    CU : BOOL;
    R : BOOL;
    PV : INT;
END_VAR
VAR_OUTPUT
    Q : BOOL;
    CV : INT;
END_VAR
VAR
    M_CU : BOOL := FALSE;
    CU_EDGE : BOOL;
END_VAR
    CU_EDGE := CU AND NOT M_CU;
    M_CU := CU;
    IF R THEN
        CV := 0;
    ELSIF CU_EDGE THEN
        IF CV < 32767 THEN
            CV := CV + 1;
        END_IF;
    END_IF;
    Q := CV >= PV;
END_FUNCTION_BLOCK

(* CTD — down counter.
 * LD has priority over CD and loads CV := PV. Q := (CV <= 0). *)
FUNCTION_BLOCK CTD
VAR_INPUT
    CD : BOOL;
    LD : BOOL;
    PV : INT;
END_VAR
VAR_OUTPUT
    Q : BOOL;
    CV : INT;
END_VAR
VAR
    M_CD : BOOL := FALSE;
    CD_EDGE : BOOL;
END_VAR
    CD_EDGE := CD AND NOT M_CD;
    M_CD := CD;
    IF LD THEN
        CV := PV;
    ELSIF CD_EDGE THEN
        IF CV > -32768 THEN
            CV := CV - 1;
        END_IF;
    END_IF;
    Q := CV <= 0;
END_FUNCTION_BLOCK

(* CTUD — up/down counter.
 *
 * Precedence, per the standard's body:
 *   R dominates LD, and both dominate counting.
 *   When CU and CD are edge-active in the same scan, CV does NOT change.
 *
 * QU := (CV >= PV);  QD := (CV <= 0). *)
FUNCTION_BLOCK CTUD
VAR_INPUT
    CU : BOOL;
    CD : BOOL;
    R : BOOL;
    LD : BOOL;
    PV : INT;
END_VAR
VAR_OUTPUT
    QU : BOOL;
    QD : BOOL;
    CV : INT;
END_VAR
VAR
    M_CU : BOOL := FALSE;
    M_CD : BOOL := FALSE;
    CU_EDGE : BOOL;
    CD_EDGE : BOOL;
    BOTH : BOOL;
END_VAR
    CU_EDGE := CU AND NOT M_CU;
    CD_EDGE := CD AND NOT M_CD;
    M_CU := CU;
    M_CD := CD;
    BOTH := CU_EDGE AND CD_EDGE;
    IF R THEN
        CV := 0;
    ELSIF LD THEN
        CV := PV;
    ELSIF NOT BOTH THEN
        IF CU_EDGE THEN
            IF CV < 32767 THEN
                CV := CV + 1;
            END_IF;
        ELSIF CD_EDGE THEN
            IF CV > -32768 THEN
                CV := CV - 1;
            END_IF;
        END_IF;
    END_IF;
    QU := CV >= PV;
    QD := CV <= 0;
END_FUNCTION_BLOCK
