PROGRAM MultiVarBlocks
VAR_INPUT
    start_cmd : BOOL;
    stop_cmd : BOOL;
END_VAR
VAR_OUTPUT
    running : BOOL;
    error : BOOL;
END_VAR
VAR
    state : INT := 0;
    cycle_count : DINT := 0;
END_VAR
    IF stop_cmd THEN
        running := FALSE;
        state := 0;
    ELSIF start_cmd THEN
        running := TRUE;
        state := 1;
    END_IF;

    IF running THEN
        cycle_count := cycle_count + 1;
    END_IF;
END_PROGRAM
