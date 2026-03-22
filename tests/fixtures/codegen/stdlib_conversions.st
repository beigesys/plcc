PROGRAM ConversionTest
VAR
    i : INT := 42;
    r : REAL;
    d : DINT;
    b : INT;
END_VAR
    r := INT_TO_REAL(i);
    d := INT_TO_DINT(i);
    b := BOOL_TO_INT(TRUE);
END_PROGRAM
