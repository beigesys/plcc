PROGRAM StateMachine
VAR
    state : INT := 0;
    output_a : BOOL := FALSE;
    output_b : BOOL := FALSE;
    input_trigger : BOOL := FALSE;
    counter : INT := 0;
END_VAR
    CASE state OF
        0:
            output_a := FALSE;
            output_b := FALSE;
            IF input_trigger THEN
                state := 1;
            END_IF;
        1:
            output_a := TRUE;
            output_b := FALSE;
            counter := counter + 1;
            IF counter >= 10 THEN
                state := 2;
                counter := 0;
            END_IF;
        2:
            output_a := FALSE;
            output_b := TRUE;
            counter := counter + 1;
            IF counter >= 5 THEN
                state := 0;
                counter := 0;
            END_IF;
    ELSE
        state := 0;
    END_CASE;
END_PROGRAM
