// SPDX-License-Identifier: MPL-2.0

//! A FUNCTION may call a FUNCTION declared later in the unit (or in a later
//! file). Only FB scan functions and methods were declared before bodies were
//! compiled, so this was "unknown function `B`" — the first thing that stopped a
//! merged compile of OSCAT.

mod common;
use common::run;

#[test]
fn forward_function_calls() {
    let src = r#"
FUNCTION A : INT
VAR_INPUT x : INT; END_VAR
A := B(x) + 1;
END_FUNCTION
FUNCTION_BLOCK F
VAR_OUTPUT o : INT; END_VAR
o := C();
END_FUNCTION_BLOCK
FUNCTION B : INT
VAR_INPUT x : INT; END_VAR
B := x * 2;
END_FUNCTION
FUNCTION C : INT
C := 42;
END_FUNCTION
PROGRAM p
VAR r : INT; f : F; o : INT; END_VAR
r := A(5);
f();
o := f.o;
END_PROGRAM
"#;
    let s = run(src);
    assert_eq!(s.i64("r"), 11);
    assert_eq!(s.i64("o"), 42);
}
