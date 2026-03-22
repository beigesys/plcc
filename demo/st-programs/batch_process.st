PROGRAM BatchProcess
VAR
    stage : INT := 0;
    timer : INT := 0;
    fill_valve : BOOL := FALSE;
    mixer : BOOL := FALSE;
    drain_valve : BOOL := FALSE;
    batch_count : INT := 0;
    complete : BOOL := FALSE;
END_VAR
    timer := timer + 1;
    CASE stage OF
        0: (* IDLE *)
            fill_valve := FALSE; mixer := FALSE; drain_valve := FALSE;
            stage := 1; timer := 0;
        1: (* FILL *)
            fill_valve := TRUE;
            IF timer >= 20 THEN stage := 2; timer := 0; fill_valve := FALSE; END_IF;
        2: (* MIX *)
            mixer := TRUE;
            IF timer >= 30 THEN stage := 3; timer := 0; mixer := FALSE; END_IF;
        3: (* DRAIN *)
            drain_valve := TRUE;
            IF timer >= 15 THEN
                stage := 0; timer := 0; drain_valve := FALSE;
                batch_count := batch_count + 1;
                complete := TRUE;
            END_IF;
    ELSE
        stage := 0;
    END_CASE;
END_PROGRAM
