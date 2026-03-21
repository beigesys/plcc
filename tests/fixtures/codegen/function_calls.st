FUNCTION AddTwo : INT
VAR_INPUT
    a : INT;
    b : INT;
END_VAR
    AddTwo := a + b;
END_FUNCTION

FUNCTION Square : INT
VAR_INPUT
    x : INT;
END_VAR
    Square := x * x;
END_FUNCTION

PROGRAM CallTest
VAR
    result1 : INT;
    result2 : INT;
END_VAR
    result1 := AddTwo(3, 4);
    result2 := Square(5);
END_PROGRAM
