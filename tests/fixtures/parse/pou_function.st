FUNCTION ABS_INT : INT
VAR_INPUT
    value : INT;
END_VAR
    IF value < 0 THEN
        ABS_INT := -value;
    ELSE
        ABS_INT := value;
    END_IF;
END_FUNCTION

FUNCTION Clamp : REAL
VAR_INPUT
    value : REAL;
    low : REAL;
    high : REAL;
END_VAR
    IF value < low THEN
        Clamp := low;
    ELSIF value > high THEN
        Clamp := high;
    ELSE
        Clamp := value;
    END_IF;
END_FUNCTION
