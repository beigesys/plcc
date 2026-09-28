// SPDX-License-Identifier: MPL-2.0

//! Operator precedence and associativity (IEC 61131-3 §7.3.2, Table 71).
//! `a ** b ** c` did not parse: only one `**` was accepted.

mod common;
use common::run;

#[test]
fn precedence_and_associativity() {
    let src = r#"
PROGRAM p
VAR
  r1 : REAL; r2 : REAL; r3 : REAL; i1 : INT; i2 : INT; b1 : BOOL; b2 : BOOL; b3 : BOOL;
  i3 : INT; i4 : DINT; w : WORD := 16#00FF; w2 : WORD; x : INT := 5; b4 : BOOL;
END_VAR
r1 := 7 / 2;
r2 := 7 / 2.0;
r3 := -2 ** 2;
i1 := 2 + 3 * 4;
i2 := 10 - 4 - 3;
b1 := TRUE OR FALSE AND FALSE;
b2 := NOT TRUE AND FALSE;
b3 := 1 < 2 = TRUE;
i3 := 17 MOD 5 * 2;
i4 := 2 ** 3 ** 2;
w2 := w AND 16#0F OR 16#F000;
b4 := x > 3 XOR x > 10;
END_PROGRAM
"#;
    let s = run(src);
    assert_eq!(s.f64("r1"), 3.0, "integer division");
    assert_eq!(s.f64("r2"), 3.5);
    assert_eq!(s.f64("r3"), 4.0, "unary minus binds tighter than **");
    assert_eq!(s.i64("i1"), 14);
    assert_eq!(s.i64("i2"), 3, "left associative");
    assert!(s.bool("b1"), "AND before OR");
    assert!(!s.bool("b2"));
    assert!(s.bool("b3"), "< before =");
    assert_eq!(s.i64("i3"), 4);
    assert_eq!(s.i64("i4"), 64, "** is left associative: (2**3)**2");
    assert_eq!(s.u64("w2"), 0xF00F);
    assert!(s.bool("b4"));
}
