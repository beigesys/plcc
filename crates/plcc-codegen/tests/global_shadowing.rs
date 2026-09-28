// SPDX-License-Identifier: MPL-2.0

//! A POU's own variable shadows a VAR_GLOBAL of the same name. The globals were
//! bound after a POU's variables and overwrote them, so with `VAR_GLOBAL x` every
//! `x` — a PROGRAM local, an FB member, a METHOD or FUNCTION input — read and
//! wrote the global instead.

mod common;
use common::run;

#[test]
fn locals_and_parameters_shadow_globals() {
    let src = r#"
VAR_GLOBAL x : INT := 100; g : INT := 50; END_VAR
FUNCTION_BLOCK F
VAR x : INT := 1; END_VAR
VAR_OUTPUT o : INT; og : INT; END_VAR
x := x + 1; o := x; og := g;
METHOD M : INT
VAR_INPUT x : INT; END_VAR
M := x;
END_METHOD
END_FUNCTION_BLOCK
FUNCTION G2 : INT
VAR_INPUT x : INT; END_VAR
G2 := x + g;
END_FUNCTION
PROGRAM p
VAR x : INT := 5; f : F; r1 : INT; r2 : INT; r3 : INT; r4 : INT; r5 : INT; END_VAR
x := x + 1;
r1 := x;
f();
r2 := f.o;
r3 := f.M(7);
r4 := G2(8);
r5 := f.og;
END_PROGRAM
"#;
    let s = run(src);
    assert_eq!(s.i64("x"), 6, "the program's own x");
    assert_eq!(s.i64("r1"), 6);
    assert_eq!(s.i64("r2"), 2, "the FB's own x");
    assert_eq!(s.i64("r3"), 7, "the method's input x");
    assert_eq!(s.i64("r4"), 58, "the function's input x, and the global g");
    assert_eq!(s.i64("r5"), 50, "an unshadowed global is still visible");
}
