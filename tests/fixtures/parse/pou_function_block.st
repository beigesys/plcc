FUNCTION_BLOCK TON_Simple
VAR_INPUT
    IN_SIGNAL : BOOL;
    PT : INT;
END_VAR
VAR_OUTPUT
    Q : BOOL;
    ET : INT;
END_VAR
VAR
    running : BOOL := FALSE;
    elapsed : INT := 0;
END_VAR
    IF IN_SIGNAL THEN
        IF NOT running THEN
            running := TRUE;
            elapsed := 0;
        END_IF;
        IF running AND NOT Q THEN
            elapsed := elapsed + 1;
            IF elapsed >= PT THEN
                Q := TRUE;
                ET := PT;
            ELSE
                ET := elapsed;
            END_IF;
        END_IF;
    ELSE
        running := FALSE;
        Q := FALSE;
        ET := 0;
        elapsed := 0;
    END_IF;
END_FUNCTION_BLOCK
