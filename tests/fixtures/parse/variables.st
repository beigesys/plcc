FUNCTION_BLOCK VarTest
VAR_INPUT
    in1 : INT;
    in2 : BOOL;
END_VAR
VAR_OUTPUT
    out1 : INT;
    out2 : REAL;
END_VAR
VAR_IN_OUT
    inout1 : INT;
END_VAR
VAR
    local1 : INT := 0;
    local2 : REAL := 1.0;
END_VAR
VAR_TEMP
    temp1 : INT;
END_VAR
VAR CONSTANT
    PI : REAL := 3.14159;
    MAX_COUNT : INT := 1000;
END_VAR
VAR RETAIN
    persistent_counter : INT := 0;
END_VAR
    out1 := in1 + local1;
END_FUNCTION_BLOCK
