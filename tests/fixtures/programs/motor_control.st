FUNCTION_BLOCK MotorController
VAR_INPUT
    start_cmd : BOOL;
    stop_cmd : BOOL;
    current : REAL;
    overload_limit : REAL := 15.0;
END_VAR
VAR_OUTPUT
    running : BOOL;
    fault : BOOL;
    run_time : INT;
END_VAR
VAR
    overload_count : INT := 0;
END_VAR
    IF stop_cmd OR fault THEN
        running := FALSE;
    ELSIF start_cmd AND NOT fault THEN
        running := TRUE;
    END_IF;

    IF running THEN
        run_time := run_time + 1;
        IF current > overload_limit THEN
            overload_count := overload_count + 1;
            IF overload_count >= 3 THEN
                fault := TRUE;
                running := FALSE;
            END_IF;
        ELSE
            overload_count := 0;
        END_IF;
    END_IF;
END_FUNCTION_BLOCK

PROGRAM MotorSystem
VAR
    motor1 : MotorController;
    motor2 : MotorController;
    any_fault : BOOL;
END_VAR
    motor1(start_cmd := TRUE, stop_cmd := FALSE, current := 10.0);
    motor2(start_cmd := TRUE, stop_cmd := FALSE, current := 10.0);
    any_fault := motor1.fault OR motor2.fault;
END_PROGRAM
