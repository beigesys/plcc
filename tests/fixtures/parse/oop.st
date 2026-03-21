INTERFACE IMovable
    METHOD Move : BOOL
    VAR_INPUT
        dx : REAL;
        dy : REAL;
    END_VAR
    END_METHOD
END_INTERFACE

CLASS Shape IMPLEMENTS IMovable
VAR
    pos_x : REAL := 0.0;
    pos_y : REAL := 0.0;
END_VAR
    METHOD Move : BOOL
    VAR_INPUT
        dx : REAL;
        dy : REAL;
    END_VAR
        pos_x := pos_x + dx;
        pos_y := pos_y + dy;
        Move := TRUE;
    END_METHOD

    METHOD GetX : REAL
        GetX := pos_x;
    END_METHOD

    METHOD GetY : REAL
        GetY := pos_y;
    END_METHOD
END_CLASS

FUNCTION_BLOCK Circle EXTENDS Shape
VAR
    radius : REAL := 1.0;
END_VAR
    METHOD Area : REAL
        Area := 3.14159 * radius * radius;
    END_METHOD
END_FUNCTION_BLOCK
