// SPDX-License-Identifier: MPL-2.0

//! `EXTENDS`, `THIS^`, `SUPER^`, and method calls without `THIS^.`.
//!
//! EXTENDS was parsed and ignored: a derived FB had none of its base's variables
//! or methods ("unknown identifier `w`"), and THIS^/SUPER^ did not exist.

mod common;
use common::{compile_error, run};

const SRC: &str = r#"
FUNCTION_BLOCK Base
VAR_INPUT w : REAL := 2.0; END_VAR
VAR calls : INT; END_VAR
VAR_OUTPUT q : INT; END_VAR
q := q + 1;
METHOD Area : REAL
Area := w * w;
END_METHOD
METHOD Twice : REAL
Twice := THIS^.Area() * 2.0;
END_METHOD
METHOD Thrice : REAL
Thrice := Area() * 3.0;
END_METHOD
METHOD Hit
calls := calls + 1;
END_METHOD
END_FUNCTION_BLOCK
FUNCTION_BLOCK Derived EXTENDS Base
VAR h : REAL := 3.0; END_VAR
VAR_OUTPUT dq : INT; END_VAR
dq := dq + 10;
SUPER^();
METHOD Area : REAL
Area := SUPER^.Area() + h;
END_METHOD
END_FUNCTION_BLOCK
FUNCTION_BLOCK D2 EXTENDS Derived
END_FUNCTION_BLOCK
CLASS Shape
VAR n : INT := 3; END_VAR
METHOD Sides : INT
Sides := n;
END_METHOD
METHOD Describe : INT
Describe := Sides() * 100;
END_METHOD
END_CLASS
CLASS Square EXTENDS Shape
METHOD Sides : INT
Sides := n + 1;
END_METHOD
END_CLASS
PROGRAM p
VAR
  b : Base; d : Derived; e : D2; sq : Square;
  r1 : REAL; r2 : REAL; r3 : REAL; r4 : REAL; r5 : REAL; c : INT; qq : INT;
  dq : INT; eq : INT; ww : REAL; ea : REAL; sd : INT;
END_VAR
r1 := b.Area();
r2 := d.Area();
r3 := d.Twice();
r4 := d.Thrice();
r5 := b.Twice();
d.Hit(); d.Hit();
c := d.calls;
d(); d(w := 5.0);
qq := d.q;
dq := d.dq;
ww := d.w;
e(); eq := e.q;
ea := e.Area();
sd := sq.Describe();
END_PROGRAM
"#;

#[test]
fn inheritance_this_and_super() {
    let s = run(SRC);
    assert_eq!(s.f64("r1"), 4.0, "base method");
    assert_eq!(s.f64("r2"), 7.0, "override calling SUPER^.Area()");
    assert_eq!(s.f64("r3"), 14.0, "inherited method calling THIS^.Area() reaches the override");
    assert_eq!(s.f64("r4"), 21.0, "and so does a plain Area() call");
    assert_eq!(s.f64("r5"), 8.0);
    assert_eq!(s.i64("c"), 2, "inherited method writes the inherited variable");
    assert_eq!(s.i64("qq"), 2, "SUPER^() runs the base body");
    assert_eq!(s.i64("dq"), 20);
    assert_eq!(s.f64("ww"), 5.0, "inherited input bound by name");
    assert_eq!(s.i64("eq"), 0, "a derived FB runs its own (empty) body only");
    assert_eq!(s.f64("ea"), 7.0, "SUPER^ in an inherited method is the declaring POU's base");
    assert_eq!(s.i64("sd"), 400, "CLASS EXTENDS, late-bound Sides()");
}

#[test]
fn redeclaring_an_inherited_variable_is_an_error() {
    let src = r#"
FUNCTION_BLOCK A VAR x : INT; END_VAR END_FUNCTION_BLOCK
FUNCTION_BLOCK B EXTENDS A VAR x : INT; END_VAR END_FUNCTION_BLOCK
PROGRAM p VAR b : B; END_VAR b(); END_PROGRAM
"#;
    let e = compile_error(src);
    assert!(e.contains("already inherits"), "{e}");
}
