// SPDX-License-Identifier: MPL-2.0

//! A `VAR CONSTANT` of a function block read from outside it through the
//! block's name, `FB_Env.MAX_LEN`, as CODESYS and TwinCAT allow (TcOpen
//! declares `STRING(TcoMessengerEnv.MAX_MESSAGE_TEXT_LENGHT)`): in a type,
//! and in an expression.

mod common;
use common::run;

const SRC: &str = r#"
FUNCTION_BLOCK FB_Env
VAR CONSTANT
    MAX_LEN : INT := 16;
    QUARTER : INT := MAX_LEN / 4;
    GAIN : REAL := 2.5;
END_VAR
END_FUNCTION_BLOCK

PROGRAM p
VAR
    s : STRING(FB_Env.MAX_LEN);
    a : ARRAY[1..FB_Env.QUARTER] OF INT;
    len_s : DINT;
    last : INT;
    total : DINT;
    r : REAL;
END_VAR
s := 'abcdefghijklmnopqrstuvwxyz';
len_s := LEN(s);
a[FB_Env.QUARTER] := 9;
last := a[4];
total := FB_Env.MAX_LEN + FB_Env.QUARTER;
r := FB_Env.GAIN * 2.0;
END_PROGRAM
"#;

#[test]
fn fb_constants_by_type_name() {
    let s = run(SRC);
    // STRING(16) truncates the 26-character literal.
    assert_eq!(s.i64("len_s"), 16);
    // ARRAY[1..4]: element 4 exists.
    assert_eq!(s.i64("last"), 9);
    assert_eq!(s.i64("total"), 20);
    assert_eq!(s.f64("r"), 5.0);
}
