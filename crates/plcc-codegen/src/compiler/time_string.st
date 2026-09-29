(* SPDX-License-Identifier: MPL-2.0 *)
(* TIME / date values -> STRING, in the literal syntax CODESYS prints, compiled
   into a module only when it uses them (see compiler/ondemand.rs). Date and
   time-of-day fields are not zero-padded, as in the CODESYS help's examples:
   DATE_TO_STRING(D#1970-1-1) = 'D#1970-1-1', TOD_TO_STRING(TOD#12:0:1) =
   'TOD#12:0:1'; milliseconds are always three digits ('TOD#2:3:4.005'). *)

(* 'T#1d2h3m4s5ms', units that are 0 left out; 'T#0ms' for zero. With NS,
   sub-millisecond units (us, ns) are printed too (LTIME). *)
FUNCTION __PLCC_DURATION_TO_STRING : STRING
VAR_INPUT
    NSV : LINT;
    PREFIX : STRING;
    NS : BOOL;
END_VAR
VAR
    v : LINT;
    s : STRING;
    part : LINT;
END_VAR
s := PREFIX;
v := NSV;
IF v < 0 THEN
    s := CONCAT(s, '-');
    v := -v;
END_IF;
IF NOT NS THEN
    v := v / 1000000;   (* whole milliseconds *)
    IF v = 0 THEN
        __PLCC_DURATION_TO_STRING := CONCAT(s, '0ms');
        RETURN;
    END_IF;
    part := v / 86400000; IF part > 0 THEN s := CONCAT(s, CONCAT(LINT_TO_STRING(part), 'd')); END_IF;
    part := (v / 3600000) MOD 24; IF part > 0 THEN s := CONCAT(s, CONCAT(LINT_TO_STRING(part), 'h')); END_IF;
    part := (v / 60000) MOD 60; IF part > 0 THEN s := CONCAT(s, CONCAT(LINT_TO_STRING(part), 'm')); END_IF;
    part := (v / 1000) MOD 60; IF part > 0 THEN s := CONCAT(s, CONCAT(LINT_TO_STRING(part), 's')); END_IF;
    part := v MOD 1000; IF part > 0 THEN s := CONCAT(s, CONCAT(LINT_TO_STRING(part), 'ms')); END_IF;
ELSE
    IF v = 0 THEN
        __PLCC_DURATION_TO_STRING := CONCAT(s, '0ns');
        RETURN;
    END_IF;
    part := v / 86400000000000; IF part > 0 THEN s := CONCAT(s, CONCAT(LINT_TO_STRING(part), 'd')); END_IF;
    part := (v / 3600000000000) MOD 24; IF part > 0 THEN s := CONCAT(s, CONCAT(LINT_TO_STRING(part), 'h')); END_IF;
    part := (v / 60000000000) MOD 60; IF part > 0 THEN s := CONCAT(s, CONCAT(LINT_TO_STRING(part), 'm')); END_IF;
    part := (v / 1000000000) MOD 60; IF part > 0 THEN s := CONCAT(s, CONCAT(LINT_TO_STRING(part), 's')); END_IF;
    part := (v / 1000000) MOD 1000; IF part > 0 THEN s := CONCAT(s, CONCAT(LINT_TO_STRING(part), 'ms')); END_IF;
    part := (v / 1000) MOD 1000; IF part > 0 THEN s := CONCAT(s, CONCAT(LINT_TO_STRING(part), 'us')); END_IF;
    part := v MOD 1000; IF part > 0 THEN s := CONCAT(s, CONCAT(LINT_TO_STRING(part), 'ns')); END_IF;
END_IF;
__PLCC_DURATION_TO_STRING := s;
END_FUNCTION

(* 'YYYY-MM-DD' of nanoseconds since 1970-01-01 (Howard Hinnant's
   civil_from_days). *)
FUNCTION __PLCC_DATE_PART : STRING
VAR_INPUT NSV : LINT; END_VAR
VAR z : LINT; era : LINT; doe : LINT; yoe : LINT; y : LINT; doy : LINT; mp : LINT; d : LINT; m : LINT; END_VAR
z := NSV / 86400000000000;
IF NSV < 0 AND NSV MOD 86400000000000 <> 0 THEN z := z - 1; END_IF;
z := z + 719468;
IF z >= 0 THEN era := z / 146097; ELSE era := (z - 146096) / 146097; END_IF;
doe := z - era * 146097;
yoe := (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
y := yoe + era * 400;
doy := doe - (365 * yoe + yoe / 4 - yoe / 100);
mp := (5 * doy + 2) / 153;
d := doy - (153 * mp + 2) / 5 + 1;
IF mp < 10 THEN m := mp + 3; ELSE m := mp - 9; END_IF;
IF m <= 2 THEN y := y + 1; END_IF;
__PLCC_DATE_PART := CONCAT(CONCAT(CONCAT(LINT_TO_STRING(y), '-'), CONCAT(LINT_TO_STRING(m), '-')), LINT_TO_STRING(d));
END_FUNCTION

(* 'h:m:s' plus '.mmm' when there are milliseconds. *)
FUNCTION __PLCC_TOD_PART : STRING
VAR_INPUT NSV : LINT; END_VAR
VAR t : LINT; s : STRING; ms : LINT; END_VAR
t := NSV MOD 86400000000000;
IF t < 0 THEN t := t + 86400000000000; END_IF;
t := t / 1000000;
s := CONCAT(CONCAT(LINT_TO_STRING(t / 3600000), ':'), CONCAT(LINT_TO_STRING(t / 60000 MOD 60), ':'));
s := CONCAT(s, LINT_TO_STRING(t / 1000 MOD 60));
ms := t MOD 1000;
IF ms > 0 THEN
    IF ms < 10 THEN
        s := CONCAT(s, '.00');
    ELSIF ms < 100 THEN
        s := CONCAT(s, '.0');
    ELSE
        s := CONCAT(s, '.');
    END_IF;
    s := CONCAT(s, LINT_TO_STRING(ms));
END_IF;
__PLCC_TOD_PART := s;
END_FUNCTION

FUNCTION TIME_TO_STRING : STRING
VAR_INPUT X : TIME; END_VAR
TIME_TO_STRING := __PLCC_DURATION_TO_STRING(LTIME_TO_LINT(TIME_TO_LTIME(X)), 'T#', FALSE);
END_FUNCTION

FUNCTION LTIME_TO_STRING : STRING
VAR_INPUT X : LTIME; END_VAR
LTIME_TO_STRING := __PLCC_DURATION_TO_STRING(LTIME_TO_LINT(X), 'LTIME#', TRUE);
END_FUNCTION

FUNCTION DATE_TO_STRING : STRING
VAR_INPUT X : DATE; END_VAR
DATE_TO_STRING := CONCAT('D#', __PLCC_DATE_PART(LDATE_TO_LINT(DATE_TO_LDATE(X))));
END_FUNCTION

FUNCTION TOD_TO_STRING : STRING
VAR_INPUT X : TOD; END_VAR
TOD_TO_STRING := CONCAT('TOD#', __PLCC_TOD_PART(LTOD_TO_LINT(TOD_TO_LTOD(X))));
END_FUNCTION

FUNCTION DT_TO_STRING : STRING
VAR_INPUT X : DT; END_VAR
VAR n : LINT; END_VAR
n := LDT_TO_LINT(DT_TO_LDT(X));
DT_TO_STRING := CONCAT(CONCAT(CONCAT('DT#', __PLCC_DATE_PART(n)), '-'), __PLCC_TOD_PART(n));
END_FUNCTION
