// SPDX-License-Identifier: MPL-2.0

//! Enumerations and CASE labels.
//!
//! * Bare enumerators (`m := Idle;`) were "unknown identifier"; `Mode#Idle` compiled
//!   the bare name and failed the same way; `Mode.Idle` (CODESYS) likewise.
//! * An enumerator without a value was numbered by position, not previous + 1.
//! * CASE only understood bare integer-literal labels: `-1:`, `INT#8:`, `K:` (a
//!   CONSTANT) and enumerators were dropped silently, so their branch never ran.
//! * A `Mode#Idle:` label made the parser loop forever, allocating until the
//!   process died.

mod common;
use common::{compile_error, run, run_o3};

#[test]
fn bare_qualified_and_dotted_enumerators() {
    let src = r#"
TYPE Mode : (Idle, Running := 5, Stopped); END_TYPE
TYPE Color : DINT (Red := 1, Green := 2, Blue := 4); END_TYPE
PROGRAM p
VAR
  m : Mode;
  m2 : Mode := Running;
  c : Color := Color#Green;
  a : DINT; b : DINT; g : DINT; h : DINT; k : DINT;
END_VAR
m := Idle;
a := m;
IF m2 = Running THEN b := 1; END_IF;
m := Stopped;
g := m;
h := Mode.Stopped;
k := c;
END_PROGRAM
"#;
    let s = run(src);
    assert_eq!(s.i64("a"), 0);
    assert_eq!(s.i64("b"), 1);
    assert_eq!(s.i64("g"), 6, "Stopped follows Running := 5");
    assert_eq!(s.i64("h"), 6);
    assert_eq!(s.i64("k"), 2);
    assert_eq!(s.i64("c"), 2, "DINT-based enum stored as DINT");
}

#[test]
fn case_labels_beyond_integer_literals() {
    let src = r#"
TYPE E1 : (A, B, C); END_TYPE
TYPE E2 : (X := -2, Y, Z); END_TYPE
PROGRAM p
VAR CONSTANT K : INT := 7; END_VAR
VAR
  i : INT := -1; r1 : DINT; r2 : DINT; r3 : DINT; r4 : DINT; r5 : DINT;
  bt : BYTE := 200; big : DINT := 1500000000;
  e : E2 := Y;
  inl : (Red, Green, Blue) := Blue;
  r7 : DINT; r8 : DINT;
  ch : DINT := 7;
  m : E1 := C;
END_VAR
CASE i OF
  -1: r1 := 1;
  0: r1 := 2;
ELSE r1 := 3;
END_CASE;
CASE bt OF
  200: r2 := 1;
ELSE r2 := 2;
END_CASE;
CASE big OF
  0..2000000000: r3 := 1;
END_CASE;
CASE ch OF
  K: r4 := 1;
  INT#8: r4 := 2;
END_CASE;
CASE e OF
  X: r5 := 1;
  Y: r5 := 2;
  Z: r5 := 3;
END_CASE;
CASE inl OF
  Red: r7 := 1;
  Blue: r7 := 3;
END_CASE;
CASE m OF
  E1#A, E1#B: r8 := 1;
  E1#C: r8 := 3;
END_CASE;
END_PROGRAM
"#;
    for s in [run(src), run_o3(src)] {
        assert_eq!(s.i64("r1"), 1, "negative label");
        assert_eq!(s.i64("r2"), 1, "BYTE selector above 127");
        assert_eq!(s.i64("r3"), 1, "a huge range");
        assert_eq!(s.i64("r4"), 1, "a CONSTANT label");
        assert_eq!(s.i64("r5"), 2, "Y follows X := -2, so it is -1");
        assert_eq!(s.i64("r7"), 3, "inline enum");
        assert_eq!(s.i64("r8"), 3, "qualified enum label");
    }
}

#[test]
fn ambiguous_bare_enumerator_is_an_error() {
    let src = r#"
TYPE E1 : (A, B); END_TYPE
TYPE E2 : (B, A); END_TYPE
PROGRAM p
VAR x : E1; END_VAR
x := A;
END_PROGRAM
"#;
    let e = compile_error(src);
    assert!(e.contains("several types"), "{e}");
}
