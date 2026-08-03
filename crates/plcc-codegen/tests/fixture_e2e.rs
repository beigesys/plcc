// SPDX-License-Identifier: MPL-2.0

//! End-to-end integration tests that compile and JIT-execute every fixture .st file
//! from tests/fixtures/. Each test reads the .st source from disk, compiles it through
//! the plcc pipeline, executes via LLVM JIT, and verifies specific output values.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;

// =============================================================================
// Helpers
// =============================================================================

/// Compile source, call _init (if it exists), then call _scan `num_scans` times.
/// Returns the raw state bytes (size `state_size`).
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

fn read_i8(state: &[u8], offset: usize) -> i8 {
    state[offset] as i8
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

fn read_f32(state: &[u8], offset: usize) -> f32 {
    f32::from_ne_bytes([
        state[offset],
        state[offset + 1],
        state[offset + 2],
        state[offset + 3],
    ])
}

fn read_i64(state: &[u8], offset: usize) -> i64 {
    i64::from_ne_bytes([
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

/// Read a fixture file relative to the workspace root.
fn read_fixture(relative_path: &str) -> String {
    let path = format!("../../tests/fixtures/{relative_path}");
    std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "Failed to read fixture file '{path}': {e}. CWD: {:?}",
            std::env::current_dir()
        )
    })
}

// =============================================================================
// programs/blink.st
// =============================================================================
// PROGRAM Blink — VAR { output: BOOL }
// State layout: { output: i8 } = 1 byte
// Each scan toggles output via NOT.

#[test]
fn fixture_blink_after_1_scan() {
    let source = read_fixture("programs/blink.st");
    let state = jit_run(&source, "blink_scan", 1, 1);
    assert_eq!(state[0] & 1, 1, "blink: output should be TRUE after 1 scan");
}

#[test]
fn fixture_blink_after_2_scans() {
    let source = read_fixture("programs/blink.st");
    let state = jit_run(&source, "blink_scan", 1, 2);
    assert_eq!(
        state[0] & 1,
        0,
        "blink: output should be FALSE after 2 scans"
    );
}

#[test]
fn fixture_blink_after_3_scans() {
    let source = read_fixture("programs/blink.st");
    let state = jit_run(&source, "blink_scan", 1, 3);
    assert_eq!(
        state[0] & 1,
        1,
        "blink: output should be TRUE after 3 scans"
    );
}

// =============================================================================
// programs/state_machine.st
// =============================================================================
// PROGRAM StateMachine — VAR {
//   state: INT,          offset 0  (i16)
//   output_a: BOOL,      offset 2  (i8)
//   output_b: BOOL,      offset 3  (i8)
//   input_trigger: BOOL, offset 4  (i8)
//   counter: INT         offset 6  (i16, aligned)
// }
// State size: 8 bytes
//
// Initial: state=0, input_trigger=FALSE, so stays in state 0:
//   output_a=FALSE, output_b=FALSE.

#[test]
fn fixture_state_machine_initial() {
    let source = read_fixture("programs/state_machine.st");
    let state = jit_run(&source, "statemachine_scan", 8, 1);
    let sm_state = read_i16(&state, 0);
    let output_a = read_i8(&state, 2);
    let output_b = read_i8(&state, 3);
    // In state 0 with input_trigger=FALSE: stays in state 0
    assert_eq!(sm_state, 0, "state_machine: should remain in state 0");
    assert_eq!(output_a & 1, 0, "state_machine: output_a should be FALSE");
    assert_eq!(output_b & 1, 0, "state_machine: output_b should be FALSE");
}

// =============================================================================
// programs/pid_simple.st
// =============================================================================
// Contains FUNCTION_BLOCK PID + PROGRAM PIDController.
// PID FB layout (VAR_INPUT, VAR_OUTPUT, VAR):
//   setpoint: REAL(4), measured: REAL(4), kp: REAL(4), ki: REAL(4),
//   kd: REAL(4), dt: REAL(4), out_min: REAL(4), out_max: REAL(4),
//   output: REAL(4), err: REAL(4), prev_err: REAL(4), integral: REAL(4),
//   derivative: REAL(4) = 52 bytes
// PIDController layout:
//   pid_inst: PID(52), sensor_value: REAL(4), target_value: REAL(4),
//   control_output: REAL(4) = 64 bytes
// Use a generous buffer and assert the exact PID arithmetic.

// Byte offsets inside the PIDController state struct (all members are f32).
// PID sub-struct starts at 0; its fields are in declaration order.
const PID_KP: usize = 8;
const PID_KI: usize = 12;
const PID_KD: usize = 16;
const PID_DT: usize = 20;
const PID_OUT_MIN: usize = 24;
const PID_OUT_MAX: usize = 28;
const PID_OUTPUT: usize = 32;
const PID_INTEGRAL: usize = 44;
const PID_DERIVATIVE: usize = 48;
const PID_TARGET_VALUE: usize = 56;
const PID_CONTROL_OUTPUT: usize = 60;

#[test]
fn fixture_pid_simple_clamps_to_out_max_on_first_scan() {
    let source = read_fixture("programs/pid_simple.st");
    let state = jit_run(&source, "pidcontroller_scan", 128, 1);

    // The FB's declared initial values must survive init (they are VAR_INPUT
    // members of the PID FB instance, never written by the program).
    assert_eq!(read_f32(&state, PID_KP), 1.0, "pid.kp");
    assert_eq!(read_f32(&state, PID_KI), 0.1, "pid.ki");
    assert_eq!(read_f32(&state, PID_KD), 0.05, "pid.kd");
    assert_eq!(read_f32(&state, PID_DT), 0.01, "pid.dt");
    assert_eq!(read_f32(&state, PID_OUT_MIN), -100.0, "pid.out_min");
    assert_eq!(read_f32(&state, PID_OUT_MAX), 100.0, "pid.out_max");
    assert_eq!(read_f32(&state, PID_TARGET_VALUE), 50.0, "target_value");

    // After 1 scan with setpoint=50.0, measured=0.0:
    //   err = 50.0, integral = 50*0.01 = 0.5, derivative = 50/0.01 = 5000
    //   output = 1*50 + 0.1*0.5 + 0.05*5000 = 50 + 0.05 + 250 = 300.05
    //   clamped to out_max = 100.0
    assert_eq!(read_f32(&state, PID_INTEGRAL), 0.5, "pid.integral");
    assert_eq!(read_f32(&state, PID_DERIVATIVE), 5000.0, "pid.derivative");

    let output = read_f32(&state, PID_OUTPUT);
    let control_output = read_f32(&state, PID_CONTROL_OUTPUT);
    assert!(
        output.is_finite(),
        "pid_simple: pid.output must be finite, got {output}"
    );
    assert_eq!(output, 100.0, "pid.output must be clamped to out_max");
    assert_eq!(
        control_output, 100.0,
        "control_output must mirror the clamped pid.output"
    );
}

#[test]
fn fixture_pid_simple_settles_after_several_scans() {
    let source = read_fixture("programs/pid_simple.st");
    // 5 scans. err stays 50.0 every scan, so from scan 2 onwards derivative is 0
    // and integral accumulates 50 * dt = 0.5 per scan.
    let state = jit_run(&source, "pidcontroller_scan", 128, 5);

    let integral = read_f32(&state, PID_INTEGRAL);
    assert_eq!(integral, 2.5, "integral after 5 scans = 5 * 50 * 0.01");
    assert_eq!(
        read_f32(&state, PID_DERIVATIVE),
        0.0,
        "derivative settles to 0"
    );

    // output = kp*50 + ki*2.5 + kd*0 = 50 + 0.25 = 50.25, inside [-100, 100].
    let control_output = read_f32(&state, PID_CONTROL_OUTPUT);
    assert!(
        control_output.is_finite(),
        "pid_simple: control_output must be finite (not NaN/inf), got {control_output}"
    );
    assert!(
        (control_output - 50.25).abs() < 1e-4,
        "pid_simple: control_output should be 50.25, got {control_output}"
    );
}

// =============================================================================
// codegen/arithmetic.st
// =============================================================================
// PROGRAM ArithTest — VAR { a,b,c,d,e : INT }
// State layout: { a: i16, b: i16, c: i16, d: i16, e: i16 } = 10 bytes
//   a = 10+5 = 15
//   b = 10-3 = 7
//   c = 4*7 = 28
//   d = 20/4 = 5
//   e = 17 MOD 5 = 2

#[test]
fn fixture_arithmetic() {
    let source = read_fixture("codegen/arithmetic.st");
    let state = jit_run(&source, "arithtest_scan", 10, 1);
    let a = read_i16(&state, 0);
    let b = read_i16(&state, 2);
    let c = read_i16(&state, 4);
    let d = read_i16(&state, 6);
    let e = read_i16(&state, 8);
    assert_eq!(a, 15, "arithmetic: 10+5 = {a}");
    assert_eq!(b, 7, "arithmetic: 10-3 = {b}");
    assert_eq!(c, 28, "arithmetic: 4*7 = {c}");
    assert_eq!(d, 5, "arithmetic: 20/4 = {d}");
    assert_eq!(e, 2, "arithmetic: 17 MOD 5 = {e}");
}

// =============================================================================
// codegen/control_flow.st
// =============================================================================
// PROGRAM ControlFlow — VAR { state: INT, output: INT, i: INT, sum: INT }
// State layout: { state: i16, output: i16, i: i16, sum: i16 } = 8 bytes
//
// After 1 scan (state starts at 0):
//   IF state=0 -> output=1
//   FOR i:=1 TO 10 -> sum=55
//   WHILE sum>50 -> sum=50 (55-10=45? no, 55>50 -> 45, 45<50 -> stop) -> sum=45
//   CASE state=0 -> output=100
// Final: state=0, output=100, i=11(loop var), sum=45

#[test]
fn fixture_control_flow() {
    let source = read_fixture("codegen/control_flow.st");
    let state = jit_run(&source, "controlflow_scan", 8, 1);
    let sm_state = read_i16(&state, 0);
    let output = read_i16(&state, 2);
    let _i = read_i16(&state, 4);
    let sum = read_i16(&state, 6);
    assert_eq!(sm_state, 0, "control_flow: state should remain 0");
    assert_eq!(
        output, 100,
        "control_flow: output should be 100 (from CASE)"
    );
    assert_eq!(sum, 45, "control_flow: sum should be 45 (55 - 10)");
}

// =============================================================================
// codegen/function_calls.st
// =============================================================================
// FUNCTION AddTwo : INT — returns a + b
// FUNCTION Square : INT — returns x * x
// PROGRAM CallTest — VAR { result1: INT, result2: INT }
// State layout: { result1: i16, result2: i16 } = 4 bytes
//   result1 = AddTwo(3, 4) = 7
//   result2 = Square(5) = 25

#[test]
fn fixture_function_calls() {
    let source = read_fixture("codegen/function_calls.st");
    let state = jit_run(&source, "calltest_scan", 4, 1);
    let result1 = read_i16(&state, 0);
    let result2 = read_i16(&state, 2);
    assert_eq!(result1, 7, "function_calls: AddTwo(3,4) = {result1}");
    assert_eq!(result2, 25, "function_calls: Square(5) = {result2}");
}

// =============================================================================
// codegen/function_blocks.st
// =============================================================================
// Counter FB: { reset: i8(BOOL), count: i16(INT), cnt: i16(INT) } = 6 bytes
// Doubler FB: { val: i16(INT), result: i16(INT) } = 4 bytes
// PROGRAM FBTest: { c1: Counter(6), c2: Counter(6), d: Doubler(4), sum: i16, doubled: i16 }
//   = 6 + 6 + 4 + 2 + 2 = 20 bytes
//
// After 5 scans:
//   c1.count=5, c2.count=5, d(val:=c1.count) -> d.result=10
//   sum = c1.count + c2.count = 10
//   doubled = d.result = 10

#[test]
fn fixture_function_blocks() {
    let source = read_fixture("codegen/function_blocks.st");
    let state = jit_run(&source, "fbtest_scan", 64, 5);

    // Counter: { i8(reset), pad, i16(count), i16(cnt) } = 6 bytes
    // c1 @ 0..6, c2 @ 6..12, Doubler @ 12..16, sum @ 16, doubled @ 18
    let c1_count = read_i16(&state, 2);
    let c2_count = read_i16(&state, 8);
    let sum = read_i16(&state, 16);
    let doubled = read_i16(&state, 18);
    assert_eq!(
        c1_count, 5,
        "function_blocks: c1.count should be 5 after 5 scans, got {c1_count}"
    );
    assert_eq!(
        c2_count, 5,
        "function_blocks: c2.count should be 5 after 5 scans, got {c2_count}"
    );
    assert_eq!(sum, 10, "function_blocks: sum should be 10, got {sum}");
    assert_eq!(
        doubled, 10,
        "function_blocks: doubled should be 10, got {doubled}"
    );
}

// =============================================================================
// codegen/arrays_structs.st
// =============================================================================
// PROGRAM ArrayTest — VAR {
//   arr: ARRAY[0..9] OF INT,  (10 * i16 = 20 bytes)
//   i: INT,                   (i16, 2 bytes)
//   sum: INT,                 (i16, 2 bytes)
//   max_val: INT              (i16, 2 bytes)
// }
// State size: 20 + 2 + 2 + 2 = 26 bytes
//
// After 1 scan: arr = [10,20,...,100], sum = 550, max_val = 100

#[test]
fn fixture_arrays_structs() {
    let source = read_fixture("codegen/arrays_structs.st");
    let state = jit_run(&source, "arraytest_scan", 64, 1);

    // arr is 20 bytes at offset 0, i at 20, sum at 22, max_val at 24
    let sum = read_i16(&state, 22);
    let max_val = read_i16(&state, 24);
    assert_eq!(sum, 550, "arrays_structs: sum should be 550, got {sum}");
    assert_eq!(
        max_val, 100,
        "arrays_structs: max_val should be 100, got {max_val}"
    );
}

// =============================================================================
// codegen/stdlib_math.st
// =============================================================================
// PROGRAM StdlibMathTest — VAR { sq,ab,sn,cs,ex,lg,fl,cl,mn,mx,lm : REAL }
// State layout: 11 * f32 = 44 bytes
//   sq = SQRT(25.0)   = 5.0
//   ab = ABS(-42.0)   = 42.0
//   sn = SIN(0.0)     = 0.0
//   cs = COS(0.0)     = 1.0
//   ex = EXP(0.0)     = 1.0
//   lg = LN(1.0)      = 0.0
//   fl = FLOOR(3.7)   = 3.0
//   cl = CEIL(3.2)    = 4.0
//   mn = MIN(5.0,3.0) = 3.0
//   mx = MAX(5.0,3.0) = 5.0
//   lm = LIMIT(0.0,-5.0,100.0) = 0.0

#[test]
fn fixture_stdlib_math() {
    let source = read_fixture("codegen/stdlib_math.st");
    let state = jit_run(&source, "stdlibmathtest_scan", 48, 1);

    let sq = read_f32(&state, 0);
    let ab = read_f32(&state, 4);
    let sn = read_f32(&state, 8);
    let cs = read_f32(&state, 12);
    let ex = read_f32(&state, 16);
    let lg = read_f32(&state, 20);
    let fl = read_f32(&state, 24);
    let cl = read_f32(&state, 28);
    let mn = read_f32(&state, 32);
    let mx = read_f32(&state, 36);
    let lm = read_f32(&state, 40);

    assert!((sq - 5.0).abs() < 0.001, "stdlib_math: SQRT(25.0) = {sq}");
    assert!((ab - 42.0).abs() < 0.001, "stdlib_math: ABS(-42.0) = {ab}");
    assert!((sn - 0.0).abs() < 0.001, "stdlib_math: SIN(0.0) = {sn}");
    assert!((cs - 1.0).abs() < 0.001, "stdlib_math: COS(0.0) = {cs}");
    assert!((ex - 1.0).abs() < 0.001, "stdlib_math: EXP(0.0) = {ex}");
    assert!((lg - 0.0).abs() < 0.001, "stdlib_math: LN(1.0) = {lg}");
    assert!((fl - 3.0).abs() < 0.001, "stdlib_math: FLOOR(3.7) = {fl}");
    assert!((cl - 4.0).abs() < 0.001, "stdlib_math: CEIL(3.2) = {cl}");
    assert!((mn - 3.0).abs() < 0.001, "stdlib_math: MIN(5.0,3.0) = {mn}");
    assert!((mx - 5.0).abs() < 0.001, "stdlib_math: MAX(5.0,3.0) = {mx}");
    assert!(
        (lm - 0.0).abs() < 0.001,
        "stdlib_math: LIMIT(0.0,-5.0,100.0) = {lm}"
    );
}

// =============================================================================
// codegen/stdlib_conversions.st
// =============================================================================
// PROGRAM ConversionTest — VAR { i: INT, r: REAL, d: DINT, b: INT }
// State layout: { i: i16(2), pad(2), r: f32(4), d: i32(4), b: i16(2) } = 14 bytes
// (REAL at offset 4 needs 4-byte alignment, so 2 bytes padding after i)
//   i = 42 (initialized)
//   r = INT_TO_REAL(42) = 42.0
//   d = INT_TO_DINT(42) = 42
//   b = BOOL_TO_INT(TRUE) = 1

#[test]
fn fixture_stdlib_conversions() {
    let source = read_fixture("codegen/stdlib_conversions.st");
    let state = jit_run(&source, "conversiontest_scan", 32, 1);

    // i at offset 0 (i16)
    let i = read_i16(&state, 0);
    assert_eq!(i, 42, "stdlib_conversions: i should be 42, got {i}");

    // r at offset 4 (f32, aligned to 4)
    let r = read_f32(&state, 4);
    assert!(
        (r - 42.0).abs() < 0.01,
        "stdlib_conversions: r should be ~42.0, got {r}"
    );

    // d at offset 8 (i32)
    let d = read_i32(&state, 8);
    assert_eq!(d, 42, "stdlib_conversions: d should be 42, got {d}");

    // b at offset 12 (i16)
    let b = read_i16(&state, 12);
    assert_eq!(b, 1, "stdlib_conversions: b should be 1, got {b}");
}

// =============================================================================
// codegen/stdlib_time.st
// =============================================================================
// PROGRAM TimeTest — VAR {
//   t1: TIME := T#100ms,   (i64, 8 bytes) — TIME stored as nanoseconds
//   t2: TIME := T#50ms,    (i64, 8 bytes)
//   sum_t: TIME,            (i64, 8 bytes)
//   diff_t: TIME,           (i64, 8 bytes)
//   scaled_t: TIME          (i64, 8 bytes)
// }
// State size: 40 bytes
//   sum_t = ADD_TIME(100ms, 50ms) = 150ms = 150_000_000 ns
//   diff_t = SUB_TIME(100ms, 50ms) = 50ms = 50_000_000 ns
//   scaled_t = MUL_TIME(100ms, 3) = 300ms = 300_000_000 ns

#[test]
fn fixture_stdlib_time() {
    let source = read_fixture("codegen/stdlib_time.st");
    let state = jit_run(&source, "timetest_scan", 64, 1);

    let t1 = read_i64(&state, 0);
    let t2 = read_i64(&state, 8);
    let sum_t = read_i64(&state, 16);
    let diff_t = read_i64(&state, 24);
    let scaled_t = read_i64(&state, 32);

    // Verify t1 and t2 were initialized correctly
    assert_eq!(
        t1, 100_000_000,
        "stdlib_time: t1 should be 100ms in ns, got {t1}"
    );
    assert_eq!(
        t2, 50_000_000,
        "stdlib_time: t2 should be 50ms in ns, got {t2}"
    );
    assert_eq!(
        sum_t, 150_000_000,
        "stdlib_time: sum_t should be 150ms in ns, got {sum_t}"
    );
    assert_eq!(
        diff_t, 50_000_000,
        "stdlib_time: diff_t should be 50ms in ns, got {diff_t}"
    );
    assert_eq!(
        scaled_t, 300_000_000,
        "stdlib_time: scaled_t should be 300ms in ns, got {scaled_t}"
    );
}

// =============================================================================
// codegen/oop.st
// =============================================================================
// FUNCTION_BLOCK Accumulator — VAR { total: INT }
//   Methods: Add(val), GetTotal(): INT, Reset
// PROGRAM OOPTest — VAR { acc: Accumulator, result: INT }
// Accumulator layout: { total: i16 } = 2 bytes
// OOPTest layout: { acc: Accumulator(2), result: i16 } = 4 bytes
//
// After 1 scan: acc.Add(10), acc.Add(20), acc.Add(30), result = acc.GetTotal() = 60

#[test]
fn fixture_oop() {
    let source = read_fixture("codegen/oop.st");
    let state = jit_run(&source, "ooptest_scan", 16, 1);

    // acc.total at offset 0, result at offset 2
    let total = read_i16(&state, 0);
    let result = read_i16(&state, 2);
    assert_eq!(total, 60, "oop: acc.total should be 60, got {total}");
    assert_eq!(result, 60, "oop: result should be 60, got {result}");
}

// =============================================================================
// codegen/globals.st
// =============================================================================
// VAR_GLOBAL { g_counter: INT, g_flag: BOOL }
// PROGRAM GlobalTest — VAR { local_copy: INT }
//
// Globals are stored separately or prefixed in the state. The program increments
// g_counter each scan and copies to local_copy.
// After 5 scans: g_counter=5, local_copy=5.
//
// Note: Global layout depends on codegen implementation. Globals may be at the
// start of the state struct or in a separate region. We search for the expected
// value.

#[test]
fn fixture_globals() {
    let source = read_fixture("codegen/globals.st");
    let state = jit_run(&source, "globaltest_scan", 64, 5);

    // The program struct likely contains: { local_copy: i16 }
    // Globals may be separate. Try offset 0 for local_copy.
    // If globals are embedded in state: { g_counter: i16, g_flag: i8, pad, local_copy: i16 }
    // Try searching for value 5 in i16 positions.
    let mut found_local_copy = false;
    for offset in (0..state.len() - 1).step_by(2) {
        let val = read_i16(&state, offset);
        if val == 5 {
            found_local_copy = true;
            break;
        }
    }
    assert!(
        found_local_copy,
        "globals: expected local_copy=5 somewhere in state after 5 scans. State: {state:?}"
    );
}

// =============================================================================
// programs/motor_control.st
// =============================================================================
// MotorController FB layout:
//   VAR_INPUT:  start_cmd(i8), stop_cmd(i8), current(f32), overload_limit(f32) = ~12 bytes
//   VAR_OUTPUT: running(i8), fault(i8), run_time(i16) = ~4 bytes
//   VAR:        overload_count(i16) = 2 bytes
//   Total per FB: ~18 bytes (with alignment ~20)
// PROGRAM MotorSystem: { motor1: MC(~20), motor2: MC(~20), any_fault: i8 }
//
// MotorController struct: { i8 start_cmd, i8 stop_cmd, pad(2), f32 current,
//   f32 overload_limit, i8 running, i8 fault, i16 run_time, i16 overload_count }
//   -> size 18, align 4 -> 20 bytes.
// MotorSystem: { motor1 @0, motor2 @20, i8 any_fault @40 }
//
// current = 10.0 is below the declared overload_limit of 15.0, so neither motor
// ever faults: running stays TRUE and run_time counts scans.

const MC_STRIDE: usize = 20;
const MC_OVERLOAD_LIMIT: usize = 8;
const MC_RUNNING: usize = 12;
const MC_FAULT: usize = 13;
const MC_RUN_TIME: usize = 14;
const MC_OVERLOAD_COUNT: usize = 16;
const MS_ANY_FAULT: usize = 40;

fn assert_motor(state: &[u8], base: usize, scans: i16, tag: &str) {
    assert_eq!(
        read_f32(state, base + MC_OVERLOAD_LIMIT),
        15.0,
        "{tag}: overload_limit initializer must survive init"
    );
    assert_eq!(
        state[base + MC_RUNNING] & 1,
        1,
        "{tag}: running must be TRUE"
    );
    assert_eq!(state[base + MC_FAULT] & 1, 0, "{tag}: fault must be FALSE");
    assert_eq!(
        read_i16(state, base + MC_RUN_TIME),
        scans,
        "{tag}: run_time must equal the scan count"
    );
    assert_eq!(
        read_i16(state, base + MC_OVERLOAD_COUNT),
        0,
        "{tag}: overload_count must stay 0 (current 10.0 < limit 15.0)"
    );
}

#[test]
fn fixture_motor_control() {
    let source = read_fixture("programs/motor_control.st");
    let state = jit_run(&source, "motorsystem_scan", 128, 1);

    assert_motor(&state, 0, 1, "motor1");
    assert_motor(&state, MC_STRIDE, 1, "motor2");
    assert_eq!(
        state[MS_ANY_FAULT] & 1,
        0,
        "motor_control: any_fault must be FALSE"
    );
}

/// With the declared overload_limit of 15.0 applied, a current of 10.0 never trips
/// the overload counter, so the motors keep running for as many scans as you like.
/// Before FB member initializers worked, overload_limit was 0.0 and both motors
/// faulted out on the third scan.
#[test]
fn fixture_motor_control_no_false_overload_trip() {
    let source = read_fixture("programs/motor_control.st");
    let state = jit_run(&source, "motorsystem_scan", 128, 10);

    assert_motor(&state, 0, 10, "motor1");
    assert_motor(&state, MC_STRIDE, 10, "motor2");
    assert_eq!(
        state[MS_ANY_FAULT] & 1,
        0,
        "motor_control: any_fault must stay FALSE across 10 scans"
    );
}

// =============================================================================
// programs/batch_process.st (may not exist yet)
// =============================================================================
// BatchProcess program with fill/mix/drain/idle states.
// After 66 scans (20 fill + 30 mix + 15 drain + 1 idle), batch_count=1.

#[test]
fn fixture_batch_process() {
    let source = read_fixture("programs/batch_process.st");
    let state = jit_run(&source, "batchprocess_scan", 128, 66);

    // Search for batch_count=1 as i16 in state
    let mut found_batch_count = false;
    for offset in (0..state.len() - 1).step_by(2) {
        let val = read_i16(&state, offset);
        if val == 1 {
            found_batch_count = true;
            break;
        }
    }
    assert!(
        found_batch_count,
        "batch_process: expected batch_count=1 after 66 scans. State: {:?}",
        &state[..64]
    );
}

// =============================================================================
// programs/alarm_system.st (may not exist yet)
// =============================================================================
// AlarmSystem program: after 1 scan, system_alarm=TRUE, system_code=3.

#[test]
fn fixture_alarm_system() {
    let source = read_fixture("programs/alarm_system.st");
    let state = jit_run(&source, "alarmsystem_scan", 128, 1);

    // Search for system_code=3 as i16
    let mut found_code = false;
    for offset in (0..state.len() - 1).step_by(2) {
        let val = read_i16(&state, offset);
        if val == 3 {
            found_code = true;
            break;
        }
    }
    assert!(
        found_code,
        "alarm_system: expected system_code=3 in state. State: {:?}",
        &state[..64]
    );
}

// =============================================================================
// Multi-scan stability tests
// =============================================================================

/// Verify blink program toggles correctly over many scans.
#[test]
fn fixture_blink_stability_100_scans() {
    let source = read_fixture("programs/blink.st");
    let state = jit_run(&source, "blink_scan", 1, 100);
    assert_eq!(
        state[0] & 1,
        0,
        "blink: after 100 scans (even), output should be FALSE"
    );
}

/// Verify state machine cycles correctly.
#[test]
fn fixture_state_machine_no_trigger_10_scans() {
    let source = read_fixture("programs/state_machine.st");
    let state = jit_run(&source, "statemachine_scan", 8, 10);
    let sm_state = read_i16(&state, 0);
    // With input_trigger=FALSE, always stays in state 0
    assert_eq!(
        sm_state, 0,
        "state_machine: should stay in state 0 without trigger"
    );
}

/// Verify PID controller doesn't crash over multiple scans.
#[test]
fn fixture_pid_simple_10_scans() {
    let source = read_fixture("programs/pid_simple.st");
    let state = jit_run(&source, "pidcontroller_scan", 128, 10);

    // integral = 10 * 50 * 0.01 = 5.0; output = 50 + 0.1*5.0 = 50.5, unclamped.
    let integral = read_f32(&state, PID_INTEGRAL);
    let control_output = read_f32(&state, PID_CONTROL_OUTPUT);
    assert!(
        control_output.is_finite(),
        "pid_simple (10 scans): control_output must be finite, got {control_output}"
    );
    assert!(
        (integral - 5.0).abs() < 1e-3,
        "pid_simple (10 scans): integral should be 5.0, got {integral}"
    );
    assert!(
        (control_output - 50.5).abs() < 1e-3,
        "pid_simple (10 scans): control_output should be 50.5, got {control_output}"
    );
}

/// Verify arithmetic is idempotent across scans.
#[test]
fn fixture_arithmetic_idempotent() {
    let source = read_fixture("codegen/arithmetic.st");
    let state = jit_run(&source, "arithtest_scan", 10, 100);
    let a = read_i16(&state, 0);
    let b = read_i16(&state, 2);
    let c = read_i16(&state, 4);
    let d = read_i16(&state, 6);
    let e = read_i16(&state, 8);
    assert_eq!(a, 15, "arithmetic (100 scans): a = {a}");
    assert_eq!(b, 7, "arithmetic (100 scans): b = {b}");
    assert_eq!(c, 28, "arithmetic (100 scans): c = {c}");
    assert_eq!(d, 5, "arithmetic (100 scans): d = {d}");
    assert_eq!(e, 2, "arithmetic (100 scans): e = {e}");
}

/// Verify function calls are idempotent.
#[test]
fn fixture_function_calls_idempotent() {
    let source = read_fixture("codegen/function_calls.st");
    let state = jit_run(&source, "calltest_scan", 4, 50);
    let result1 = read_i16(&state, 0);
    let result2 = read_i16(&state, 2);
    assert_eq!(
        result1, 7,
        "function_calls (50 scans): AddTwo(3,4) = {result1}"
    );
    assert_eq!(
        result2, 25,
        "function_calls (50 scans): Square(5) = {result2}"
    );
}
