// SPDX-License-Identifier: MPL-2.0

//! Integer literals that do not fit are compile errors. They were truncated to
//! their low bits without a word: `INT#70000` was 4464, a 20-digit literal lost
//! its top bits, `16#1FFFFFFFFFFFFFFFF` (65 bits) became 16#FFFFFFFFFFFFFFFF.

mod common;
use common::{compile_error, run};

#[test]
fn out_of_range_literals_are_errors() {
    for (expr, needle) in [
        ("INT#70000", "out of range for INT"),
        ("BYTE#256", "out of range for BYTE"),
        ("SINT#-129", "out of range for SINT"),
        ("BOOL#2", "out of range for BOOL"),
        ("99999999999999999999", "does not fit in 64 bits"),
        ("16#1FFFFFFFFFFFFFFFF", "does not fit in 64 bits"),
    ] {
        let src = format!("PROGRAM p VAR x : LINT; END_VAR x := {expr}; END_PROGRAM");
        let e = compile_error(&src);
        assert!(e.contains(needle), "{expr}: {e}");
    }
}

#[test]
fn literals_at_the_edges_still_work() {
    let src = r#"
PROGRAM p
VAR a : INT; b : WORD; c : SINT; d : ULINT; e : LINT; f : INT; END_VAR
a := INT#-32768;
b := WORD#16#FFFF;
c := SINT#-128;
d := 18446744073709551615;
e := LINT#-9223372036854775808;
f := INT#16#FFFF;
END_PROGRAM
"#;
    let s = run(src);
    assert_eq!(s.i64("a"), -32768);
    assert_eq!(s.u64("b"), 0xFFFF);
    assert_eq!(s.i64("c"), -128);
    assert_eq!(s.u64("d"), u64::MAX);
    assert_eq!(s.i64("e"), i64::MIN);
    assert_eq!(s.i64("f"), -1, "a based literal is a bit pattern of the type's width");
}
