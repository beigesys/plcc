// SPDX-License-Identifier: MPL-2.0

//! Untyped real literals keep LREAL precision until their context narrows them.
//! They were compiled as REAL (f32) constants, so every LREAL computation with a
//! literal lost precision silently: `lr := 0.1` was 0.10000000149011612,
//! `lr := 1.0E300` was +inf, `lr := SQRT(2.0)` had 7 correct digits.

mod common;
use common::{run, run_o3};

#[test]
fn lreal_literals_keep_their_precision() {
    let src = r#"
PROGRAM p
VAR
  a : LREAL; b : LREAL := 0.1; c : LREAL; d : LREAL; e : LREAL := 3.0; g : LREAL; h : LREAL;
  r : REAL := 1.5; r2 : REAL; r3 : REAL; big : LREAL; cmp : BOOL;
END_VAR
a := 0.1;
c := e * 0.1;
d := 1.0E300;
g := 0.1 + 0.2;
h := SQRT(2.0);
r2 := r * 0.1;
r3 := 0.1;
big := 1.0E308 * 10.0;
cmp := r < 1.50000001;
END_PROGRAM
"#;
    for s in [run(src), run_o3(src)] {
        assert_eq!(s.f64("a"), 0.1);
        assert_eq!(s.f64("b"), 0.1);
        assert_eq!(s.f64("c"), 3.0 * 0.1);
        assert_eq!(s.f64("d"), 1.0e300);
        assert_eq!(s.f64("g"), 0.1 + 0.2);
        assert_eq!(s.f64("h"), 2f64.sqrt());
        assert_eq!(s.f64("r2"), (1.5f32 * 0.1f32) as f64, "REAL * literal is REAL arithmetic");
        assert_eq!(s.f64("r3"), 0.1f32 as f64);
        assert_eq!(s.f64("big"), f64::INFINITY);
        assert!(!s.bool("cmp"), "1.50000001 compared with a REAL is 1.5");
    }
}
