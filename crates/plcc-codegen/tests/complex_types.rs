// SPDX-License-Identifier: MPL-2.0

//! Tests for array indexing, struct member access, and multi-file compilation.

use inkwell::context::Context;
use inkwell::OptimizationLevel;
use plcc_codegen::Compiler;

fn jit_run(source: &str, scan_fn_name: &str, state_size: usize, num_scans: usize) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");

    let context = Context::create();
    let mut compiler = Compiler::new(&context, "test");
    compiler.compile(&unit).expect("codegen failed");

    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT");

    let mut state = vec![0u8; state_size];
    let state_ptr = state.as_mut_ptr();

    let fn_ptr = ee
        .get_function_address(scan_fn_name)
        .expect("function not found");

    let scan: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(fn_ptr) };
    for _ in 0..num_scans {
        scan(state_ptr);
    }

    state
}

fn jit_run_merged(sources: &[&str], scan_fn_name: &str, state_size: usize, num_scans: usize) -> Vec<u8> {
    let mut all_declarations = Vec::new();

    for source in sources {
        let (unit, errors) = plcc_st::parse(source);
        assert!(errors.is_empty(), "parse errors: {errors:?}");
        all_declarations.extend(unit.declarations);
    }

    let merged = plcc_st::ast::CompilationUnit {
        declarations: all_declarations,
        span: plcc_st::span::Span::empty(),
    };

    let context = Context::create();
    let mut compiler = Compiler::new(&context, "test");
    compiler.compile(&merged).expect("codegen failed");

    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT");

    let mut state = vec![0u8; state_size];
    let state_ptr = state.as_mut_ptr();

    let fn_ptr = ee
        .get_function_address(scan_fn_name)
        .expect("function not found");

    let scan: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(fn_ptr) };
    for _ in 0..num_scans {
        scan(state_ptr);
    }

    state
}

fn read_i16(state: &[u8], offset: usize) -> i16 {
    i16::from_ne_bytes([state[offset], state[offset + 1]])
}

#[test]
fn array_read_write() {
    // ARRAY[0..4] OF INT = [5 x i16] = 10 bytes
    // i : INT = 2 bytes
    // result : INT = 2 bytes
    // Total state: 10 + 2 + 2 = 14 bytes
    let source = r#"
PROGRAM ArrTest
VAR
    arr : ARRAY[0..4] OF INT;
    i : INT;
    result : INT;
END_VAR
    arr[0] := 10;
    arr[1] := 20;
    arr[2] := 30;
    result := arr[1];
END_PROGRAM
"#;

    let state = jit_run(source, "arrtest_scan", 14, 1);
    // arr is [5 x i16] at offset 0, i at offset 10, result at offset 12
    let result = read_i16(&state, 12);
    assert_eq!(result, 20, "arr[1] should be 20, got {result}");

    // Also verify the array contents
    let arr0 = read_i16(&state, 0);
    let arr1 = read_i16(&state, 2);
    let arr2 = read_i16(&state, 4);
    assert_eq!(arr0, 10, "arr[0] should be 10, got {arr0}");
    assert_eq!(arr1, 20, "arr[1] should be 20, got {arr1}");
    assert_eq!(arr2, 30, "arr[2] should be 30, got {arr2}");
}

#[test]
fn array_loop_sum() {
    // ARRAY[0..4] OF INT = [5 x i16] = 10 bytes
    // i : INT = 2 bytes
    // sum : INT = 2 bytes
    // Total state: 10 + 2 + 2 = 14 bytes
    let source = r#"
PROGRAM ArrSum
VAR
    arr : ARRAY[0..4] OF INT;
    i : INT;
    sum : INT;
END_VAR
    arr[0] := 1;
    arr[1] := 2;
    arr[2] := 3;
    arr[3] := 4;
    arr[4] := 5;
    sum := 0;
    FOR i := 0 TO 4 DO
        sum := sum + arr[i];
    END_FOR;
END_PROGRAM
"#;

    let state = jit_run(source, "arrsum_scan", 14, 1);
    // sum is at offset 12
    let sum = read_i16(&state, 12);
    assert_eq!(sum, 15, "sum of arr[0..4] should be 15, got {sum}");
}

#[test]
fn multi_file_merge() {
    // File 1: defines a function
    let source1 = r#"
FUNCTION AddTwo : INT
VAR_INPUT
    x : INT;
END_VAR
    AddTwo := x + 2;
END_FUNCTION
"#;

    // File 2: defines a program that calls the function from file 1
    let source2 = r#"
PROGRAM Caller
VAR
    result : INT;
END_VAR
    result := AddTwo(5);
END_PROGRAM
"#;

    // State struct: { i16 } = 2 bytes
    let state = jit_run_merged(&[source1, source2], "caller_scan", 2, 1);
    let result = read_i16(&state, 0);
    assert_eq!(result, 7, "AddTwo(5) should be 7, got {result}");
}

#[test]
fn array_with_nonzero_lower_bound() {
    // ARRAY[1..5] — lower bound is 1, not 0
    // [5 x i16] = 10 bytes, result : INT = 2 bytes = 12 total
    let source = r#"
PROGRAM ArrOneBase
VAR
    arr : ARRAY[1..5] OF INT;
    result : INT;
END_VAR
    arr[1] := 100;
    arr[2] := 200;
    arr[3] := 300;
    result := arr[2];
END_PROGRAM
"#;

    let state = jit_run(source, "arronebase_scan", 12, 1);
    let result = read_i16(&state, 10);
    assert_eq!(result, 200, "arr[2] should be 200, got {result}");
}

#[test]
fn array_write_in_loop() {
    // Initialize array elements in a loop, then sum them
    // ARRAY[0..9] OF INT = [10 x i16] = 20 bytes
    // i : INT = 2, sum : INT = 2
    // Total: 24 bytes
    let source = r#"
PROGRAM ArrLoop
VAR
    arr : ARRAY[0..9] OF INT;
    i : INT;
    sum : INT;
END_VAR
    FOR i := 0 TO 9 DO
        arr[i] := i * 2;
    END_FOR;
    sum := 0;
    FOR i := 0 TO 9 DO
        sum := sum + arr[i];
    END_FOR;
END_PROGRAM
"#;

    let state = jit_run(source, "arrloop_scan", 24, 1);
    // sum at offset 22
    let sum = read_i16(&state, 22);
    // 0+2+4+6+8+10+12+14+16+18 = 90
    assert_eq!(sum, 90, "sum of i*2 for i=0..9 should be 90, got {sum}");
}
