PROGRAM TimeTest
VAR
    t1 : TIME := T#100ms;
    t2 : TIME := T#50ms;
    sum_t : TIME;
    diff_t : TIME;
    scaled_t : TIME;
END_VAR
    sum_t := ADD_TIME(t1, t2);
    diff_t := SUB_TIME(t1, t2);
    scaled_t := MUL_TIME(t1, 3);
END_PROGRAM
