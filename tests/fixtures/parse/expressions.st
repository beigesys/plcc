PROGRAM TestExpressions
VAR
    a : INT := 0;
    b : INT := 0;
    c : INT := 0;
    d : REAL := 0.0;
    flag : BOOL := FALSE;
END_VAR
    (* Arithmetic *)
    a := 1 + 2;
    a := 3 - 1;
    a := 2 * 3;
    a := 10 / 2;
    a := 10 MOD 3;
    d := 2.0 ** 3.0;

    (* Comparison *)
    flag := a = b;
    flag := a <> b;
    flag := a < b;
    flag := a <= b;
    flag := a > b;
    flag := a >= b;

    (* Logical *)
    flag := TRUE AND FALSE;
    flag := TRUE OR FALSE;
    flag := TRUE XOR FALSE;
    flag := NOT flag;
    flag := TRUE & FALSE;

    (* Precedence *)
    a := 1 + 2 * 3;
    a := (1 + 2) * 3;

    (* Nested *)
    a := (a + b) * (c - 1);
END_PROGRAM
