FUNCTION_BLOCK Counter
VAR_INPUT reset : BOOL; END_VAR
VAR_OUTPUT count : INT; END_VAR
VAR cnt : INT := 0; END_VAR
    IF reset THEN cnt := 0; ELSE cnt := cnt + 1; END_IF;
    count := cnt;
END_FUNCTION_BLOCK

FUNCTION_BLOCK Doubler
VAR_INPUT val : INT; END_VAR
VAR_OUTPUT result : INT; END_VAR
    result := val + val;
END_FUNCTION_BLOCK

PROGRAM FBTest
VAR
    c1 : Counter;
    c2 : Counter;
    d : Doubler;
    sum : INT;
    doubled : INT;
END_VAR
    c1(reset := FALSE);
    c2(reset := FALSE);
    d(val := c1.count);
    sum := c1.count + c2.count;
    doubled := d.result;
END_PROGRAM
