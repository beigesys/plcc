// SPDX-License-Identifier: MPL-2.0

//! INTERFACE references: `itf := inst;`, `itf.M(..)` (late bound), `itf = 0`,
//! interface-typed FB inputs and FUNCTION parameters, arrays of interfaces,
//! EXTENDS between interfaces, IMPLEMENTS inherited from a base.
//!
//! An interface variable was a lone pointer nothing could assign or call
//! through; both were compile errors.

mod common;
use common::{compile_error, run, run_o3};

const SRC: &str = r#"
INTERFACE INamed
METHOD Id : INT
END_METHOD
END_INTERFACE
INTERFACE IShape EXTENDS INamed
METHOD Area : REAL
END_METHOD
METHOD Scale
VAR_INPUT k : REAL; END_VAR
VAR_OUTPUT before : REAL; END_VAR
END_METHOD
END_INTERFACE
FUNCTION_BLOCK Sq IMPLEMENTS IShape
VAR_INPUT w : REAL := 2.0; END_VAR
METHOD Area : REAL
Area := w * w;
END_METHOD
METHOD Scale
VAR_INPUT k : REAL; END_VAR
VAR_OUTPUT before : REAL; END_VAR
before := w;
w := w * k;
END_METHOD
METHOD Id : INT
Id := 1;
END_METHOD
END_FUNCTION_BLOCK
FUNCTION_BLOCK Rect EXTENDS Sq
VAR_INPUT h : REAL := 3.0; END_VAR
METHOD Area : REAL
Area := w * h;
END_METHOD
METHOD Id : INT
Id := 2;
END_METHOD
END_FUNCTION_BLOCK
FUNCTION TotalArea : REAL
VAR_INPUT a : IShape; b : IShape; END_VAR
TotalArea := a.Area() + b.Area();
END_FUNCTION
FUNCTION_BLOCK Holder
VAR_INPUT s : IShape; END_VAR
VAR_OUTPUT area : REAL; id : INT; END_VAR
IF s <> 0 THEN
  area := s.Area();
  id := s.Id();
END_IF;
END_FUNCTION_BLOCK
PROGRAM p
VAR
  sq : Sq; rc : Rect; itf : IShape; named : INamed; shapes : ARRAY[1..2] OF IShape; h : Holder;
  a1 : REAL; a2 : REAL; a3 : REAL; tot : REAL; id1 : INT; id2 : INT; bound0 : BOOL; bound1 : BOOL;
  before : REAL; after : REAL; ha : REAL; hid : INT; empty : Holder; ea : REAL;
END_VAR
bound0 := itf <> 0;
itf := sq;
bound1 := itf <> 0;
a1 := itf.Area();
itf := rc;
a2 := itf.Area();
itf.Scale(k := 2.0, before => before);
after := rc.w;
named := rc;
id1 := named.Id();
shapes[1] := sq;
shapes[2] := rc;
a3 := shapes[2].Area();
id2 := shapes[1].Id();
tot := TotalArea(sq, rc);
h(s := rc);
ha := h.area;
hid := h.id;
empty();
ea := empty.area;
END_PROGRAM
"#;

#[test]
fn interface_references() {
    for s in [run(SRC), run_o3(SRC)] {
        assert!(!s.bool("bound0"));
        assert!(s.bool("bound1"));
        assert_eq!(s.f64("a1"), 4.0);
        assert_eq!(s.f64("a2"), 6.0, "late bound: Rect's Area");
        assert_eq!(s.f64("before"), 2.0, "VAR_OUTPUT through the interface");
        assert_eq!(s.f64("after"), 4.0, "the method ran on rc itself");
        assert_eq!(s.i64("id1"), 2, "an extended interface's method");
        assert_eq!(s.f64("a3"), 12.0);
        assert_eq!(s.i64("id2"), 1);
        assert_eq!(s.f64("tot"), 4.0 + 12.0);
        assert_eq!(s.f64("ha"), 12.0);
        assert_eq!(s.i64("hid"), 2);
        assert_eq!(s.f64("ea"), 0.0, "an unbound interface input is 0");
    }
}

#[test]
fn binding_an_instance_that_does_not_implement_the_interface_is_an_error() {
    let src = r#"
INTERFACE I METHOD M : INT END_METHOD END_INTERFACE
FUNCTION_BLOCK F METHOD M : INT M := 1; END_METHOD END_FUNCTION_BLOCK
PROGRAM p VAR f : F; i : I; END_VAR
i := f;
END_PROGRAM
"#;
    let e = compile_error(src);
    assert!(e.contains("does not implement"), "{e}");
}
