// SPDX-License-Identifier: MPL-2.0

//! An unknown type name in a variable declaration must be a hard error.
//!
//! Before this, `t : TON;` with no `TON` in scope compiled cleanly: the field became
//! an anonymous `i32` slot and every statement that touched `t` was silently dropped
//! from the generated code. `main_scan` came out as a single `ret`. No diagnostic, no
//! undefined symbol at link time — the program just did nothing.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;

/// Compile and return the codegen error message, or `Ok(())` on success.
fn try_compile(source: &str) -> Result<(), String> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let context = Context::create();
    let mut compiler = Compiler::new(&context, "unresolved_test");
    compiler.compile(&unit).map_err(|e| e.to_string())
}

fn expect_error(source: &str) -> String {
    match try_compile(source) {
        Ok(()) => panic!("expected a codegen error, but compilation succeeded"),
        Err(msg) => msg,
    }
}

// ---------------------------------------------------------------------------
// The headline case: a standard FB with no definition in scope.
// ---------------------------------------------------------------------------

#[test]
fn undefined_standard_fb_instance_is_rejected() {
    let source = r#"
PROGRAM Main
VAR
    t : TON;
    q : BOOL;
END_VAR
    t(IN := TRUE, PT := T#100ms);
    q := t.Q;
END_PROGRAM
"#;
    let msg = expect_error(source);
    assert!(
        msg.contains("TON"),
        "the diagnostic must name the unknown type; got: {msg}"
    );
    assert!(
        msg.contains('t'),
        "the diagnostic must name the variable; got: {msg}"
    );
    assert!(
        msg.contains("Main"),
        "the diagnostic must name the POU; got: {msg}"
    );
}

#[test]
fn undefined_type_is_rejected_even_when_never_used() {
    // The declaration alone is enough: the state struct layout is already wrong.
    let source = r#"
PROGRAM Main
VAR
    t : TON;
    x : INT := 5;
END_VAR
    x := x + 1;
END_PROGRAM
"#;
    let msg = expect_error(source);
    assert!(msg.contains("TON"), "got: {msg}");
}

#[test]
fn undefined_type_inside_a_function_block_is_rejected() {
    let source = r#"
FUNCTION_BLOCK Wrapper
VAR
    inner : CTU;
END_VAR
    ;
END_FUNCTION_BLOCK

PROGRAM Main
VAR
    w : Wrapper;
END_VAR
    w();
END_PROGRAM
"#;
    let msg = expect_error(source);
    assert!(msg.contains("CTU"), "got: {msg}");
    assert!(msg.contains("Wrapper"), "got: {msg}");
}

#[test]
fn undefined_element_type_of_an_array_is_rejected() {
    let source = r#"
PROGRAM Main
VAR
    timers : ARRAY [1..4] OF TOF;
END_VAR
    ;
END_PROGRAM
"#;
    let msg = expect_error(source);
    assert!(
        msg.contains("TOF"),
        "the array element type must be checked too; got: {msg}"
    );
}

#[test]
fn undefined_global_type_is_rejected() {
    let source = r#"
VAR_GLOBAL
    g : R_TRIG;
END_VAR

PROGRAM Main
VAR
    x : INT;
END_VAR
    x := 1;
END_PROGRAM
"#;
    let msg = expect_error(source);
    assert!(msg.contains("R_TRIG"), "got: {msg}");
}

// ---------------------------------------------------------------------------
// The good path must keep working — and must actually execute.
// ---------------------------------------------------------------------------

#[test]
fn user_defined_fb_still_compiles_and_runs() {
    let source = r#"
FUNCTION_BLOCK MyTimer
VAR_INPUT
    IN : BOOL;
END_VAR
VAR_OUTPUT
    Q : BOOL;
    CV : INT;
END_VAR
    IF IN THEN
        CV := CV + 1;
    ELSE
        CV := 0;
    END_IF;
    Q := CV >= 3;
END_FUNCTION_BLOCK

PROGRAM Main
VAR
    t : MyTimer;
    fired : BOOL;
    count : INT;
END_VAR
    t(IN := TRUE);
    fired := t.Q;
    count := t.CV;
END_PROGRAM
"#;
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let context = Context::create();
    let mut compiler = Compiler::new(&context, "good_path");
    compiler.compile(&unit).expect("codegen failed");

    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT");

    let mut state = vec![0u8; 1024];
    let ptr = state.as_mut_ptr();
    if let Ok(a) = ee.get_function_address("main_init") {
        let f: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(a) };
        f(ptr);
    }
    let a = ee
        .get_function_address("main_scan")
        .expect("main_scan missing");
    let scan: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(a) };

    // MyTimer state is { IN:i8, Q:i8, CV:i16 } = 4 bytes, so Main is
    // { t:{i8,i8,i16}, fired:i8, count:i16 }. Read `count` via the program's own
    // fields rather than guessing: field offsets are t=0..4, fired=4, count=6.
    let read_i16 = |s: &[u8], off: usize| i16::from_ne_bytes([s[off], s[off + 1]]);

    scan(ptr);
    assert_eq!(read_i16(&state, 6), 1, "first scan CV");
    assert_eq!(state[4], 0, "Q must still be FALSE after 1 scan");
    scan(ptr);
    assert_eq!(read_i16(&state, 6), 2, "second scan CV");
    assert_eq!(state[4], 0, "Q must still be FALSE after 2 scans");
    scan(ptr);
    assert_eq!(read_i16(&state, 6), 3, "third scan CV");
    assert_eq!(state[4], 1, "Q must be TRUE once CV reaches 3");
}

#[test]
fn user_type_declarations_after_the_first_in_a_type_block_resolve() {
    // A TYPE block may declare several types. Only the first used to survive parsing,
    // so `m : MyInt` resolved to nothing and was silently laid out as an i32.
    let source = r#"
TYPE
  Point : STRUCT
    x : INT;
    y : INT;
  END_STRUCT;
  MyInt : INT;
END_TYPE

PROGRAM Main
VAR
    p : Point;
    m : MyInt;
END_VAR
    p.x := 3;
    p.y := 4;
    m := p.x + p.y;
END_PROGRAM
"#;
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let context = Context::create();
    let mut compiler = Compiler::new(&context, "typedecl");
    compiler.compile(&unit).expect("codegen failed");

    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT");
    let mut state = vec![0u8; 1024];
    let ptr = state.as_mut_ptr();
    if let Ok(a) = ee.get_function_address("main_init") {
        let f: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(a) };
        f(ptr);
    }
    let a = ee
        .get_function_address("main_scan")
        .expect("main_scan missing");
    let scan: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(a) };
    scan(ptr);

    // Main state: { p:{i16,i16}, m:i16 } => p.x@0, p.y@2, m@4
    assert_eq!(i16::from_ne_bytes([state[0], state[1]]), 3, "p.x");
    assert_eq!(i16::from_ne_bytes([state[2], state[3]]), 4, "p.y");
    assert_eq!(i16::from_ne_bytes([state[4], state[5]]), 7, "m");
}
