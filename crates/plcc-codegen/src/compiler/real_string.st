(* SPDX-License-Identifier: MPL-2.0 *)
(* REAL/LREAL <-> STRING, compiled into a module only when it uses them
   (see compiler/ondemand.rs). *)

FUNCTION __PLCC_FLOAT_TO_STRING : STRING
VAR_INPUT
    X : LREAL;
    DIGITS : INT;   (* significant digits: 7 for REAL, 15 for LREAL *)
END_VAR
VAR
    ax : LREAL;
    e : DINT;
    m : LINT;
    lim : LINT;
    ds : STRING;
    nd : INT;
    k : INT;
    s : STRING;
    ex : DINT;
END_VAR
IF X <> X THEN
    __PLCC_FLOAT_TO_STRING := 'NaN';
    RETURN;
END_IF;
ax := ABS(X);
IF ax > LREAL#1.7976931348623157E308 THEN
    IF X < 0.0 THEN
        __PLCC_FLOAT_TO_STRING := '-INF';
    ELSE
        __PLCC_FLOAT_TO_STRING := 'INF';
    END_IF;
    RETURN;
END_IF;
IF ax = 0.0 THEN
    __PLCC_FLOAT_TO_STRING := '0';
    RETURN;
END_IF;
(* ax = m * 10^(e - DIGITS + 1), with m holding exactly DIGITS digits. *)
e := TRUNC(FLOOR(LOG(ax)));
m := LREAL_TO_LINT(ax * EXPT(LREAL#10.0, DIGITS - 1 - e));
lim := LREAL_TO_LINT(EXPT(LREAL#10.0, DIGITS));
IF m >= lim THEN
    m := m / 10;
    e := e + 1;
ELSIF m < lim / 10 THEN
    m := m * 10;
    e := e - 1;
END_IF;
ds := LINT_TO_STRING(m);
(* Significant digits without trailing zeros. *)
nd := DIGITS;
WHILE nd > 1 AND MID(ds, 1, nd) = '0' DO
    nd := nd - 1;
END_WHILE;
IF X < 0.0 THEN
    s := '-';
END_IF;
IF e >= -5 AND e < DIGITS THEN
    IF e >= 0 THEN
        (* Integer part: the first e+1 digits, padded with zeros. *)
        FOR k := 1 TO DINT_TO_INT(e) + 1 DO
            IF k <= nd THEN
                s := CONCAT(s, MID(ds, 1, k));
            ELSE
                s := CONCAT(s, '0');
            END_IF;
        END_FOR;
        IF nd > e + 1 THEN
            s := CONCAT(s, '.');
            s := CONCAT(s, MID(ds, nd - DINT_TO_INT(e) - 1, DINT_TO_INT(e) + 2));
        END_IF;
    ELSE
        s := CONCAT(s, '0.');
        FOR k := 1 TO DINT_TO_INT(-e) - 1 DO
            s := CONCAT(s, '0');
        END_FOR;
        s := CONCAT(s, LEFT(ds, nd));
    END_IF;
ELSE
    s := CONCAT(s, LEFT(ds, 1));
    IF nd > 1 THEN
        s := CONCAT(s, '.');
        s := CONCAT(s, MID(ds, nd - 1, 2));
    END_IF;
    IF e < 0 THEN
        s := CONCAT(s, 'E-');
        ex := -e;
    ELSE
        s := CONCAT(s, 'E+');
        ex := e;
    END_IF;
    IF ex < 10 THEN
        s := CONCAT(s, '0');
    END_IF;
    s := CONCAT(s, DINT_TO_STRING(ex));
END_IF;
__PLCC_FLOAT_TO_STRING := s;
END_FUNCTION

FUNCTION REAL_TO_STRING : STRING
VAR_INPUT X : REAL; END_VAR
REAL_TO_STRING := __PLCC_FLOAT_TO_STRING(REAL_TO_LREAL(X), 7);
END_FUNCTION

FUNCTION LREAL_TO_STRING : STRING
VAR_INPUT X : LREAL; END_VAR
LREAL_TO_STRING := __PLCC_FLOAT_TO_STRING(X, 15);
END_FUNCTION

FUNCTION STRING_TO_LREAL : LREAL
VAR_INPUT S : STRING; END_VAR
VAR
    pt : POINTER TO BYTE;
    v : LREAL;
    neg : BOOL;
    scale : DINT;
    e : DINT;
    eneg : BOOL;
END_VAR
pt := ADR(S);
WHILE pt^ = 32 DO
    pt := pt + 1;
END_WHILE;
IF pt^ = 45 THEN
    neg := TRUE;
    pt := pt + 1;
ELSIF pt^ = 43 THEN
    pt := pt + 1;
END_IF;
WHILE pt^ >= 48 AND pt^ <= 57 DO
    v := v * 10.0 + BYTE_TO_LREAL(pt^ - 48);
    pt := pt + 1;
END_WHILE;
IF pt^ = 46 THEN
    pt := pt + 1;
    WHILE pt^ >= 48 AND pt^ <= 57 DO
        v := v * 10.0 + BYTE_TO_LREAL(pt^ - 48);
        scale := scale - 1;
        pt := pt + 1;
    END_WHILE;
END_IF;
IF pt^ = 69 OR pt^ = 101 THEN
    pt := pt + 1;
    IF pt^ = 45 THEN
        eneg := TRUE;
        pt := pt + 1;
    ELSIF pt^ = 43 THEN
        pt := pt + 1;
    END_IF;
    WHILE pt^ >= 48 AND pt^ <= 57 DO
        e := e * 10 + BYTE_TO_DINT(pt^ - 48);
        pt := pt + 1;
    END_WHILE;
    IF eneg THEN
        e := -e;
    END_IF;
END_IF;
scale := scale + e;
IF scale <> 0 THEN
    v := v * EXPT(LREAL#10.0, scale);
END_IF;
IF neg THEN
    v := -v;
END_IF;
STRING_TO_LREAL := v;
END_FUNCTION

FUNCTION STRING_TO_REAL : REAL
VAR_INPUT S : STRING; END_VAR
STRING_TO_REAL := LREAL_TO_REAL(STRING_TO_LREAL(S));
END_FUNCTION
