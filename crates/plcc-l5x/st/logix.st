(* SPDX-License-Identifier: MPL-2.0 *)
(*
 * Logix 5000 instruction prelude for L5X/L5K inputs.
 *
 * Compiled alongside every Rockwell project. Behaviour follows the Logix 5000
 * Controllers General Instructions Reference Manual (1756-RM003, published as
 * 1756-RM018A-EN-P, September 2025); the section each block implements is named
 * next to it. Everything plcc adds is spelled with a double underscore
 * (`lx__...`), which Logix names can never contain.
 *
 * Time: the controller clock is MONOTONIC_NS() (the external
 * `plcc_monotonic_ns` symbol). Timers keep the time of their last update in
 * nanoseconds and add whole milliseconds to .ACC, carrying the remainder, so
 * they do not drift with the scan period ("How a Timer Runs": ACC = ACC +
 * (current_time - last_time_scanned)).
 *)

(* ---- Predefined structures (1756-RM003 "Timer and Counter Instructions") -- *)

TYPE TIMER :
STRUCT
    PRE : DINT;
    ACC : DINT;
    EN : BOOL;
    TT : BOOL;
    DN : BOOL;
    FS : BOOL;
    LS : BOOL;
    OV : BOOL;
    ER : BOOL;
    lx__last : LINT;
END_STRUCT;
END_TYPE

TYPE COUNTER :
STRUCT
    PRE : DINT;
    ACC : DINT;
    CU : BOOL;
    CD : BOOL;
    DN : BOOL;
    OV : BOOL;
    UN : BOOL;
END_STRUCT;
END_TYPE

TYPE CONTROL :
STRUCT
    LEN : DINT;
    POS : DINT;
    EN : BOOL;
    EU : BOOL;
    DN : BOOL;
    EM : BOOL;
    ER : BOOL;
    UL : BOOL;
    IN : BOOL;
    FD : BOOL;
END_STRUCT;
END_TYPE

(* STRING: LEN + 82 characters (1756-RM003 "ASCII String Instructions"). *)
TYPE LX_STRING :
STRUCT
    LEN : DINT;
    DATA : ARRAY[0..81] OF SINT;
END_STRUCT;
END_TYPE

(* FBD_TIMER / FBD_COUNTER / FBD_ONESHOT: the backing tags of the Structured
   Text timer, counter and one-shot instructions (TONR, TOFR, RTOR, CTUD, OSRI,
   OSFI). *)
TYPE FBD_TIMER :
STRUCT
    EnableIn : BOOL := TRUE;
    TimerEnable : BOOL;
    PRE : DINT;
    Reset : BOOL;
    EnableOut : BOOL;
    ACC : DINT;
    EN : BOOL;
    TT : BOOL;
    DN : BOOL;
    Status : DINT;
    InstructFault : BOOL;
    PresetInv : BOOL;
    lx__last : LINT;
    lx__first : BOOL := TRUE;
END_STRUCT;
END_TYPE

TYPE FBD_COUNTER :
STRUCT
    EnableIn : BOOL := TRUE;
    CUEnable : BOOL;
    CDEnable : BOOL;
    PRE : DINT;
    Reset : BOOL;
    EnableOut : BOOL;
    ACC : DINT;
    CU : BOOL;
    CD : BOOL;
    DN : BOOL;
    OV : BOOL;
    UN : BOOL;
    lx__cu : BOOL;
    lx__cd : BOOL;
    lx__first : BOOL := TRUE;
END_STRUCT;
END_TYPE

(* Byte k (0 = least significant) of an integer, and the integer with byte k
   replaced: COP between integer types copies bytes, little-endian. *)
FUNCTION lx__getbyte : LINT
VAR_INPUT v : LINT; k : LINT; END_VAR
    lx__getbyte := LWORD_TO_LINT(SHR(LINT_TO_LWORD(v), LINT_TO_INT(k * 8)) AND LWORD#16#FF);
END_FUNCTION

FUNCTION lx__setbyte : LINT
VAR_INPUT v : LINT; k : LINT; b : LINT; END_VAR
VAR m : LWORD; END_VAR
    m := SHL(LWORD#16#FF, LINT_TO_INT(k * 8));
    lx__setbyte := LWORD_TO_LINT((LINT_TO_LWORD(v) AND NOT m)
        OR SHL(LINT_TO_LWORD(b) AND LWORD#16#FF, LINT_TO_INT(k * 8)));
END_FUNCTION

(* Bit n of v set or cleared (indirect bit writes `tag.[n]`). *)
FUNCTION lx__setbitl : LINT
VAR_INPUT v : LINT; n : LINT; b : BOOL; END_VAR
VAR m : LWORD; END_VAR
    IF n < 0 OR n > 63 THEN
        lx__setbitl := v;
        RETURN;
    END_IF;
    m := SHL(LWORD#1, LINT_TO_INT(n));
    IF b THEN
        lx__setbitl := LWORD_TO_LINT(LINT_TO_LWORD(v) OR m);
    ELSE
        lx__setbitl := LWORD_TO_LINT(LINT_TO_LWORD(v) AND NOT m);
    END_IF;
END_FUNCTION

(* SWPB order modes of a DINT (1756-RM003 "Swap Byte (SWPB)"):
   HIGH/LOW ABCD -> BADC, REVERSE ABCD -> DCBA. *)
FUNCTION lx__swpb_highlow : DINT
VAR_INPUT v : DINT; END_VAR
VAR x : DWORD; END_VAR
    x := DINT_TO_DWORD(v);
    lx__swpb_highlow := DWORD_TO_DINT(SHL(x AND DWORD#16#00FF00FF, 8) OR (SHR(x, 8) AND DWORD#16#00FF00FF));
END_FUNCTION

FUNCTION lx__swpb_reverse : DINT
VAR_INPUT v : DINT; END_VAR
    lx__swpb_reverse := DWORD_TO_DINT(ROL(DINT_TO_DWORD(lx__swpb_highlow(v)), 16));
END_FUNCTION

(* One bit of a DINT set or cleared (bit-array instructions BSL/BSR). *)
FUNCTION lx__setbit_dint : DINT
VAR_INPUT v : DINT; n : LINT; b : BOOL; END_VAR
VAR m : DWORD; END_VAR
    m := SHL(DWORD#1, LINT_TO_INT(n));
    IF b THEN
        lx__setbit_dint := DWORD_TO_DINT(DINT_TO_DWORD(v) OR m);
    ELSE
        lx__setbit_dint := DWORD_TO_DINT(DINT_TO_DWORD(v) AND NOT m);
    END_IF;
END_FUNCTION

TYPE FBD_ONESHOT :
STRUCT
    EnableIn : BOOL := TRUE;
    InputBit : BOOL;
    EnableOut : BOOL;
    OutputBit : BOOL;
    lx__prev : BOOL;
    lx__first : BOOL := TRUE;
END_STRUCT;
END_TYPE

(* ---- Controller status (1756-RM003 "Math status flags") -------------------
   S:FS is TRUE during a program's first scan. S:V, S:Z, S:N and S:C are set by
   the math and move instructions of ladder routines from the value they store;
   an overflow also sets S:MINOR (minor fault type 4, code 4). plcc has no
   fault log: S:MINOR stays set until the program clears it. *)
VAR_GLOBAL
    lx__S_FS : BOOL;
    lx__S_V : BOOL;
    lx__S_Z : BOOL;
    lx__S_N : BOOL;
    lx__S_C : BOOL;
    lx__S_MINOR : BOOL;
    (* Count of minor faults: arithmetic overflow (4/4) and divide by zero. *)
    lx__minor_faults : UDINT;
END_VAR

(* A minor fault that is not an arithmetic overflow (string instructions:
   type 4, codes 51/52/56). *)
FUNCTION lx__minor : BOOL
    lx__S_MINOR := TRUE;
    lx__minor_faults := lx__minor_faults + 1;
    lx__minor := TRUE;
END_FUNCTION

FUNCTION lx__overflow : BOOL
    lx__S_V := TRUE;
    lx__S_MINOR := TRUE;
    lx__minor_faults := lx__minor_faults + 1;
    lx__overflow := TRUE;
END_FUNCTION

FUNCTION lx__inf : REAL
VAR one : REAL := 1.0; zero : REAL := 0.0; END_VAR
    lx__inf := one / zero;
END_FUNCTION

(* REAL → integer: "Fractions = 0.5 round up or down to the nearest even
   number" (1756-RM003 "Data conversions", -2.5 → -2, 3.5 → 4). *)
FUNCTION lx__round : LREAL
VAR_INPUT v : LREAL; END_VAR
VAR t : LREAL; f : LREAL; half : LREAL; END_VAR
    t := TRUNC_LREAL(v);
    f := v - t;
    IF f > 0.5 THEN
        t := t + 1.0;
    ELSIF f < -0.5 THEN
        t := t - 1.0;
    ELSIF f = 0.5 OR f = -0.5 THEN
        half := t / 2.0;
        IF TRUNC_LREAL(half) <> half THEN
            (* t is odd: move away from it, towards the even neighbour *)
            IF f > 0.0 THEN t := t + 1.0; ELSE t := t - 1.0; END_IF;
        END_IF;
    END_IF;
    lx__round := t;
END_FUNCTION

(* Truncation toward zero of an LREAL within the LINT range. (IEC TRUNC
   returns a DINT in plcc; LREAL_TO_LINT rounds.) *)
FUNCTION lx__trunc_lint : LINT
VAR_INPUT v : LREAL; END_VAR
VAR t : LINT; END_VAR
    t := LREAL_TO_LINT(v);
    IF v >= 0.0 AND LINT_TO_LREAL(t) > v THEN
        t := t - 1;
    ELSIF v < 0.0 AND LINT_TO_LREAL(t) < v THEN
        t := t + 1;
    END_IF;
    lx__trunc_lint := t;
END_FUNCTION

(* Truncation toward zero, as an LREAL (no integer range limit). *)
FUNCTION TRUNC_LREAL : LREAL
VAR_INPUT v : LREAL; END_VAR
    IF v >= 9.2233720368547758E18 OR v <= -9.2233720368547758E18 OR v <> v THEN
        TRUNC_LREAL := v;
    ELSE
        TRUNC_LREAL := LINT_TO_LREAL(lx__trunc_lint(v));
    END_IF;
END_FUNCTION

(* An LREAL already rounded to an integer, as LINT; out of range sets S:V. *)
FUNCTION lx__r2l : LINT
VAR_INPUT v : LREAL; END_VAR
    IF v <> v THEN
        lx__overflow();
        lx__r2l := 0;
    ELSIF v >= 9.2233720368547758E18 THEN
        lx__overflow();
        lx__r2l := 9223372036854775807;
    ELSIF v < -9.2233720368547758E18 THEN
        lx__overflow();
        lx__r2l := -9223372036854775807 - 1;
    ELSE
        lx__r2l := lx__trunc_lint(v);
    END_IF;
END_FUNCTION

(* Integer DIV: truncated; "If SourceB is zero the result will be SourceA for
   integer divides" and a minor fault 4/4 (1756-RM003 DIV, footnote 1). *)
FUNCTION lx__div_i : LINT
VAR_INPUT a : LINT; b : LINT; END_VAR
    IF b = 0 THEN
        lx__overflow();
        lx__div_i := a;
    ELSIF b = -1 THEN
        lx__div_i := 0 - a;
    ELSE
        lx__div_i := a / b;
    END_IF;
END_FUNCTION

(* Floating DIV: IEEE (x/0 is ±1.$, i.e. infinity), divide by zero also sets
   S:V and a minor fault. *)
FUNCTION lx__div_r : LREAL
VAR_INPUT a : LREAL; b : LREAL; END_VAR
    IF b = 0.0 THEN lx__overflow(); END_IF;
    lx__div_r := a / b;
END_FUNCTION

FUNCTION lx__div_f : REAL
VAR_INPUT a : REAL; b : REAL; END_VAR
    IF b = 0.0 THEN lx__overflow(); END_IF;
    lx__div_f := a / b;
END_FUNCTION

(* MOD: Dest = A - (truncate(A / B) * B); "If Source B is 0, the result is 0
   and a minor fault is generated" (1756-RM003 MOD). *)
FUNCTION lx__mod_i : LINT
VAR_INPUT a : LINT; b : LINT; END_VAR
    IF b = 0 THEN
        lx__overflow();
        lx__mod_i := 0;
    ELSIF b = -1 THEN
        lx__mod_i := 0;
    ELSE
        lx__mod_i := a - (a / b) * b;
    END_IF;
END_FUNCTION

FUNCTION lx__mod_r : LREAL
VAR_INPUT a : LREAL; b : LREAL; END_VAR
    IF b = 0.0 THEN
        lx__overflow();
        lx__mod_r := 0.0;
    ELSE
        lx__mod_r := a - TRUNC_LREAL(a / b) * b;
    END_IF;
END_FUNCTION

FUNCTION lx__mod_f : REAL
VAR_INPUT a : REAL; b : REAL; END_VAR
    lx__mod_f := LREAL_TO_REAL(lx__mod_r(a, b));
END_FUNCTION

FUNCTION lx__pow : LREAL
VAR_INPUT a : LREAL; b : LREAL; END_VAR
    lx__pow := a ** b;
END_FUNCTION

(* TRN (1756-RM003 "Truncate"): drop the fraction, keep the type. *)
FUNCTION lx__trn : LREAL
VAR_INPUT v : LREAL; END_VAR
    lx__trn := TRUNC_LREAL(v);
END_FUNCTION

FUNCTION lx__deg : LREAL
VAR_INPUT v : LREAL; END_VAR
    lx__deg := v * 57.295779513082320876798154814105;
END_FUNCTION

FUNCTION lx__rad : LREAL
VAR_INPUT v : LREAL; END_VAR
    lx__rad := v * 0.017453292519943295769236907684886;
END_FUNCTION

(* FRD: BCD → integer; TOD: integer → BCD (1756-RM003 "Math Conversion
   Instructions", BCD_TO / TO_BCD). A value outside 0..99_999_999 sets S:V. *)
FUNCTION lx__frd : LINT
VAR_INPUT v : LINT; END_VAR
VAR r : LINT; m : LINT := 1; d : LINT; x : LINT; i : INT; END_VAR
    x := v;
    FOR i := 1 TO 8 DO
        d := x AND 16#F;
        IF d > 9 THEN lx__overflow(); END_IF;
        r := r + d * m;
        m := m * 10;
        x := x / 16;
    END_FOR;
    lx__frd := r;
END_FUNCTION

FUNCTION lx__tod : LINT
VAR_INPUT v : LINT; END_VAR
VAR r : LINT; s : LINT := 0; x : LINT; i : INT; END_VAR
    IF v < 0 OR v > 99999999 THEN
        lx__overflow();
    END_IF;
    x := v;
    FOR i := 0 TO 7 DO
        r := r + ((x MOD 10) * lx__shl(1, s));
        s := s + 4;
        x := x / 10;
    END_FOR;
    lx__tod := r;
END_FUNCTION

FUNCTION lx__shl : LINT
VAR_INPUT v : LINT; n : LINT; END_VAR
    lx__shl := LWORD_TO_LINT(SHL(LINT_TO_LWORD(v), LINT_TO_INT(n)));
END_FUNCTION

(* Bit n of v (indirect bit addressing `tag.[n]`, 1756-RM003 "Bit Addressing"). *)
FUNCTION lx__getbit : BOOL
VAR_INPUT v : LINT; n : LINT; END_VAR
    IF n < 0 OR n > 63 THEN
        lx__getbit := FALSE;
    ELSE
        lx__getbit := (SHR(LINT_TO_LWORD(v), LINT_TO_INT(n)) AND LWORD#1) <> LWORD#0;
    END_IF;
END_FUNCTION

(* ---- Timers (1756-RM003 TON, TOF, RTO: execution tables and flow charts) -- *)

(* Elapsed whole milliseconds since t.lx__last; advances lx__last by them. *)
FUNCTION lx__elapsed_ms : LINT
VAR_IN_OUT last : LINT; END_VAR
VAR now : LINT; d : LINT; END_VAR
    now := MONOTONIC_NS();
    d := (now - last) / 1000000;
    IF d < 0 THEN d := 0; END_IF;
    last := last + d * 1000000;
    lx__elapsed_ms := d;
END_FUNCTION

(* The accumulate step shared by TON/RTO (true) and TOF (false): ACC +=
   elapsed; on overflow ACC := 2147483647; returns whether the timer is done. *)
FUNCTION lx__accumulate : BOOL
VAR_IN_OUT t : TIMER; END_VAR
VAR acc : LINT; END_VAR
    acc := DINT_TO_LINT(t.ACC) + lx__elapsed_ms(t.lx__last);
    IF acc > 2147483647 THEN
        t.ACC := 2147483647;
        lx__accumulate := TRUE;
    ELSE
        t.ACC := LINT_TO_DINT(acc);
        lx__accumulate := t.ACC >= t.PRE;
    END_IF;
END_FUNCTION

(* TON. False: EN, TT, DN cleared, ACC := 0. True: TT := 1; if DN, TT := 0;
   else if EN was 0, start timing (last := now); else accumulate, and when
   ACC >= PRE (or on overflow) TT := 0, DN := 1. Then EN := 1. ACC is not
   clamped to PRE. *)
FUNCTION lx__ton : BOOL
VAR_IN_OUT t : TIMER; END_VAR
VAR_INPUT rc : BOOL; END_VAR
    IF rc THEN
        t.TT := TRUE;
        IF t.DN THEN
            t.TT := FALSE;
        ELSIF NOT t.EN THEN
            t.lx__last := MONOTONIC_NS();
        ELSIF lx__accumulate(t) THEN
            t.TT := FALSE;
            t.DN := TRUE;
        END_IF;
        t.EN := TRUE;
    ELSE
        t.EN := FALSE;
        t.TT := FALSE;
        t.DN := FALSE;
        t.ACC := 0;
    END_IF;
    lx__ton := rc;
END_FUNCTION

(* RTO: as TON while true; false clears EN and TT and keeps ACC and DN. *)
FUNCTION lx__rto : BOOL
VAR_IN_OUT t : TIMER; END_VAR
VAR_INPUT rc : BOOL; END_VAR
    IF rc THEN
        t.TT := TRUE;
        IF t.DN THEN
            t.TT := FALSE;
        ELSIF NOT t.EN THEN
            t.lx__last := MONOTONIC_NS();
        ELSIF lx__accumulate(t) THEN
            t.TT := FALSE;
            t.DN := TRUE;
        END_IF;
        t.EN := TRUE;
    ELSE
        t.EN := FALSE;
        t.TT := FALSE;
    END_IF;
    lx__rto := rc;
END_FUNCTION

(* TOF. True: EN := 1, TT := 0, DN := 1, ACC := 0. False: TT := 1; if DN is 0,
   TT := 0; else if EN was 1, start timing; else accumulate, and when done
   TT := 0, DN := 0. Then EN := 0. *)
FUNCTION lx__tof : BOOL
VAR_IN_OUT t : TIMER; END_VAR
VAR_INPUT rc : BOOL; END_VAR
    IF rc THEN
        t.EN := TRUE;
        t.TT := FALSE;
        t.DN := TRUE;
        t.ACC := 0;
    ELSE
        t.TT := TRUE;
        IF NOT t.DN THEN
            t.TT := FALSE;
        ELSIF t.EN THEN
            t.lx__last := MONOTONIC_NS();
        ELSIF lx__accumulate(t) THEN
            t.TT := FALSE;
            t.DN := FALSE;
        END_IF;
        t.EN := FALSE;
    END_IF;
    lx__tof := rc;
END_FUNCTION

(* ---- Counters (1756-RM003 CTU, CTD flow charts) --------------------------- *)

FUNCTION lx__cdone
VAR_IN_OUT c : COUNTER; END_VAR
    IF NOT c.OV AND NOT c.UN THEN
        c.DN := c.ACC >= c.PRE;
    END_IF;
END_FUNCTION

(* CTU. True with CU = 0: CU := 1, ACC := ACC + 1; past 2,147,483,647 the
   ACC wraps to -2,147,483,648 and OV is set (or, if UN was set, both OV and UN
   clear). DN := ACC >= PRE while neither OV nor UN is set. False: CU := 0. *)
FUNCTION lx__ctu : BOOL
VAR_IN_OUT c : COUNTER; END_VAR
VAR_INPUT rc : BOOL; END_VAR
    IF rc THEN
        IF NOT c.CU THEN
            c.CU := TRUE;
            IF c.ACC = 2147483647 THEN
                c.ACC := -2147483647 - 1;
                IF NOT c.UN THEN
                    c.OV := TRUE;
                    lx__ctu := rc;
                    RETURN;
                END_IF;
                c.UN := FALSE;
                c.OV := FALSE;
            ELSE
                c.ACC := c.ACC + 1;
            END_IF;
        END_IF;
    ELSE
        c.CU := FALSE;
    END_IF;
    lx__cdone(c);
    lx__ctu := rc;
END_FUNCTION

FUNCTION lx__ctd : BOOL
VAR_IN_OUT c : COUNTER; END_VAR
VAR_INPUT rc : BOOL; END_VAR
    IF rc THEN
        IF NOT c.CD THEN
            c.CD := TRUE;
            IF c.ACC = -2147483647 - 1 THEN
                c.ACC := 2147483647;
                IF NOT c.OV THEN
                    c.UN := TRUE;
                    lx__ctd := rc;
                    RETURN;
                END_IF;
                c.UN := FALSE;
                c.OV := FALSE;
            ELSE
                c.ACC := c.ACC - 1;
            END_IF;
        END_IF;
    ELSE
        c.CD := FALSE;
    END_IF;
    lx__cdone(c);
    lx__ctd := rc;
END_FUNCTION

(* RES (1756-RM003 "Reset"): TIMER/COUNTER .ACC := 0, CONTROL .POS := 0, and
   the status bits cleared. *)
FUNCTION lx__res_timer
VAR_IN_OUT t : TIMER; END_VAR
    t.ACC := 0;
    t.EN := FALSE; t.TT := FALSE; t.DN := FALSE;
    t.FS := FALSE; t.LS := FALSE; t.OV := FALSE; t.ER := FALSE;
END_FUNCTION

FUNCTION lx__res_counter
VAR_IN_OUT c : COUNTER; END_VAR
    c.ACC := 0;
    c.CU := FALSE; c.CD := FALSE; c.DN := FALSE; c.OV := FALSE; c.UN := FALSE;
END_FUNCTION

FUNCTION lx__res_control
VAR_IN_OUT c : CONTROL; END_VAR
    c.POS := 0;
    c.EN := FALSE; c.EU := FALSE; c.DN := FALSE; c.EM := FALSE;
    c.ER := FALSE; c.UL := FALSE; c.IN := FALSE; c.FD := FALSE;
END_FUNCTION

(* ---- Structured Text timer/counter/one-shot instructions ----------------- *)

(* TONR (1756-RM003): Reset clears EN, TT, DN and ACC; otherwise a TON on
   TimerEnable. Invalid PRE (< 0) sets PresetInv and does not time. *)
FUNCTION TONR
VAR_IN_OUT t : FBD_TIMER; END_VAR
VAR tm : TIMER; END_VAR
    t.EnableOut := t.EnableIn;
    IF NOT t.EnableIn THEN RETURN; END_IF;
    IF t.PRE < 0 THEN
        t.PresetInv := TRUE; t.InstructFault := TRUE; t.Status := 3;
        RETURN;
    END_IF;
    t.PresetInv := FALSE; t.InstructFault := FALSE; t.Status := 0;
    IF t.Reset THEN
        t.EN := FALSE; t.TT := FALSE; t.DN := FALSE; t.ACC := 0;
        RETURN;
    END_IF;
    tm.PRE := t.PRE; tm.ACC := t.ACC; tm.EN := t.EN; tm.TT := t.TT; tm.DN := t.DN;
    tm.lx__last := t.lx__last;
    lx__ton(tm, t.TimerEnable);
    t.ACC := tm.ACC; t.EN := tm.EN; t.TT := tm.TT; t.DN := tm.DN; t.lx__last := tm.lx__last;
END_FUNCTION

(* TOFR: Reset clears EN, TT, DN and sets ACC := PRE. *)
FUNCTION TOFR
VAR_IN_OUT t : FBD_TIMER; END_VAR
VAR tm : TIMER; END_VAR
    t.EnableOut := t.EnableIn;
    IF NOT t.EnableIn THEN RETURN; END_IF;
    IF t.PRE < 0 THEN
        t.PresetInv := TRUE; t.InstructFault := TRUE; t.Status := 3;
        RETURN;
    END_IF;
    t.PresetInv := FALSE; t.InstructFault := FALSE; t.Status := 0;
    IF t.lx__first THEN
        t.lx__first := FALSE;
        t.EN := FALSE; t.TT := FALSE; t.DN := FALSE;
    END_IF;
    IF t.Reset THEN
        t.EN := FALSE; t.TT := FALSE; t.DN := FALSE; t.ACC := t.PRE;
        RETURN;
    END_IF;
    tm.PRE := t.PRE; tm.ACC := t.ACC; tm.EN := t.EN; tm.TT := t.TT; tm.DN := t.DN;
    tm.lx__last := t.lx__last;
    lx__tof(tm, t.TimerEnable);
    t.ACC := tm.ACC; t.EN := tm.EN; t.TT := tm.TT; t.DN := tm.DN; t.lx__last := tm.lx__last;
END_FUNCTION

(* RTOR: Reset clears EN, TT, DN and ACC; otherwise an RTO on TimerEnable. *)
FUNCTION RTOR
VAR_IN_OUT t : FBD_TIMER; END_VAR
VAR tm : TIMER; END_VAR
    t.EnableOut := t.EnableIn;
    IF NOT t.EnableIn THEN RETURN; END_IF;
    IF t.PRE < 0 THEN
        t.PresetInv := TRUE; t.InstructFault := TRUE; t.Status := 3;
        RETURN;
    END_IF;
    t.PresetInv := FALSE; t.InstructFault := FALSE; t.Status := 0;
    IF t.Reset THEN
        t.EN := FALSE; t.TT := FALSE; t.DN := FALSE; t.ACC := 0;
        RETURN;
    END_IF;
    tm.PRE := t.PRE; tm.ACC := t.ACC; tm.EN := t.EN; tm.TT := t.TT; tm.DN := t.DN;
    tm.lx__last := t.lx__last;
    lx__rto(tm, t.TimerEnable);
    t.ACC := tm.ACC; t.EN := tm.EN; t.TT := tm.TT; t.DN := tm.DN; t.lx__last := tm.lx__last;
END_FUNCTION

(* CTUD (1756-RM003): counts up on a CUEnable rising edge, then down on a
   CDEnable rising edge; first run requires a 0→1 transition; Reset clears
   the history, CU, CD, DN, OV, UN and ACC. *)
FUNCTION CTUD
VAR_IN_OUT c : FBD_COUNTER; END_VAR
    c.EnableOut := c.EnableIn;
    IF NOT c.EnableIn THEN RETURN; END_IF;
    IF c.lx__first THEN
        c.lx__first := FALSE;
        c.lx__cu := TRUE;
        c.lx__cd := TRUE;
    END_IF;
    IF c.Reset THEN
        c.lx__cu := FALSE; c.lx__cd := FALSE;
        c.CU := FALSE; c.CD := FALSE; c.DN := FALSE; c.OV := FALSE; c.UN := FALSE;
        c.ACC := 0;
        RETURN;
    END_IF;
    c.CU := c.CUEnable;
    c.CD := c.CDEnable;
    IF c.CUEnable AND NOT c.lx__cu THEN
        IF c.ACC = 2147483647 THEN
            c.ACC := -2147483647 - 1;
            c.OV := TRUE;
        ELSE
            c.ACC := c.ACC + 1;
        END_IF;
    END_IF;
    IF c.CDEnable AND NOT c.lx__cd THEN
        IF c.ACC = -2147483647 - 1 THEN
            c.ACC := 2147483647;
            c.UN := TRUE;
        ELSE
            c.ACC := c.ACC - 1;
        END_IF;
    END_IF;
    c.lx__cu := c.CUEnable;
    c.lx__cd := c.CDEnable;
    c.DN := c.ACC >= c.PRE;
END_FUNCTION

(* OSRI / OSFI (1756-RM003): OutputBit is set for one execution when InputBit
   goes 0→1 (OSRI) or 1→0 (OSFI); first run needs a transition. *)
FUNCTION OSRI
VAR_IN_OUT o : FBD_ONESHOT; END_VAR
    o.EnableOut := o.EnableIn;
    IF NOT o.EnableIn THEN RETURN; END_IF;
    IF o.lx__first THEN o.lx__first := FALSE; o.lx__prev := TRUE; END_IF;
    o.OutputBit := o.InputBit AND NOT o.lx__prev;
    o.lx__prev := o.InputBit;
END_FUNCTION

FUNCTION OSFI
VAR_IN_OUT o : FBD_ONESHOT; END_VAR
    o.EnableOut := o.EnableIn;
    IF NOT o.EnableIn THEN RETURN; END_IF;
    IF o.lx__first THEN o.lx__first := FALSE; o.lx__prev := FALSE; END_IF;
    o.OutputBit := o.lx__prev AND NOT o.InputBit;
    o.lx__prev := o.InputBit;
END_FUNCTION
