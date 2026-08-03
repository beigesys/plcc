// SPDX-License-Identifier: MPL-2.0

//! Coverage completion tests: REPEAT/UNTIL, MOD, EXIT, function return values,
//! composed functions, multi-program files, stress counters, and edge cases.

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

#[allow(dead_code)]
fn read_f32(state: &[u8], offset: usize) -> f32 {
    f32::from_ne_bytes([
        state[offset],
        state[offset + 1],
        state[offset + 2],
        state[offset + 3],
    ])
}

// ---------------------------------------------------------------------------
// 1. REPEAT/UNTIL loop (do-while semantics)
// ---------------------------------------------------------------------------
#[test]
fn repeat_until_loop() {
    let source = r#"
PROGRAM RepeatTest
VAR
    count : INT := 0;
END_VAR
    count := 0;
    REPEAT
        count := count + 1;
    UNTIL count >= 7
    END_REPEAT;
END_PROGRAM
"#;
    // State: { count: i16 } = 2 bytes
    let state = jit_run(source, "repeattest_scan", 2, 1);
    let count = read_i16(&state, 0);
    assert_eq!(count, 7, "REPEAT should count until 7, got {count}");
}

// ---------------------------------------------------------------------------
// 2. MOD operator
// ---------------------------------------------------------------------------
#[test]
fn mod_operator() {
    let source = r#"
PROGRAM ModTest
VAR
    a : INT := 0;
    b : INT := 0;
END_VAR
    a := 17 MOD 5;
    b := 10 MOD 3;
END_PROGRAM
"#;
    // State: { a: i16, b: i16 } = 4 bytes
    let state = jit_run(source, "modtest_scan", 4, 1);
    let a = read_i16(&state, 0);
    let b = read_i16(&state, 2);
    assert_eq!(a, 2, "17 MOD 5 should be 2, got {a}");
    assert_eq!(b, 1, "10 MOD 3 should be 1, got {b}");
}

// ---------------------------------------------------------------------------
// 3. EXIT from FOR loop
// ---------------------------------------------------------------------------
#[test]
fn exit_from_loop() {
    let source = r#"
PROGRAM ExitTest
VAR
    i : INT;
    sum : INT := 0;
END_VAR
    sum := 0;
    FOR i := 1 TO 100 DO
        IF i > 10 THEN
            EXIT;
        END_IF;
        sum := sum + i;
    END_FOR;
END_PROGRAM
"#;
    // State: { i: i16, sum: i16 } = 4 bytes
    // sum = 1+2+...+10 = 55 (EXIT fires when i=11, before adding 11)
    let state = jit_run(source, "exittest_scan", 4, 1);
    let sum = read_i16(&state, 2);
    assert_eq!(
        sum, 55,
        "sum of 1..10 with EXIT at 11 should be 55, got {sum}"
    );
}

// ---------------------------------------------------------------------------
// 4. Function with return value (clamp pattern)
// ---------------------------------------------------------------------------
#[test]
fn return_from_function() {
    let source = r#"
FUNCTION Clamp : INT
VAR_INPUT
    val : INT;
    lo : INT;
    hi : INT;
END_VAR
    IF val < lo THEN
        Clamp := lo;
    ELSIF val > hi THEN
        Clamp := hi;
    ELSE
        Clamp := val;
    END_IF;
END_FUNCTION

PROGRAM ClampTest
VAR
    a : INT;
    b : INT;
    c : INT;
END_VAR
    a := Clamp(5, 0, 10);
    b := Clamp(-5, 0, 10);
    c := Clamp(15, 0, 10);
END_PROGRAM
"#;
    // State: { a: i16, b: i16, c: i16 } = 6 bytes
    let state = jit_run(source, "clamptest_scan", 6, 1);
    let a = read_i16(&state, 0);
    let b = read_i16(&state, 2);
    let c = read_i16(&state, 4);
    assert_eq!(a, 5, "Clamp(5, 0, 10) should be 5, got {a}");
    assert_eq!(b, 0, "Clamp(-5, 0, 10) should be 0, got {b}");
    assert_eq!(c, 10, "Clamp(15, 0, 10) should be 10, got {c}");
}

// ---------------------------------------------------------------------------
// 5. Multiple functions composed: Inc, Dbl, IncDbl
// ---------------------------------------------------------------------------
#[test]
fn multiple_functions_composed() {
    let source = r#"
FUNCTION Inc : INT
VAR_INPUT
    x : INT;
END_VAR
    Inc := x + 1;
END_FUNCTION

FUNCTION Dbl : INT
VAR_INPUT
    x : INT;
END_VAR
    Dbl := x + x;
END_FUNCTION

FUNCTION IncDbl : INT
VAR_INPUT
    x : INT;
END_VAR
    IncDbl := Dbl(Inc(x));
END_FUNCTION

PROGRAM ComposeTest
VAR
    r : INT;
END_VAR
    r := IncDbl(4);
END_PROGRAM
"#;
    // Inc(4) = 5, Dbl(5) = 10
    // State: { r: i16 } = 2 bytes
    let state = jit_run(source, "composetest_scan", 2, 1);
    let r = read_i16(&state, 0);
    assert_eq!(r, 10, "IncDbl(4) = Dbl(Inc(4)) = Dbl(5) = 10, got {r}");
}

// ---------------------------------------------------------------------------
// 6. Multi-program file: two PROGRAMs in one source
// ---------------------------------------------------------------------------
#[test]
fn multi_program_file_a() {
    let source = r#"
PROGRAM ProgramA
VAR
    a : INT := 0;
END_VAR
    a := 42;
END_PROGRAM

PROGRAM ProgramB
VAR
    b : INT := 0;
END_VAR
    b := 99;
END_PROGRAM
"#;
    // Test ProgramA: state = { a: i16 } = 2 bytes
    let state = jit_run(source, "programa_scan", 2, 1);
    let a = read_i16(&state, 0);
    assert_eq!(a, 42, "ProgramA should set a=42, got {a}");
}

#[test]
fn multi_program_file_b() {
    let source = r#"
PROGRAM ProgramA
VAR
    a : INT := 0;
END_VAR
    a := 42;
END_PROGRAM

PROGRAM ProgramB
VAR
    b : INT := 0;
END_VAR
    b := 99;
END_PROGRAM
"#;
    // Test ProgramB: state = { b: i16 } = 2 bytes
    let state = jit_run(source, "programb_scan", 2, 1);
    let b = read_i16(&state, 0);
    assert_eq!(b, 99, "ProgramB should set b=99, got {b}");
}

// ---------------------------------------------------------------------------
// 7. Large scan count stress test: 10000 scans
// ---------------------------------------------------------------------------
#[test]
fn large_scan_count_stress() {
    let source = r#"
PROGRAM Stress
VAR
    count : INT := 0;
END_VAR
    count := count + 1;
END_PROGRAM
"#;
    // State: { count: i16 } = 2 bytes
    // INT range is -32768..32767, so 10000 is fine
    let state = jit_run(source, "stress_scan", 2, 10000);
    let count = read_i16(&state, 0);
    assert_eq!(
        count, 10000,
        "after 10000 scans count should be 10000, got {count}"
    );
}

// ---------------------------------------------------------------------------
// 8. Integer edge cases: INT max and min values
// ---------------------------------------------------------------------------
#[test]
fn integer_edge_cases() {
    let source = r#"
PROGRAM EdgeCases
VAR
    a : INT;
    b : INT;
END_VAR
    a := 32767;
    b := -32768;
END_PROGRAM
"#;
    // State: { a: i16, b: i16 } = 4 bytes
    let state = jit_run(source, "edgecases_scan", 4, 1);
    let a = read_i16(&state, 0);
    let b = read_i16(&state, 2);
    assert_eq!(a, 32767, "INT max should be 32767, got {a}");
    assert_eq!(b, -32768, "INT min should be -32768, got {b}");
}
