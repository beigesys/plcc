PROGRAM ControlFlow
VAR
    state : INT := 0;
    output : INT := 0;
    i : INT;
    sum : INT := 0;
END_VAR
    IF state = 0 THEN
        output := 1;
    ELSIF state = 1 THEN
        output := 2;
    ELSE
        output := 3;
    END_IF;

    sum := 0;
    FOR i := 1 TO 10 DO
        sum := sum + i;
    END_FOR;

    WHILE sum > 50 DO
        sum := sum - 10;
    END_WHILE;

    CASE state OF
        0: output := 100;
        1: output := 200;
    ELSE
        output := 0;
    END_CASE;
END_PROGRAM
