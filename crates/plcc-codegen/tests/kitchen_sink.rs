// SPDX-License-Identifier: MPL-2.0

//! Kitchen-sink execution tests: complex ST programs JIT-executed and verified.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
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

fn read_i16(state: &[u8], offset: usize) -> i16 {
    i16::from_ne_bytes([state[offset], state[offset + 1]])
}

fn read_f32(state: &[u8], offset: usize) -> f32 {
    f32::from_ne_bytes([
        state[offset],
        state[offset + 1],
        state[offset + 2],
        state[offset + 3],
    ])
}

#[test]
fn nested_if_elsif() {
    let source = r#"
PROGRAM NestedIf
VAR
    x : INT := 0;
    category : INT := 0;
END_VAR
    x := x + 1;
    IF x <= 3 THEN
        category := 1;
    ELSIF x <= 6 THEN
        category := 2;
    ELSIF x <= 9 THEN
        category := 3;
    ELSE
        category := 4;
    END_IF;
END_PROGRAM
"#;
    // After 2 scans: x=2, category=1
    let state = jit_run(source, "nestedif_scan", 4, 2);
    assert_eq!(read_i16(&state, 0), 2);
    assert_eq!(read_i16(&state, 2), 1);

    // After 5 scans: x=5, category=2
    let state = jit_run(source, "nestedif_scan", 4, 5);
    assert_eq!(read_i16(&state, 0), 5);
    assert_eq!(read_i16(&state, 2), 2);

    // After 8 scans: x=8, category=3
    let state = jit_run(source, "nestedif_scan", 4, 8);
    assert_eq!(read_i16(&state, 0), 8);
    assert_eq!(read_i16(&state, 2), 3);

    // After 11 scans: x=11, category=4
    let state = jit_run(source, "nestedif_scan", 4, 11);
    assert_eq!(read_i16(&state, 0), 11);
    assert_eq!(read_i16(&state, 2), 4);
}

#[test]
fn fibonacci_for_loop() {
    let source = r#"
PROGRAM Fib
VAR
    n : INT := 0;
    a : INT := 0;
    b : INT := 1;
    temp : INT := 0;
    i : INT;
END_VAR
    a := 0;
    b := 1;
    FOR i := 1 TO 10 DO
        temp := a + b;
        a := b;
        b := temp;
    END_FOR;
    n := b;
END_PROGRAM
"#;
    // Fibonacci: after 10 iterations, b = fib(11) = 89
    let state = jit_run(source, "fib_scan", 10, 1);
    let n = read_i16(&state, 0);
    assert_eq!(n, 89, "fib(11) should be 89, got {n}");
}

#[test]
fn gcd_while_loop() {
    let source = r#"
PROGRAM GCD
VAR
    a : INT := 0;
    b : INT := 0;
    temp : INT := 0;
END_VAR
    (* Compute GCD of 48 and 18 using Euclidean algorithm *)
    a := 48;
    b := 18;
    WHILE b > 0 DO
        temp := b;
        b := a MOD b;
        a := temp;
    END_WHILE;
END_PROGRAM
"#;
    let state = jit_run(source, "gcd_scan", 6, 1);
    let a = read_i16(&state, 0);
    assert_eq!(a, 6, "GCD(48, 18) should be 6, got {a}");
}

#[test]
fn nested_loops() {
    let source = r#"
PROGRAM NestedLoops
VAR
    sum : INT := 0;
    i : INT;
    j : INT;
END_VAR
    sum := 0;
    FOR i := 1 TO 5 DO
        FOR j := 1 TO i DO
            sum := sum + 1;
        END_FOR;
    END_FOR;
END_PROGRAM
"#;
    // 1 + 2 + 3 + 4 + 5 = 15
    let state = jit_run(source, "nestedloops_scan", 6, 1);
    let sum = read_i16(&state, 0);
    assert_eq!(sum, 15, "nested loop sum should be 15, got {sum}");
}

#[test]
fn case_with_ranges() {
    let source = r#"
PROGRAM CaseRange
VAR
    input : INT := 0;
    output : INT := 0;
END_VAR
    input := input + 1;
    CASE input OF
        1..3:
            output := 100;
        4..6:
            output := 200;
        7..9:
            output := 300;
    ELSE
        output := 0;
    END_CASE;
END_PROGRAM
"#;
    let state = jit_run(source, "caserange_scan", 4, 2);
    assert_eq!(read_i16(&state, 2), 100, "input=2 should match 1..3");

    let state = jit_run(source, "caserange_scan", 4, 5);
    assert_eq!(read_i16(&state, 2), 200, "input=5 should match 4..6");

    let state = jit_run(source, "caserange_scan", 4, 8);
    assert_eq!(read_i16(&state, 2), 300, "input=8 should match 7..9");

    let state = jit_run(source, "caserange_scan", 4, 11);
    assert_eq!(read_i16(&state, 2), 0, "input=11 should fall to ELSE");
}

#[test]
fn boolean_logic() {
    let source = r#"
PROGRAM BoolLogic
VAR
    a : INT := 0;
    b : INT := 0;
    c : INT := 0;
    d : INT := 0;
END_VAR
    (* Test AND: 5 > 3 AND 10 > 7 -> TRUE *)
    IF (5 > 3) AND (10 > 7) THEN
        a := 1;
    ELSE
        a := 0;
    END_IF;

    (* Test OR: 1 > 3 OR 10 > 7 -> TRUE *)
    IF (1 > 3) OR (10 > 7) THEN
        b := 1;
    ELSE
        b := 0;
    END_IF;

    (* Test NOT AND: NOT (5 > 3 AND 1 > 10) -> TRUE *)
    IF NOT ((5 > 3) AND (1 > 10)) THEN
        c := 1;
    ELSE
        c := 0;
    END_IF;

    (* Test FALSE AND: 1 > 3 AND 1 > 10 -> FALSE *)
    IF (1 > 3) AND (1 > 10) THEN
        d := 1;
    ELSE
        d := 0;
    END_IF;
END_PROGRAM
"#;
    let state = jit_run(source, "boollogic_scan", 8, 1);
    assert_eq!(read_i16(&state, 0), 1, "AND(true, true) should be true");
    assert_eq!(read_i16(&state, 2), 1, "OR(false, true) should be true");
    assert_eq!(
        read_i16(&state, 4),
        1,
        "NOT(AND(true, false)) should be true"
    );
    assert_eq!(read_i16(&state, 6), 0, "AND(false, false) should be false");
}

#[test]
fn float_arithmetic() {
    let source = r#"
PROGRAM FloatMath
VAR
    a : REAL := 0.0;
    b : REAL := 0.0;
    c : REAL := 0.0;
    d : REAL := 0.0;
END_VAR
    a := 3.14 + 2.86;
    b := 10.0 - 3.5;
    c := 2.5 * 4.0;
    d := 15.0 / 3.0;
END_PROGRAM
"#;
    // REAL = f32, 4 bytes each, struct = 16 bytes
    // But our codegen uses f64 for literals... let me check
    // Actually REAL maps to f32 in iec_to_llvm_type, but RealLiteral is f64
    // This should work via float truncation on store
    let state = jit_run(source, "floatmath_scan", 16, 1);
    let a = read_f32(&state, 0);
    let b = read_f32(&state, 4);
    let c = read_f32(&state, 8);
    let d = read_f32(&state, 12);
    assert!((a - 6.0).abs() < 0.01, "3.14 + 2.86 = {a}");
    assert!((b - 6.5).abs() < 0.01, "10.0 - 3.5 = {b}");
    assert!((c - 10.0).abs() < 0.01, "2.5 * 4.0 = {c}");
    assert!((d - 5.0).abs() < 0.01, "15.0 / 3.0 = {d}");
}

#[test]
fn multi_scan_accumulator() {
    let source = r#"
PROGRAM Accum
VAR
    total : INT := 0;
    scan_count : INT := 0;
END_VAR
    scan_count := scan_count + 1;
    total := total + scan_count;
END_PROGRAM
"#;
    // After 10 scans: scan_count=10, total=1+2+3+...+10=55
    let state = jit_run(source, "accum_scan", 4, 10);
    let total = read_i16(&state, 0);
    let scan_count = read_i16(&state, 2);
    assert_eq!(scan_count, 10, "scan_count should be 10");
    assert_eq!(total, 55, "total should be 55 (sum 1..10)");
}
