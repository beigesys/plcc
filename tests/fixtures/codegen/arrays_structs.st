PROGRAM ArrayTest
VAR
    arr : ARRAY[0..9] OF INT;
    i : INT;
    sum : INT := 0;
    max_val : INT := 0;
END_VAR
    (* Fill array *)
    FOR i := 0 TO 9 DO
        arr[i] := (i + 1) * 10;
    END_FOR;

    (* Sum and find max *)
    sum := 0;
    max_val := arr[0];
    FOR i := 0 TO 9 DO
        sum := sum + arr[i];
        IF arr[i] > max_val THEN max_val := arr[i]; END_IF;
    END_FOR;
END_PROGRAM
