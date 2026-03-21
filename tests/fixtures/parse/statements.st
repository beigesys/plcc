PROGRAM TestStatements
VAR
    x : INT := 0;
    y : INT := 0;
    flag : BOOL := FALSE;
    sum : INT := 0;
    i : INT;
END_VAR
    (* Assignment *)
    x := 42;

    (* IF / ELSIF / ELSE *)
    IF x > 10 THEN
        y := 1;
    ELSIF x > 5 THEN
        y := 2;
    ELSIF x > 0 THEN
        y := 3;
    ELSE
        y := 0;
    END_IF;

    (* CASE *)
    CASE x OF
        0:
            flag := FALSE;
        1, 2, 3:
            flag := TRUE;
        10..20:
            y := 100;
    ELSE
        y := -1;
    END_CASE;

    (* FOR *)
    sum := 0;
    FOR i := 1 TO 100 BY 2 DO
        sum := sum + i;
    END_FOR;

    (* WHILE *)
    WHILE sum > 0 DO
        sum := sum - 1;
    END_WHILE;

    (* REPEAT..UNTIL *)
    REPEAT
        sum := sum + 1;
    UNTIL sum >= 50
    END_REPEAT;

    (* EXIT / CONTINUE *)
    FOR i := 0 TO 100 DO
        IF i = 50 THEN
            EXIT;
        END_IF;
        IF i MOD 2 = 0 THEN
            CONTINUE;
        END_IF;
        sum := sum + i;
    END_FOR;

    (* RETURN *)
    RETURN;
END_PROGRAM
