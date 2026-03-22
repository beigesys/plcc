FUNCTION_BLOCK Accumulator
VAR
    total : INT := 0;
END_VAR
    METHOD Add
    VAR_INPUT val : INT; END_VAR
        total := total + val;
    END_METHOD
    METHOD GetTotal : INT
        GetTotal := total;
    END_METHOD
    METHOD Reset
        total := 0;
    END_METHOD
END_FUNCTION_BLOCK

PROGRAM OOPTest
VAR
    acc : Accumulator;
    result : INT;
END_VAR
    acc.Add(val := 10);
    acc.Add(val := 20);
    acc.Add(val := 30);
    result := acc.GetTotal();
END_PROGRAM
