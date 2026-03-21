TYPE
    Color : (Red, Green, Blue);
END_TYPE

TYPE
    Voltage : REAL;
END_TYPE

TYPE
    SmallInt : INT(0..100);
END_TYPE

TYPE
    TenInts : ARRAY[0..9] OF INT;
END_TYPE

TYPE
    Point : STRUCT
        x : REAL := 0.0;
        y : REAL := 0.0;
        z : REAL := 0.0;
    END_STRUCT;
END_TYPE

PROGRAM TypesTest
VAR
    color : Color := Color#Red;
    volts : Voltage := 3.3;
    small : SmallInt := 50;
    arr : TenInts;
    pt : Point;
    matrix : ARRAY[0..2, 0..2] OF REAL;
    name : STRING[80] := 'test';
END_VAR
    arr[0] := 42;
    pt.x := 1.0;
    pt.y := 2.0;
    matrix[0, 0] := 1.0;
END_PROGRAM
