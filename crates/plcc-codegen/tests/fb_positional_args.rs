// SPDX-License-Identifier: MPL-2.0

//! Positional (non-formal) arguments in function block calls: `f(1, 2);`.
//!
//! They used to be skipped without a diagnostic, so the FB ran with its inputs
//! unchanged. IEC 61131-3 3rd ed. Annex A allows a `param_assign` without a name;
//! positional arguments bind the VAR_INPUT / VAR_IN_OUT parameters in declaration
//! order, as they do for a FUNCTION. CODESYS accepts them too: its compiler error
//! C0044 is `inst(1);` against an FB with no input, fixed by declaring one.

mod common;
use common::{compile_error, run};

const FB: &str = r#"
FUNCTION_BLOCK FB_A
VAR_INPUT a : DINT; b : DINT; END_VAR
VAR_OUTPUT s : DINT; END_VAR
VAR_IN_OUT acc : DINT; END_VAR
VAR_INPUT c : DINT := 100; END_VAR
s := a * 10 + b + c;
acc := acc + s;
END_FUNCTION_BLOCK
"#;

#[test]
fn positional_arguments_bind_inputs_and_in_outs_in_declaration_order() {
    let src = format!(
        "{FB}
PROGRAM p
VAR r : DINT; total : DINT := 5; f : FB_A; END_VAR
f(1, 2, total, 3);
r := f.s;
END_PROGRAM"
    );
    let s = run(&src);
    assert_eq!(s.i64("r"), 15);
    assert_eq!(s.i64("total"), 20);
}

#[test]
fn fewer_positional_arguments_leave_the_rest_unchanged() {
    // An FB input that a call does not assign keeps its previous value, however
    // the call is written.
    let src = format!(
        "{FB}
PROGRAM p
VAR r : DINT; f : FB_A; END_VAR
f.b := 7;
f(4);
r := f.s;
END_PROGRAM"
    );
    // acc is not bound: the FB dereferences a null in-out, so bind it by name first.
    let src = src.replace("f(4);", "f(a := 0, acc := r);\nf(4);");
    let s = run(&src);
    assert_eq!(s.i64("r"), 147);
}

#[test]
fn positional_standard_fb_call() {
    let src = r#"
PROGRAM p
VAR q : BOOL; cv : INT; c : CTU; END_VAR
c(TRUE, FALSE, 5);
c(FALSE, FALSE, 5);
c(TRUE, FALSE, 5);
q := c.Q;
cv := c.CV;
END_PROGRAM
"#;
    let s = run(src);
    assert_eq!(s.i64("cv"), 2);
    assert!(!s.bool("q"));
}

#[test]
fn mixing_named_and_positional_is_an_error() {
    let src = format!(
        "{FB}
PROGRAM p
VAR r : DINT; f : FB_A; END_VAR
f(1, b := 2);
END_PROGRAM"
    );
    let e = compile_error(&src);
    assert!(e.contains("mixed"), "{e}");
}

#[test]
fn too_many_positional_arguments_is_an_error() {
    let src = format!(
        "{FB}
PROGRAM p
VAR r : DINT; f : FB_A; END_VAR
f(1, 2, r, 3, 4);
END_PROGRAM"
    );
    let e = compile_error(&src);
    assert!(e.contains("too many"), "{e}");
}
