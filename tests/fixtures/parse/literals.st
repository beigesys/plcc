PROGRAM TestLiterals
VAR
    b : BOOL := TRUE;
    i : INT := 42;
    hex : DINT := 16#FF;
    bin : BYTE := 2#10101010;
    oct : INT := 8#77;
    r : REAL := 3.14;
    s : STRING := 'Hello, World!';
    neg : INT := -1;
    big : LINT := 1_000_000;
    typed_int : INT := INT#100;
    typed_real : REAL := REAL#2.718;
    typed_bool : BOOL := BOOL#TRUE;
END_VAR
END_PROGRAM
