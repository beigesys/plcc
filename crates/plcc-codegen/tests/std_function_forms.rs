// SPDX-License-Identifier: MPL-2.0

//! Standard functions that were missing or broke on mixed types: `MUX`, `MOVE`,
//! the functional operators `ADD`/`MUL`/`SUB`/`DIV`/`GT`/`GE`/`EQ`/`LE`/`LT`/`NE`,
//! extensible `MIN`/`MAX`, and `SEL` with inputs of different types (an invalid
//! `select i16/i32` the LLVM verifier rejected).

mod common;
use common::{run, run_o3};

#[test]
fn functional_forms_and_mux() {
    let src = r#"
PROGRAM p
VAR
  a : INT := -5; b : DINT := 70000; k : INT := 2; sel : BOOL := TRUE;
  s1 : DINT; s2 : DINT; s3 : DINT; s4 : DINT; mv : INT; mx : INT; mn : REAL;
  g1 : BOOL; g2 : BOOL; e1 : BOOL; n1 : BOOL; ad : DINT; mu : DINT; su : INT; dv : INT;
  kk : INT := 7;
END_VAR
s1 := SEL(sel, a, b);
s2 := SEL(NOT sel, a, b);
s3 := MUX(k, 10, 20, 30);
s4 := MUX(kk, 10, 20, 30);
mv := MOVE(a);
mx := MAX(1, 2, 3, 7, 5);
mn := MIN(4.5, 2, 3.25);
g1 := GT(3, 2, 1);
g2 := GT(3, 2, 2);
e1 := EQ(a, -5);
n1 := NE(a, 5);
ad := ADD(1, 2, 3, b);
mu := MUL(2, 3, 4);
su := SUB(10, 4);
dv := DIV(20, 3);
END_PROGRAM
"#;
    for s in [run(src), run_o3(src)] {
        assert_eq!(s.i64("s1"), 70000);
        assert_eq!(s.i64("s2"), -5);
        assert_eq!(s.i64("s3"), 30);
        assert_eq!(s.i64("s4"), 10, "an out-of-range K selects IN0");
        assert_eq!(s.i64("mv"), -5);
        assert_eq!(s.i64("mx"), 7);
        assert_eq!(s.f64("mn"), 2.0);
        assert!(s.bool("g1"));
        assert!(!s.bool("g2"));
        assert!(s.bool("e1"));
        assert!(s.bool("n1"));
        assert_eq!(s.i64("ad"), 70006);
        assert_eq!(s.i64("mu"), 24);
        assert_eq!(s.i64("su"), 6);
        assert_eq!(s.i64("dv"), 6);
    }
}
