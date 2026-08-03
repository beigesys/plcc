// SPDX-License-Identifier: MPL-2.0

//! JIT execution tests for IEC 61131-3 standard library functions.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;

/// Helper: compile source, JIT execute the scan function, return state bytes.
fn compile_and_run(
    source: &str,
    scan_fn_name: &str,
    state_size: usize,
    num_scans: usize,
) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");

    let context = Context::create();
    let mut compiler = Compiler::new(&context, "test");
    compiler.compile(&unit).expect("codegen failed");

    let ir = compiler.emit_ir();
    assert!(
        ir.contains(scan_fn_name),
        "scan function not found in IR:\n{ir}"
    );

    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT execution engine");

    let mut state = vec![0u8; state_size];
    let state_ptr = state.as_mut_ptr();

    let fn_ptr = ee
        .get_function_address(scan_fn_name)
        .expect("failed to get function address");

    let scan: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(fn_ptr) };
    for _ in 0..num_scans {
        scan(state_ptr);
    }

    state
}

fn read_f32(state: &[u8], offset: usize) -> f32 {
    f32::from_ne_bytes([
        state[offset],
        state[offset + 1],
        state[offset + 2],
        state[offset + 3],
    ])
}

fn read_i16(state: &[u8], offset: usize) -> i16 {
    i16::from_ne_bytes([state[offset], state[offset + 1]])
}

#[test]
fn test_sqrt() {
    let source = r#"
PROGRAM SqrtTest
VAR
    result : REAL;
END_VAR
    result := SQRT(9.0);
END_PROGRAM
"#;
    // State: { f32 } = 4 bytes
    let state = compile_and_run(source, "sqrttest_scan", 4, 1);
    let result = read_f32(&state, 0);
    assert!(
        (result - 3.0).abs() < 0.001,
        "SQRT(9.0) should be 3.0, got {result}"
    );
}

#[test]
fn test_abs_int() {
    let source = r#"
PROGRAM AbsIntTest
VAR
    result : INT;
END_VAR
    result := ABS(-5);
END_PROGRAM
"#;
    // State: { i16 } = 2 bytes
    let state = compile_and_run(source, "absinttest_scan", 2, 1);
    let result = read_i16(&state, 0);
    assert_eq!(result, 5, "ABS(-5) should be 5, got {result}");
}

#[test]
fn test_abs_real() {
    let source = r#"
PROGRAM AbsRealTest
VAR
    result : REAL;
END_VAR
    result := ABS(-3.14);
END_PROGRAM
"#;
    let state = compile_and_run(source, "absrealtest_scan", 4, 1);
    let result = read_f32(&state, 0);
    assert!(
        (result - 3.14).abs() < 0.01,
        "ABS(-3.14) should be ~3.14, got {result}"
    );
}

#[test]
fn test_sin_cos() {
    let source = r#"
PROGRAM SinCosTest
VAR
    sin_result : REAL;
    cos_result : REAL;
END_VAR
    sin_result := SIN(0.0);
    cos_result := COS(0.0);
END_PROGRAM
"#;
    // State: { f32, f32 } = 8 bytes
    let state = compile_and_run(source, "sincostest_scan", 8, 1);
    let sin_val = read_f32(&state, 0);
    let cos_val = read_f32(&state, 4);
    assert!(
        sin_val.abs() < 0.001,
        "SIN(0.0) should be ~0.0, got {sin_val}"
    );
    assert!(
        (cos_val - 1.0).abs() < 0.001,
        "COS(0.0) should be ~1.0, got {cos_val}"
    );
}

#[test]
fn test_min_max() {
    let source = r#"
PROGRAM MinMaxTest
VAR
    min_result : INT;
    max_result : INT;
END_VAR
    min_result := MIN(3, 7);
    max_result := MAX(3, 7);
END_PROGRAM
"#;
    // State: { i16, i16 } = 4 bytes
    let state = compile_and_run(source, "minmaxtest_scan", 4, 1);
    let min_val = read_i16(&state, 0);
    let max_val = read_i16(&state, 2);
    assert_eq!(min_val, 3, "MIN(3, 7) should be 3, got {min_val}");
    assert_eq!(max_val, 7, "MAX(3, 7) should be 7, got {max_val}");
}

#[test]
fn test_limit() {
    let source = r#"
PROGRAM LimitTest
VAR
    clamped_low : INT;
    clamped_mid : INT;
    clamped_high : INT;
END_VAR
    clamped_low := LIMIT(0, -5, 100);
    clamped_mid := LIMIT(0, 50, 100);
    clamped_high := LIMIT(0, 150, 100);
END_PROGRAM
"#;
    // State: { i16, i16, i16 } = 6 bytes
    let state = compile_and_run(source, "limittest_scan", 6, 1);
    let low = read_i16(&state, 0);
    let mid = read_i16(&state, 2);
    let high = read_i16(&state, 4);
    assert_eq!(low, 0, "LIMIT(0, -5, 100) should be 0, got {low}");
    assert_eq!(mid, 50, "LIMIT(0, 50, 100) should be 50, got {mid}");
    assert_eq!(high, 100, "LIMIT(0, 150, 100) should be 100, got {high}");
}

#[test]
fn test_sel() {
    let source = r#"
PROGRAM SelTest
VAR
    sel_false : INT;
    sel_true : INT;
END_VAR
    sel_false := SEL(FALSE, 10, 20);
    sel_true := SEL(TRUE, 10, 20);
END_PROGRAM
"#;
    // State: { i16, i16 } = 4 bytes
    let state = compile_and_run(source, "seltest_scan", 4, 1);
    let sf = read_i16(&state, 0);
    let st = read_i16(&state, 2);
    assert_eq!(sf, 10, "SEL(FALSE, 10, 20) should be 10, got {sf}");
    assert_eq!(st, 20, "SEL(TRUE, 10, 20) should be 20, got {st}");
}

#[test]
fn test_int_to_real() {
    let source = r#"
PROGRAM IntToRealTest
VAR
    result : REAL;
END_VAR
    result := INT_TO_REAL(42);
END_PROGRAM
"#;
    let state = compile_and_run(source, "inttorealtest_scan", 4, 1);
    let result = read_f32(&state, 0);
    assert!(
        (result - 42.0).abs() < 0.01,
        "INT_TO_REAL(42) should be ~42.0, got {result}"
    );
}

#[test]
fn test_real_to_int() {
    let source = r#"
PROGRAM RealToIntTest
VAR
    result : INT;
END_VAR
    result := REAL_TO_INT(3.7);
END_PROGRAM
"#;
    let state = compile_and_run(source, "realtointtest_scan", 2, 1);
    let result = read_i16(&state, 0);
    assert_eq!(result, 3, "REAL_TO_INT(3.7) should be 3, got {result}");
}

#[test]
fn test_shl_shr() {
    let source = r#"
PROGRAM ShlShrTest
VAR
    shl_result : INT;
    shr_result : INT;
END_VAR
    shl_result := SHL(1, 4);
    shr_result := SHR(16, 4);
END_PROGRAM
"#;
    // State: { i16, i16 } = 4 bytes
    let state = compile_and_run(source, "shlshrtest_scan", 4, 1);
    let shl = read_i16(&state, 0);
    let shr = read_i16(&state, 2);
    assert_eq!(shl, 16, "SHL(1, 4) should be 16, got {shl}");
    assert_eq!(shr, 1, "SHR(16, 4) should be 1, got {shr}");
}

#[test]
fn test_exp_ln() {
    let source = r#"
PROGRAM ExpLnTest
VAR
    exp_result : REAL;
    ln_result : REAL;
END_VAR
    exp_result := EXP(0.0);
    ln_result := LN(1.0);
END_PROGRAM
"#;
    // State: { f32, f32 } = 8 bytes
    let state = compile_and_run(source, "explntest_scan", 8, 1);
    let exp_val = read_f32(&state, 0);
    let ln_val = read_f32(&state, 4);
    assert!(
        (exp_val - 1.0).abs() < 0.001,
        "EXP(0.0) should be ~1.0, got {exp_val}"
    );
    assert!(ln_val.abs() < 0.001, "LN(1.0) should be ~0.0, got {ln_val}");
}

#[test]
fn test_expt() {
    let source = r#"
PROGRAM ExptTest
VAR
    result : REAL;
END_VAR
    result := EXPT(2.0, 3.0);
END_PROGRAM
"#;
    let state = compile_and_run(source, "expttest_scan", 4, 1);
    let result = read_f32(&state, 0);
    assert!(
        (result - 8.0).abs() < 0.001,
        "EXPT(2.0, 3.0) should be 8.0, got {result}"
    );
}

#[test]
fn test_bool_to_int() {
    let source = r#"
PROGRAM BoolToIntTest
VAR
    result_true : INT;
    result_false : INT;
END_VAR
    result_true := BOOL_TO_INT(TRUE);
    result_false := BOOL_TO_INT(FALSE);
END_PROGRAM
"#;
    let state = compile_and_run(source, "booltointtest_scan", 4, 1);
    let rt = read_i16(&state, 0);
    let rf = read_i16(&state, 2);
    assert_eq!(rt, 1, "BOOL_TO_INT(TRUE) should be 1, got {rt}");
    assert_eq!(rf, 0, "BOOL_TO_INT(FALSE) should be 0, got {rf}");
}

#[test]
fn test_stdlib_case_insensitive() {
    // ST function names are case-insensitive
    let source = r#"
PROGRAM CaseTest
VAR
    r1 : REAL;
    r2 : REAL;
    r3 : REAL;
END_VAR
    r1 := sqrt(9.0);
    r2 := Sqrt(4.0);
    r3 := SQRT(16.0);
END_PROGRAM
"#;
    let state = compile_and_run(source, "casetest_scan", 12, 1);
    let r1 = read_f32(&state, 0);
    let r2 = read_f32(&state, 4);
    let r3 = read_f32(&state, 8);
    assert!(
        (r1 - 3.0).abs() < 0.001,
        "sqrt(9.0) should be 3.0, got {r1}"
    );
    assert!(
        (r2 - 2.0).abs() < 0.001,
        "Sqrt(4.0) should be 2.0, got {r2}"
    );
    assert!(
        (r3 - 4.0).abs() < 0.001,
        "SQRT(16.0) should be 4.0, got {r3}"
    );
}
