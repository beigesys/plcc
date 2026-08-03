// SPDX-License-Identifier: MPL-2.0

//! Tests for newly added standard library functions: trig, rounding, type conversions.

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

fn read_f64(state: &[u8], offset: usize) -> f64 {
    f64::from_ne_bytes([
        state[offset],
        state[offset + 1],
        state[offset + 2],
        state[offset + 3],
        state[offset + 4],
        state[offset + 5],
        state[offset + 6],
        state[offset + 7],
    ])
}

fn read_i16(state: &[u8], offset: usize) -> i16 {
    i16::from_ne_bytes([state[offset], state[offset + 1]])
}

fn read_i32(state: &[u8], offset: usize) -> i32 {
    i32::from_ne_bytes([
        state[offset],
        state[offset + 1],
        state[offset + 2],
        state[offset + 3],
    ])
}

// ==================== Trig functions ====================

#[test]
fn test_tan() {
    let source = r#"
PROGRAM TanTest
VAR
    result : REAL;
END_VAR
    result := TAN(0.0);
END_PROGRAM
"#;
    let state = compile_and_run(source, "tantest_scan", 4, 1);
    let result = read_f32(&state, 0);
    assert!(
        result.abs() < 0.001,
        "TAN(0.0) should be ~0.0, got {result}"
    );
}

#[test]
fn test_asin_acos() {
    let source = r#"
PROGRAM AsinAcosTest
VAR
    asin_result : REAL;
    acos_result : REAL;
END_VAR
    asin_result := ASIN(1.0);
    acos_result := ACOS(1.0);
END_PROGRAM
"#;
    let state = compile_and_run(source, "asinacostest_scan", 8, 1);
    let asin_val = read_f32(&state, 0);
    let acos_val = read_f32(&state, 4);
    let pi_half: f32 = std::f32::consts::FRAC_PI_2;
    assert!(
        (asin_val - pi_half).abs() < 0.001,
        "ASIN(1.0) should be ~pi/2 ({pi_half}), got {asin_val}"
    );
    assert!(
        acos_val.abs() < 0.001,
        "ACOS(1.0) should be ~0.0, got {acos_val}"
    );
}

#[test]
fn test_atan_atan2() {
    let source = r#"
PROGRAM AtanTest
VAR
    atan_result : REAL;
    atan2_result : REAL;
END_VAR
    atan_result := ATAN(1.0);
    atan2_result := ATAN2(1.0, 1.0);
END_PROGRAM
"#;
    let state = compile_and_run(source, "atantest_scan", 8, 1);
    let atan_val = read_f32(&state, 0);
    let atan2_val = read_f32(&state, 4);
    let pi_quarter: f32 = std::f32::consts::FRAC_PI_4;
    assert!(
        (atan_val - pi_quarter).abs() < 0.001,
        "ATAN(1.0) should be ~pi/4 ({pi_quarter}), got {atan_val}"
    );
    assert!(
        (atan2_val - pi_quarter).abs() < 0.001,
        "ATAN2(1.0, 1.0) should be ~pi/4 ({pi_quarter}), got {atan2_val}"
    );
}

#[test]
fn test_log10() {
    let source = r#"
PROGRAM LogTest
VAR
    log10 : REAL;
    log100 : REAL;
END_VAR
    log10 := LOG(10.0);
    log100 := LOG(100.0);
END_PROGRAM
"#;
    let state = compile_and_run(source, "logtest_scan", 8, 1);
    let log10_val = read_f32(&state, 0);
    let log100_val = read_f32(&state, 4);
    assert!(
        (log10_val - 1.0).abs() < 0.001,
        "LOG(10.0) should be ~1.0, got {log10_val}"
    );
    assert!(
        (log100_val - 2.0).abs() < 0.001,
        "LOG(100.0) should be ~2.0, got {log100_val}"
    );
}

// ==================== Rounding functions ====================

#[test]
fn test_floor_ceil_round() {
    let source = r#"
PROGRAM RoundingTest
VAR
    floor_result : REAL;
    ceil_result : REAL;
    round_result : REAL;
END_VAR
    floor_result := FLOOR(3.7);
    ceil_result := CEIL(3.2);
    round_result := ROUND(3.5);
END_PROGRAM
"#;
    let state = compile_and_run(source, "roundingtest_scan", 12, 1);
    let floor_val = read_f32(&state, 0);
    let ceil_val = read_f32(&state, 4);
    let round_val = read_f32(&state, 8);
    assert!(
        (floor_val - 3.0).abs() < 0.001,
        "FLOOR(3.7) should be 3.0, got {floor_val}"
    );
    assert!(
        (ceil_val - 4.0).abs() < 0.001,
        "CEIL(3.2) should be 4.0, got {ceil_val}"
    );
    assert!(
        (round_val - 4.0).abs() < 0.001,
        "ROUND(3.5) should be 4.0, got {round_val}"
    );
}

// ==================== Integer widening (sext) ====================

#[test]
fn test_sint_to_int() {
    // SINT is i8, INT is i16. Use two INT vars to avoid layout complexity.
    // We assign a small value via INT, narrow it to SINT range, then widen back.
    let source = r#"
PROGRAM SintToIntTest
VAR
    input_val : SINT;
    result : INT;
END_VAR
    input_val := 5;
    result := SINT_TO_INT(input_val);
END_PROGRAM
"#;
    // State: { i8(SINT), pad(1), i16(INT) } = 4 bytes
    let state = compile_and_run(source, "sinttointtest_scan", 4, 1);
    let result = read_i16(&state, 2);
    assert_eq!(result, 5, "SINT_TO_INT(5) should be 5, got {result}");
}

#[test]
fn test_int_to_dint() {
    let source = r#"
PROGRAM IntToDintTest
VAR
    result : DINT;
END_VAR
    result := INT_TO_DINT(1000);
END_PROGRAM
"#;
    let state = compile_and_run(source, "inttodinttest_scan", 4, 1);
    let result = read_i32(&state, 0);
    assert_eq!(
        result, 1000,
        "INT_TO_DINT(1000) should be 1000, got {result}"
    );
}

// ==================== Integer narrowing (trunc) ====================

#[test]
fn test_dint_to_int_narrow() {
    let source = r#"
PROGRAM DintToIntTest
VAR
    input_val : DINT;
    result : INT;
END_VAR
    input_val := 42;
    result := DINT_TO_INT(input_val);
END_PROGRAM
"#;
    // State: { i32(DINT), i16(INT), pad(2) } = 8 bytes
    let state = compile_and_run(source, "dinttointtest_scan", 8, 1);
    let result = read_i16(&state, 4);
    assert_eq!(result, 42, "DINT_TO_INT(42) should be 42, got {result}");
}

// ==================== Byte/Word conversions ====================

#[test]
fn test_byte_to_int() {
    let source = r#"
PROGRAM ByteToIntTest
VAR
    input_val : BYTE;
    result : INT;
END_VAR
    input_val := 200;
    result := BYTE_TO_INT(input_val);
END_PROGRAM
"#;
    // State: { i8(BYTE), pad(1), i16(INT) } = 4 bytes
    let state = compile_and_run(source, "bytetointtest_scan", 4, 1);
    let result = read_i16(&state, 2);
    assert_eq!(result, 200, "BYTE_TO_INT(200) should be 200, got {result}");
}

// ==================== Float conversions ====================

#[test]
fn test_real_to_lreal() {
    let source = r#"
PROGRAM RealToLrealTest
VAR
    result : LREAL;
END_VAR
    result := REAL_TO_LREAL(3.14);
END_PROGRAM
"#;
    let state = compile_and_run(source, "realtolrealtest_scan", 8, 1);
    let result = read_f64(&state, 0);
    assert!(
        (result - 3.14).abs() < 0.01,
        "REAL_TO_LREAL(3.14) should be ~3.14, got {result}"
    );
}

// ==================== Bool conversions ====================

#[test]
fn test_bool_to_dint() {
    let source = r#"
PROGRAM BoolToDintTest
VAR
    result : DINT;
END_VAR
    result := BOOL_TO_DINT(TRUE);
END_PROGRAM
"#;
    let state = compile_and_run(source, "booltodinttest_scan", 4, 1);
    let result = read_i32(&state, 0);
    assert_eq!(result, 1, "BOOL_TO_DINT(TRUE) should be 1, got {result}");
}

// ==================== Chained conversions ====================

#[test]
fn test_multiple_conversions_chained() {
    let source = r#"
PROGRAM ChainedTest
VAR
    byte_val : BYTE;
    int_result : INT;
    dint_result : DINT;
END_VAR
    byte_val := 200;
    int_result := BYTE_TO_INT(byte_val);
    dint_result := INT_TO_DINT(int_result);
END_PROGRAM
"#;
    // State: { i8(BYTE), pad(1), i16(INT), i32(DINT) } = 8 bytes
    let state = compile_and_run(source, "chainedtest_scan", 8, 1);
    let int_result = read_i16(&state, 2);
    let dint_result = read_i32(&state, 4);
    assert_eq!(
        int_result, 200,
        "BYTE_TO_INT(200) should be 200, got {int_result}"
    );
    assert_eq!(
        dint_result, 200,
        "INT_TO_DINT(200) should be 200, got {dint_result}"
    );
}
