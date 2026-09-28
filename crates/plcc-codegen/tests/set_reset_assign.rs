// SPDX-License-Identifier: MPL-2.0

//! CODESYS set/reset assignments `q S= cond;` and `q R= cond;`. They did not
//! parse (before `;` was required, `a S= TRUE` parsed as the statement `a`
//! followed by garbage).

mod common;
use common::run;

#[test]
fn set_and_reset_assignments() {
    let src = r#"
PROGRAM p
VAR a : BOOL; b : BOOL := TRUE; c : BOOL; d : BOOL := TRUE; S : INT := 1; x : BOOL; END_VAR
a S= TRUE;
b R= TRUE;
c S= FALSE;
d R= FALSE;
x := S=1;
END_PROGRAM
"#;
    let s = run(src);
    assert!(s.bool("a"));
    assert!(!s.bool("b"));
    assert!(!s.bool("c"), "S= FALSE leaves it alone");
    assert!(s.bool("d"), "R= FALSE leaves it alone");
    assert!(s.bool("x"), "a variable named S still compares with =");
}
