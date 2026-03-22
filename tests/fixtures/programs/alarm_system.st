FUNCTION PriorityEncode : INT
VAR_INPUT critical : INT; high : INT; medium : INT; low : INT; END_VAR
    IF critical <> 0 THEN PriorityEncode := 4;
    ELSIF high <> 0 THEN PriorityEncode := 3;
    ELSIF medium <> 0 THEN PriorityEncode := 2;
    ELSIF low <> 0 THEN PriorityEncode := 1;
    ELSE PriorityEncode := 0;
    END_IF;
END_FUNCTION

FUNCTION_BLOCK AlarmHandler
VAR_INPUT
    temp_alarm : BOOL;
    pressure_alarm : BOOL;
    level_alarm : BOOL;
END_VAR
VAR_OUTPUT
    alarm_active : BOOL;
    alarm_code : INT;
END_VAR
VAR
    temp_val : INT;
    press_val : INT;
    level_val : INT;
END_VAR
    IF temp_alarm THEN temp_val := 1; ELSE temp_val := 0; END_IF;
    IF pressure_alarm THEN press_val := 1; ELSE press_val := 0; END_IF;
    IF level_alarm THEN level_val := 1; ELSE level_val := 0; END_IF;
    alarm_code := PriorityEncode(press_val, temp_val, level_val, 0);
    alarm_active := alarm_code > 0;
END_FUNCTION_BLOCK

PROGRAM AlarmSystem
VAR
    handler : AlarmHandler;
    system_alarm : BOOL;
    system_code : INT;
END_VAR
    handler(temp_alarm := TRUE, pressure_alarm := FALSE, level_alarm := TRUE);
    system_alarm := handler.alarm_active;
    system_code := handler.alarm_code;
END_PROGRAM
