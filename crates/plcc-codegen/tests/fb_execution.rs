// SPDX-License-Identifier: MPL-2.0

//! Tests for function block (FB) instantiation and calling.

use inkwell::context::Context;
use inkwell::OptimizationLevel;
use plcc_codegen::Compiler;

/// Helper: compile, JIT execute with init + multiple scans, return state bytes.
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

    // Call _init() to apply variable initializers
    let init_fn_name = scan_fn_name.replace("_scan", "_init");
    if let Ok(init_ptr) = ee.get_function_address(&init_fn_name) {
        let init: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(init_ptr) };
        init(state_ptr);
    }

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

// ---------------------------------------------------------------------------
// Test 1: Simple FB call — Counter FB increments, read output
// ---------------------------------------------------------------------------
#[test]
fn simple_fb_call() {
    let source = r#"
FUNCTION_BLOCK Counter
VAR_INPUT
    reset : BOOL;
END_VAR
VAR_OUTPUT
    count : INT;
END_VAR
VAR
    cnt : INT := 0;
END_VAR
    IF reset THEN
        cnt := 0;
    ELSE
        cnt := cnt + 1;
    END_IF;
    count := cnt;
END_FUNCTION_BLOCK

PROGRAM Main
VAR
    my_counter : Counter;
    result : INT;
END_VAR
    my_counter(reset := FALSE);
    result := my_counter.count;
END_PROGRAM
"#;

    // Counter struct: { i8 (reset/BOOL), i16 (count/INT), i16 (cnt/INT) }
    // With padding: i8 + 1 pad + i16 + i16 = 6 bytes
    // Program struct: { Counter(6 bytes), i16 (result) } = 8 bytes
    // But actual LLVM layout may differ, so use a generous buffer
    let state = jit_run(source, "main_scan", 64, 5);

    // The 'result' field is the last i16 in the program struct.
    // Counter struct = { i8, i16, i16 } with alignment.
    // Let's find result by searching for it. With LLVM struct packing:
    // Counter: offset 0 = reset (i8), offset 2 = count (i16), offset 4 = cnt (i16) -> 6 bytes
    // Program: offset 0 = Counter (6 bytes, padded to 6), offset 6 = result (i16) -> 8 bytes
    let result = read_i16(&state, 6);
    assert_eq!(result, 5, "after 5 scans, count should be 5, got {result}");
}

// ---------------------------------------------------------------------------
// Test 2: FB with reset — reset after 3 scans
// ---------------------------------------------------------------------------
#[test]
fn fb_with_reset() {
    // We need a way to control the reset input between scans.
    // Strategy: Use a counter variable to determine when to reset.
    let source = r#"
FUNCTION_BLOCK Counter
VAR_INPUT
    reset : BOOL;
END_VAR
VAR_OUTPUT
    count : INT;
END_VAR
VAR
    cnt : INT := 0;
END_VAR
    IF reset THEN
        cnt := 0;
    ELSE
        cnt := cnt + 1;
    END_IF;
    count := cnt;
END_FUNCTION_BLOCK

PROGRAM ResetTest
VAR
    my_counter : Counter;
    scan_num : INT;
    do_reset : BOOL;
    result : INT;
END_VAR
    scan_num := scan_num + 1;
    IF scan_num = 4 THEN
        do_reset := TRUE;
    ELSE
        do_reset := FALSE;
    END_IF;
    my_counter(reset := do_reset);
    result := my_counter.count;
END_PROGRAM
"#;

    // Counter struct: { i8 (reset), i16 (count), i16 (cnt) } = 6 bytes
    // Program struct: { Counter(6), i16(scan_num), i8(do_reset), pad, i16(result) }
    // After 6 scans:
    //   scan 1: cnt=1, scan 2: cnt=2, scan 3: cnt=3
    //   scan 4: reset=TRUE, cnt=0
    //   scan 5: cnt=1, scan 6: cnt=2
    //   result should be 2
    let state = jit_run(source, "resettest_scan", 64, 6);

    // Find result offset — it's the last field. With LLVM packing this may vary.
    // Let's just check from different offsets and print diagnostics.
    // Counter: 6 bytes, scan_num: i16 = 2 bytes, do_reset: i8 = 1 byte, result: i16 = 2 bytes
    // Offsets: Counter @ 0-5, scan_num @ 6-7, do_reset @ 8, result @ 10-11
    let result = read_i16(&state, 10);
    assert_eq!(
        result, 2,
        "after 6 scans (3 normal + 1 reset + 2 normal), count should be 2, got {result}"
    );
}

// ---------------------------------------------------------------------------
// Test 3: Multiple FB instances
// ---------------------------------------------------------------------------
#[test]
fn multiple_fb_instances() {
    let source = r#"
FUNCTION_BLOCK Counter
VAR_INPUT
    reset : BOOL;
END_VAR
VAR_OUTPUT
    count : INT;
END_VAR
VAR
    cnt : INT := 0;
END_VAR
    IF reset THEN
        cnt := 0;
    ELSE
        cnt := cnt + 1;
    END_IF;
    count := cnt;
END_FUNCTION_BLOCK

PROGRAM TwoCounters
VAR
    a : Counter;
    b : Counter;
    sum : INT;
END_VAR
    a(reset := FALSE);
    b(reset := FALSE);
    sum := a.count + b.count;
END_PROGRAM
"#;

    // Counter struct: 6 bytes each
    // Program: { Counter_a(6), Counter_b(6), i16(sum) } = 14 bytes
    let state = jit_run(source, "twocounters_scan", 64, 4);

    // sum is at offset 12 (after two 6-byte Counter structs)
    let sum = read_i16(&state, 12);
    assert_eq!(sum, 8, "after 4 scans, both counters at 4, sum should be 8, got {sum}");
}

// ---------------------------------------------------------------------------
// Test 4: FB chained — output of one feeds input of another
// ---------------------------------------------------------------------------
#[test]
fn fb_chained() {
    let source = r#"
FUNCTION_BLOCK Counter
VAR_INPUT
    reset : BOOL;
END_VAR
VAR_OUTPUT
    count : INT;
END_VAR
VAR
    cnt : INT := 0;
END_VAR
    IF reset THEN
        cnt := 0;
    ELSE
        cnt := cnt + 1;
    END_IF;
    count := cnt;
END_FUNCTION_BLOCK

FUNCTION_BLOCK Doubler
VAR_INPUT
    val : INT;
END_VAR
VAR_OUTPUT
    result : INT;
END_VAR
    result := val + val;
END_FUNCTION_BLOCK

PROGRAM Chain
VAR
    c : Counter;
    d : Doubler;
    output : INT;
END_VAR
    c(reset := FALSE);
    d(val := c.count);
    output := d.result;
END_PROGRAM
"#;

    // Counter struct: { i8 (reset), i16 (count), i16 (cnt) } = 6 bytes
    // Doubler struct: { i16 (val), i16 (result) } = 4 bytes
    // Program: { Counter(6), Doubler(4), i16(output) } = 12 bytes
    let state = jit_run(source, "chain_scan", 64, 5);

    // output is at offset 10 (after Counter 6 bytes + Doubler 4 bytes)
    let output = read_i16(&state, 10);
    assert_eq!(output, 10, "after 5 scans, count=5, doubled=10, got {output}");
}

// ---------------------------------------------------------------------------
// Test 5: FB with timer pattern
// ---------------------------------------------------------------------------
#[test]
fn fb_with_timer_pattern() {
    let source = r#"
FUNCTION_BLOCK SimpleTimer
VAR_INPUT
    enable : BOOL;
    preset : INT;
END_VAR
VAR_OUTPUT
    done : BOOL;
END_VAR
VAR
    elapsed : INT := 0;
END_VAR
    IF enable THEN
        elapsed := elapsed + 1;
        IF elapsed >= preset THEN
            done := TRUE;
        END_IF;
    ELSE
        elapsed := 0;
        done := FALSE;
    END_IF;
END_FUNCTION_BLOCK

PROGRAM TimerTest
VAR
    t : SimpleTimer;
    output : INT;
END_VAR
    t(enable := TRUE, preset := 10);
    IF t.done THEN
        output := 1;
    ELSE
        output := 0;
    END_IF;
END_PROGRAM
"#;

    // SimpleTimer struct: { i8 (enable), i16 (preset), i8 (done), i16 (elapsed) } = 8 bytes
    // Actually LLVM may arrange: i8, pad, i16, i8, pad, i16 = 8 bytes
    // Program: { SimpleTimer(8), i16(output) } = 10 bytes

    // After 9 scans: elapsed=9, done=FALSE, output=0
    let state9 = jit_run(source, "timertest_scan", 64, 9);
    // After 10 scans: elapsed=10, done=TRUE, output=1
    let state10 = jit_run(source, "timertest_scan", 64, 10);

    // Find output offset — it's the last i16 in the program struct
    // SimpleTimer: { i8(enable), pad, i16(preset), i8(done), pad, i16(elapsed) } = 8 bytes
    // Program: { SimpleTimer(8), i16(output) } -> output at offset 8
    let output9 = read_i16(&state9, 8);
    let output10 = read_i16(&state10, 8);
    assert_eq!(output9, 0, "after 9 scans, timer not done, output should be 0, got {output9}");
    assert_eq!(output10, 1, "after 10 scans, timer done, output should be 1, got {output10}");
}
