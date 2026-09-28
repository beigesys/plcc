// SPDX-License-Identifier: MPL-2.0

//! REAL/LREAL ⇄ STRING. They were "unknown function" (and stopped a merged
//! compile of OSCAT). They are written in ST (compiler/real_string.st) and
//! compiled into a module only when it calls them.

mod common;
use common::run;

#[test]
fn reals_to_and_from_strings() {
    let src = r#"
PROGRAM p
VAR
  s1 : STRING; s2 : STRING; s3 : STRING; s4 : STRING; s5 : STRING; s6 : STRING;
  s7 : STRING; s8 : STRING; s9 : STRING; s10 : STRING; s11 : STRING; s12 : STRING;
  r1 : REAL; r2 : LREAL; r3 : REAL; r4 : REAL; r5 : LREAL;
  z : REAL := 0.0;
END_VAR
s1 := REAL_TO_STRING(1.234);
s2 := REAL_TO_STRING(2.0);
s3 := REAL_TO_STRING(-0.5);
s4 := REAL_TO_STRING(1.0 / 3.0);
s5 := REAL_TO_STRING(1.23456789E11);
s6 := REAL_TO_STRING(0.000012345);
s7 := LREAL_TO_STRING(LREAL#3.141592653589793);
s8 := REAL_TO_STRING(100.0);
s9 := REAL_TO_STRING(0.0);
s10 := REAL_TO_STRING(1.0 / z);
s11 := REAL_TO_STRING(z / z);
s12 := CONCAT('T=', REAL_TO_STRING(21.5));
r1 := STRING_TO_REAL('1.5');
r2 := STRING_TO_LREAL(' -2.5e3');
r3 := STRING_TO_REAL('abc');
r4 := STRING_TO_REAL('3');
r5 := STRING_TO_LREAL('0.1');
END_PROGRAM
"#;
    let s = run(src);
    assert_eq!(s.str("s1"), "1.234");
    assert_eq!(s.str("s2"), "2");
    assert_eq!(s.str("s3"), "-0.5");
    assert_eq!(s.str("s4"), "0.3333333");
    assert_eq!(s.str("s5"), "1.234568E+11");
    assert_eq!(s.str("s6"), "0.000012345");
    assert_eq!(s.str("s7"), "3.14159265358979");
    assert_eq!(s.str("s8"), "100");
    assert_eq!(s.str("s9"), "0");
    assert_eq!(s.str("s10"), "INF");
    assert_eq!(s.str("s11"), "NaN");
    assert_eq!(s.str("s12"), "T=21.5");
    assert_eq!(s.f64("r1"), 1.5);
    assert_eq!(s.f64("r2"), -2500.0);
    assert_eq!(s.f64("r3"), 0.0);
    assert_eq!(s.f64("r4"), 3.0);
    assert!((s.f64("r5") - 0.1).abs() < 1e-15);
}

#[test]
fn unused_helpers_are_not_compiled_in() {
    let src = "PROGRAM p VAR x : INT; END_VAR x := 1; END_PROGRAM";
    let (unit, errors) = plcc_st::parse(src);
    assert!(errors.is_empty());
    let ctx = inkwell::context::Context::create();
    let mut c = plcc_codegen::Compiler::new(&ctx, "m");
    c.compile(&unit).unwrap();
    assert!(!c.emit_ir().contains("real_to_string"));
}
