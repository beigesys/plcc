// SPDX-License-Identifier: MPL-2.0

//! End-to-end tests: compile ST → LLVM → JIT execute → verify outputs.

use inkwell::context::Context;
use inkwell::OptimizationLevel;
use plcc_codegen::Compiler;

/// Helper: compile source, JIT execute the scan function, return state bytes.
fn compile_and_run(source: &str, scan_fn_name: &str, state_size: usize, num_scans: usize) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");

    let context = Context::create();
    let mut compiler = Compiler::new(&context, "test");
    compiler.compile(&unit).expect("codegen failed");

    // Verify the IR is valid
    let ir = compiler.emit_ir();
    assert!(ir.contains(scan_fn_name), "scan function not found in IR:\n{ir}");

    // Use execution engine to JIT
    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT execution engine");

    // Allocate state struct (zeroed)
    let mut state = vec![0u8; state_size];
    let state_ptr = state.as_mut_ptr();

    // Get function pointer
    let fn_ptr = ee
        .get_function_address(scan_fn_name)
        .expect("failed to get function address");

    // Cast to fn(ptr) -> void and call
    let scan: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(fn_ptr) };
    for _ in 0..num_scans {
        scan(state_ptr);
    }

    state
}

#[test]
fn blink_toggles_output() {
    let source = r#"
PROGRAM Blink
VAR
    output : BOOL := FALSE;
END_VAR
    output := NOT output;
END_PROGRAM
"#;

    // State struct: { i1 } — but LLVM i1 in a struct is typically 1 byte
    // After 1 scan: output should be TRUE
    let state = compile_and_run(source, "blink_scan", 1, 1);
    assert_eq!(state[0] & 1, 1, "output should be TRUE after 1 scan");

    // After 2 scans: output should be FALSE
    let state = compile_and_run(source, "blink_scan", 1, 2);
    assert_eq!(state[0] & 1, 0, "output should be FALSE after 2 scans");

    // After 3 scans: output should be TRUE again
    let state = compile_and_run(source, "blink_scan", 1, 3);
    assert_eq!(state[0] & 1, 1, "output should be TRUE after 3 scans");
}

#[test]
fn state_machine_transitions() {
    let source = r#"
PROGRAM SM
VAR
    state : INT := 0;
    result : INT := 0;
END_VAR
    CASE state OF
        0:
            result := 10;
            state := 1;
        1:
            result := 20;
            state := 2;
        2:
            result := 30;
            state := 0;
    END_CASE;
END_PROGRAM
"#;

    // State struct: { i16, i16 } = 4 bytes
    // After 1 scan: state=1, result=10
    let state = compile_and_run(source, "sm_scan", 4, 1);
    let state_val = i16::from_ne_bytes([state[0], state[1]]);
    let result_val = i16::from_ne_bytes([state[2], state[3]]);
    assert_eq!(state_val, 1, "state should be 1 after scan 1");
    assert_eq!(result_val, 10, "result should be 10 after scan 1");

    // After 2 scans: state=2, result=20
    let state = compile_and_run(source, "sm_scan", 4, 2);
    let state_val = i16::from_ne_bytes([state[0], state[1]]);
    let result_val = i16::from_ne_bytes([state[2], state[3]]);
    assert_eq!(state_val, 2, "state should be 2 after scan 2");
    assert_eq!(result_val, 20, "result should be 20 after scan 2");

    // After 3 scans: state=0, result=30
    let state = compile_and_run(source, "sm_scan", 4, 3);
    let state_val = i16::from_ne_bytes([state[0], state[1]]);
    let result_val = i16::from_ne_bytes([state[2], state[3]]);
    assert_eq!(state_val, 0, "state should be 0 after scan 3");
    assert_eq!(result_val, 30, "result should be 30 after scan 3");
}

#[test]
fn for_loop_sums() {
    let source = r#"
PROGRAM ForSum
VAR
    sum : INT := 0;
    i : INT;
END_VAR
    sum := 0;
    FOR i := 1 TO 10 DO
        sum := sum + i;
    END_FOR;
END_PROGRAM
"#;

    // State struct: { i16, i16 } = 4 bytes
    let state = compile_and_run(source, "forsum_scan", 4, 1);
    let sum_val = i16::from_ne_bytes([state[0], state[1]]);
    assert_eq!(sum_val, 55, "sum of 1..10 should be 55, got {sum_val}");
}

#[test]
fn arithmetic_operations() {
    let source = r#"
PROGRAM Arith
VAR
    a : INT := 0;
    b : INT := 0;
    c : INT := 0;
    d : INT := 0;
END_VAR
    a := 10 + 5;
    b := 10 - 3;
    c := 4 * 7;
    d := 20 / 4;
END_PROGRAM
"#;

    // State struct: { i16, i16, i16, i16 } = 8 bytes
    let state = compile_and_run(source, "arith_scan", 8, 1);
    let a = i16::from_ne_bytes([state[0], state[1]]);
    let b = i16::from_ne_bytes([state[2], state[3]]);
    let c = i16::from_ne_bytes([state[4], state[5]]);
    let d = i16::from_ne_bytes([state[6], state[7]]);
    assert_eq!(a, 15, "10 + 5 = {a}");
    assert_eq!(b, 7, "10 - 3 = {b}");
    assert_eq!(c, 28, "4 * 7 = {c}");
    assert_eq!(d, 5, "20 / 4 = {d}");
}

#[test]
fn if_else_branching() {
    let source = r#"
PROGRAM IfElse
VAR
    x : INT := 0;
    result : INT := 0;
END_VAR
    x := x + 1;
    IF x > 5 THEN
        result := 1;
    ELSE
        result := 0;
    END_IF;
END_PROGRAM
"#;

    // State: { i16, i16 } = 4 bytes
    // After 3 scans: x=3, result=0 (x <= 5)
    let state = compile_and_run(source, "ifelse_scan", 4, 3);
    let x = i16::from_ne_bytes([state[0], state[1]]);
    let result = i16::from_ne_bytes([state[2], state[3]]);
    assert_eq!(x, 3, "x should be 3 after 3 scans");
    assert_eq!(result, 0, "result should be 0 when x <= 5");

    // After 6 scans: x=6, result=1 (x > 5)
    let state = compile_and_run(source, "ifelse_scan", 4, 6);
    let x = i16::from_ne_bytes([state[0], state[1]]);
    let result = i16::from_ne_bytes([state[2], state[3]]);
    assert_eq!(x, 6, "x should be 6 after 6 scans");
    assert_eq!(result, 1, "result should be 1 when x > 5");
}

#[test]
fn while_loop() {
    let source = r#"
PROGRAM WhileTest
VAR
    count : INT := 0;
END_VAR
    count := 0;
    WHILE count < 7 DO
        count := count + 1;
    END_WHILE;
END_PROGRAM
"#;

    let state = compile_and_run(source, "whiletest_scan", 2, 1);
    let count = i16::from_ne_bytes([state[0], state[1]]);
    assert_eq!(count, 7, "while loop should count to 7, got {count}");
}
