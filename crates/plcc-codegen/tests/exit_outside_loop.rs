// SPDX-License-Identifier: MPL-2.0

//! EXIT and CONTINUE outside a loop are compile errors (as in CODESYS). They were
//! silently dropped: `a := 1; EXIT; a := 2;` ran both assignments.

mod common;
use common::{compile_error, run};

#[test]
fn exit_and_continue_outside_a_loop_are_errors() {
    for stmt in ["EXIT;", "CONTINUE;", "IF a = 1 THEN EXIT; END_IF;"] {
        let src = format!("PROGRAM p VAR a : INT; END_VAR a := 1; {stmt} a := 2; END_PROGRAM");
        let e = compile_error(&src);
        assert!(e.contains("outside a FOR, WHILE or REPEAT loop"), "{stmt}: {e}");
    }
}

#[test]
fn exit_inside_a_loop_still_works() {
    let s = run("PROGRAM p VAR a : INT; i : INT; END_VAR
FOR i := 1 TO 10 DO a := i; IF i = 3 THEN EXIT; END_IF; END_FOR;
END_PROGRAM");
    assert_eq!(s.i64("a"), 3);
}
