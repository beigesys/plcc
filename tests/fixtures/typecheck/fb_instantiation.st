FUNCTION_BLOCK Counter
VAR_INPUT
    reset : BOOL;
END_VAR
VAR_OUTPUT
    count : INT;
END_VAR
VAR
    cnt : INT := 0;
END_VAR
    IF reset THEN
        cnt := 0;
    ELSE
        cnt := cnt + 1;
    END_IF;
    count := cnt;
END_FUNCTION_BLOCK

PROGRAM UseCounter
VAR
    my_counter : Counter;
    result : INT;
END_VAR
    my_counter(reset := FALSE);
    result := my_counter.count;
END_PROGRAM
