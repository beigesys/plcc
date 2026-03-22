PROGRAM StdlibMathTest
VAR
    sq : REAL;
    ab : REAL;
    sn : REAL;
    cs : REAL;
    ex : REAL;
    lg : REAL;
    fl : REAL;
    cl : REAL;
    mn : REAL;
    mx : REAL;
    lm : REAL;
END_VAR
    sq := SQRT(25.0);
    ab := ABS(-42.0);
    sn := SIN(0.0);
    cs := COS(0.0);
    ex := EXP(0.0);
    lg := LN(1.0);
    fl := FLOOR(3.7);
    cl := CEIL(3.2);
    mn := MIN(5.0, 3.0);
    mx := MAX(5.0, 3.0);
    lm := LIMIT(0.0, -5.0, 100.0);
END_PROGRAM
