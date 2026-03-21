PROGRAM ImplicitConv
VAR
    s : SINT := 1;
    i : INT := 2;
    d : DINT := 3;
    r : REAL := 1.0;
    lr : LREAL := 2.0;
END_VAR
    i := s;
    d := i;
    r := i;
    lr := r;
END_PROGRAM
