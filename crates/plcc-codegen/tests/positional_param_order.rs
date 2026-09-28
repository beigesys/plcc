// SPDX-License-Identifier: MPL-2.0

//! A positional FUNCTION / METHOD call binds its arguments in declaration order.
//! Parameters were ordered VAR_INPUT first, then VAR_IN_OUT, whatever the
//! declaration said, so `Fill(arr, 42)` on `VAR_IN_OUT arr; VAR_INPUT v` passed
//! `arr` to `v` — an error there, but a silent swap when both arguments are
//! variables.

mod common;
use common::run;

#[test]
fn positional_arguments_follow_declaration_order() {
    let src = r#"
FUNCTION Mix : INT
VAR_IN_OUT io : INT; END_VAR
VAR_INPUT k : INT; END_VAR
io := io + k;
Mix := k;
END_FUNCTION
FUNCTION_BLOCK F
METHOD M : INT
VAR_IN_OUT io : INT; END_VAR
VAR_INPUT k : INT; END_VAR
io := io * k;
M := k;
END_METHOD
END_FUNCTION_BLOCK
PROGRAM p
VAR a : INT := 1; b : INT := 10; r1 : INT; r2 : INT; f : F; END_VAR
r1 := Mix(a, b);
r2 := f.M(a, b);
END_PROGRAM
"#;
    let s = run(src);
    assert_eq!(s.i64("r1"), 10);
    assert_eq!(s.i64("r2"), 10);
    assert_eq!(s.i64("a"), 110, "(1 + 10) * 10");
    assert_eq!(s.i64("b"), 10, "the input is not written");
}
