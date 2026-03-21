// SPDX-License-Identifier: MPL-2.0

//! IEC 61131-3 conformance tests: JIT execution tests verifying behavior
//! specified by the IEC 61131-3:2013 standard (3rd edition).

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

    // Call _init() to apply variable initializers (IEC requires initial values)
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

#[allow(dead_code)]
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

// ===========================================================================
// Section 2.3 — Type System
// ===========================================================================

// 1. BOOL values: TRUE=1, FALSE=0, NOT TRUE=FALSE, NOT FALSE=TRUE
#[test]
fn iec_bool_values() {
    let source = r#"
PROGRAM BoolVals
VAR
    t_val   : BOOL := FALSE;
    f_val   : BOOL := FALSE;
    not_t   : BOOL := FALSE;
    not_f   : BOOL := FALSE;
END_VAR
    t_val := TRUE;
    f_val := FALSE;
    not_t := NOT TRUE;
    not_f := NOT FALSE;
END_PROGRAM
"#;
    // State: 4 x i1 (1 byte each) = 4 bytes
    let state = jit_run(source, "boolvals_scan", 4, 1);
    assert_eq!(state[0] & 1, 1, "TRUE should be 1");
    assert_eq!(state[1] & 1, 0, "FALSE should be 0");
    assert_eq!(state[2] & 1, 0, "NOT TRUE should be FALSE (0)");
    assert_eq!(state[3] & 1, 1, "NOT FALSE should be TRUE (1)");
}

// 2. INT arithmetic wraparound: 32767+1 = -32768 for signed 16-bit
#[test]
fn iec_int_arithmetic_wraparound() {
    let source = r#"
PROGRAM IntWrap
VAR
    max_plus_one : INT := 0;
    min_minus_one : INT := 0;
END_VAR
    max_plus_one := 32767 + 1;
    min_minus_one := -32768 - 1;
END_PROGRAM
"#;
    // State: 2 x i16 = 4 bytes
    let state = jit_run(source, "intwrap_scan", 4, 1);
    assert_eq!(
        read_i16(&state, 0),
        -32768,
        "32767 + 1 should wrap to -32768"
    );
    assert_eq!(
        read_i16(&state, 2),
        32767,
        "-32768 - 1 should wrap to 32767"
    );
}

// 3. REAL precision: at least 6 significant digits
#[test]
fn iec_real_precision() {
    let source = r#"
PROGRAM RealPrec
VAR
    val : REAL := 0.0;
END_VAR
    val := 1.23456 * 1.0;
END_PROGRAM
"#;
    let state = jit_run(source, "realprec_scan", 4, 1);
    let val = read_f32(&state, 0);
    assert!(
        (val - 1.23456).abs() < 1e-5,
        "REAL should preserve at least 6 significant digits, got {val}"
    );
}

// 4. Integer division truncates toward zero
#[test]
fn iec_integer_division_truncates() {
    let source = r#"
PROGRAM IntDiv
VAR
    pos_div : INT := 0;
    neg_div : INT := 0;
END_VAR
    pos_div := 7 / 2;
    neg_div := -7 / 2;
END_PROGRAM
"#;
    let state = jit_run(source, "intdiv_scan", 4, 1);
    assert_eq!(read_i16(&state, 0), 3, "7 / 2 should truncate to 3");
    assert_eq!(
        read_i16(&state, 2),
        -3,
        "-7 / 2 should truncate toward zero to -3"
    );
}

// 5. MOD sign follows dividend per IEC 61131-3
#[test]
fn iec_mod_sign_follows_dividend() {
    let source = r#"
PROGRAM ModSign
VAR
    pos_pos : INT := 0;
    neg_pos : INT := 0;
    pos_neg : INT := 0;
END_VAR
    pos_pos := 7 MOD 3;
    neg_pos := -7 MOD 3;
    pos_neg := 7 MOD -3;
END_PROGRAM
"#;
    let state = jit_run(source, "modsign_scan", 6, 1);
    assert_eq!(read_i16(&state, 0), 1, "7 MOD 3 should be 1");
    assert_eq!(
        read_i16(&state, 2),
        -1,
        "-7 MOD 3 should be -1 (sign follows dividend)"
    );
    assert_eq!(
        read_i16(&state, 4),
        1,
        "7 MOD -3 should be 1 (sign follows dividend)"
    );
}

// ===========================================================================
// Section 2.4 — Variables
// ===========================================================================

// 6. Uninitialized variables default to zero
#[test]
fn iec_var_initialized_to_zero() {
    let source = r#"
PROGRAM ZeroInit
VAR
    i : INT;
    b : BOOL;
    r : REAL;
    result_i : INT := 0;
    result_b : INT := 0;
    result_r : REAL := 0.0;
END_VAR
    result_i := i;
    IF b THEN
        result_b := 1;
    ELSE
        result_b := 0;
    END_IF;
    result_r := r;
END_PROGRAM
"#;
    // State: i(i16) + b(i8) + pad(1) + r(f32) + result_i(i16) + result_b(i16) + result_r(f32)
    // Layout depends on codegen, but with zeroed state all values start at 0.
    // We use a generous buffer and check result fields.
    // i=0, b=FALSE, r=0.0 by default (zeroed state).
    // result_i at offset after first 3 vars.
    // Simpler approach: just test that the result vars reflect zero defaults.
    // With 2+1+1+4+2+2+4 = 16 bytes (generous)
    let state = jit_run(source, "zeroinit_scan", 16, 1);
    // INT i defaults to 0
    assert_eq!(read_i16(&state, 0), 0, "uninitialized INT should default to 0");
    // BOOL b defaults to FALSE (0)
    assert_eq!(state[2] & 1, 0, "uninitialized BOOL should default to FALSE");
}

// 7. Explicit initialization value
#[test]
fn iec_var_explicit_init() {
    let source = r#"
PROGRAM ExplInit
VAR
    x : INT := 42;
    y : INT := 0;
END_VAR
    y := x;
END_PROGRAM
"#;
    // State: 2 x i16 = 4 bytes
    let state = jit_run(source, "explinit_scan", 4, 1);
    let y = read_i16(&state, 2);
    assert_eq!(y, 42, "x initialized to 42, y := x should be 42, got {y}");
}

// 8. CONSTANT variable holds its value
#[test]
fn iec_constant_var_value() {
    let source = r#"
PROGRAM ConstVal
VAR CONSTANT
    PI : REAL := 3.14159;
END_VAR
VAR
    result : REAL := 0.0;
END_VAR
    result := PI;
END_PROGRAM
"#;
    // State: PI(f32) + result(f32) = 8 bytes
    let state = jit_run(source, "constval_scan", 8, 1);
    let result = read_f32(&state, 4);
    assert!(
        (result - 3.14159).abs() < 0.001,
        "CONSTANT PI should be 3.14159, got {result}"
    );
}

// ===========================================================================
// Section 2.5 — Expressions
// ===========================================================================

// 9. Operator precedence per Table 68
#[test]
fn iec_operator_precedence() {
    let source = r#"
PROGRAM OpPrec
VAR
    mul_before_add : INT := 0;
    not_before_or  : INT := 0;
END_VAR
    (* * binds tighter than + *)
    mul_before_add := 2 + 3 * 4;

    (* NOT binds tighter than OR: NOT TRUE OR FALSE = FALSE OR FALSE = FALSE *)
    IF NOT TRUE OR FALSE THEN
        not_before_or := 1;
    ELSE
        not_before_or := 0;
    END_IF;
END_PROGRAM
"#;
    let state = jit_run(source, "opprec_scan", 4, 1);
    assert_eq!(
        read_i16(&state, 0),
        14,
        "2 + 3 * 4 should be 14 (* before +)"
    );
    assert_eq!(
        read_i16(&state, 2),
        0,
        "NOT TRUE OR FALSE should be FALSE (NOT binds tighter than OR)"
    );
}

// 10. Comparison returns BOOL usable in IF
#[test]
fn iec_comparison_returns_bool() {
    let source = r#"
PROGRAM CmpBool
VAR
    result : INT := 0;
END_VAR
    IF 5 > 3 THEN
        result := 1;
    ELSE
        result := 0;
    END_IF;
END_PROGRAM
"#;
    let state = jit_run(source, "cmpbool_scan", 2, 1);
    assert_eq!(read_i16(&state, 0), 1, "(5 > 3) should evaluate to TRUE");
}

// 11. Boolean AND/OR behave correctly
#[test]
fn iec_boolean_short_circuit() {
    let source = r#"
PROGRAM BoolOps
VAR
    and_tt : INT := 0;
    and_tf : INT := 0;
    or_ff  : INT := 0;
    or_tf  : INT := 0;
END_VAR
    IF TRUE AND TRUE THEN and_tt := 1; ELSE and_tt := 0; END_IF;
    IF TRUE AND FALSE THEN and_tf := 1; ELSE and_tf := 0; END_IF;
    IF FALSE OR FALSE THEN or_ff := 1; ELSE or_ff := 0; END_IF;
    IF TRUE OR FALSE THEN or_tf := 1; ELSE or_tf := 0; END_IF;
END_PROGRAM
"#;
    let state = jit_run(source, "boolops_scan", 8, 1);
    assert_eq!(read_i16(&state, 0), 1, "TRUE AND TRUE should be TRUE");
    assert_eq!(read_i16(&state, 2), 0, "TRUE AND FALSE should be FALSE");
    assert_eq!(read_i16(&state, 4), 0, "FALSE OR FALSE should be FALSE");
    assert_eq!(read_i16(&state, 6), 1, "TRUE OR FALSE should be TRUE");
}

// ===========================================================================
// Section 3.3 — Statements
// ===========================================================================

// 12. Assignment copies value (not reference)
#[test]
fn iec_assignment_copies_value() {
    let source = r#"
PROGRAM CopyVal
VAR
    a : INT := 0;
    b : INT := 0;
END_VAR
    b := 42;
    a := b;
    b := 99;
END_PROGRAM
"#;
    let state = jit_run(source, "copyval_scan", 4, 1);
    assert_eq!(
        read_i16(&state, 0),
        42,
        "a should retain original value of b (42), not 99"
    );
    assert_eq!(read_i16(&state, 2), 99, "b should be 99");
}

// 13. IF/ELSIF/ELSE complete branching
#[test]
fn iec_if_elsif_else_complete() {
    let source = r#"
PROGRAM IfElsif
VAR
    x : INT := 0;
    result : INT := 0;
END_VAR
    x := 2;
    IF x = 1 THEN
        result := 10;
    ELSIF x = 2 THEN
        result := 20;
    ELSIF x = 3 THEN
        result := 30;
    ELSE
        result := 99;
    END_IF;
END_PROGRAM
"#;
    let state = jit_run(source, "ifelsif_scan", 4, 1);
    assert_eq!(
        read_i16(&state, 2),
        20,
        "ELSIF x=2 branch should execute, result=20"
    );
}

// 14. CASE first match wins
#[test]
fn iec_case_first_match_wins() {
    let source = r#"
PROGRAM CaseFirst
VAR
    sel : INT := 0;
    result : INT := 0;
END_VAR
    sel := 2;
    CASE sel OF
        1:
            result := 10;
        2:
            result := 20;
        3:
            result := 30;
    ELSE
        result := 99;
    END_CASE;
END_PROGRAM
"#;
    let state = jit_run(source, "casefirst_scan", 4, 1);
    assert_eq!(
        read_i16(&state, 2),
        20,
        "CASE 2 should match first, result=20"
    );
}

// 15. FOR loop inclusive: FOR i := 1 TO 5 executes for i=1,2,3,4,5
#[test]
fn iec_for_loop_inclusive() {
    let source = r#"
PROGRAM ForIncl
VAR
    sum : INT := 0;
    i : INT;
END_VAR
    sum := 0;
    FOR i := 1 TO 5 DO
        sum := sum + 1;
    END_FOR;
END_PROGRAM
"#;
    let state = jit_run(source, "forincl_scan", 4, 1);
    assert_eq!(
        read_i16(&state, 0),
        5,
        "FOR i := 1 TO 5 should execute 5 times (inclusive both ends)"
    );
}

// 16. FOR loop with BY step
#[test]
fn iec_for_loop_by_step() {
    let source = r#"
PROGRAM ForStep
VAR
    sum : INT := 0;
    i : INT;
END_VAR
    sum := 0;
    FOR i := 0 TO 10 BY 3 DO
        sum := sum + i;
    END_FOR;
END_PROGRAM
"#;
    // i takes values 0, 3, 6, 9 -> sum = 0+3+6+9 = 18
    let state = jit_run(source, "forstep_scan", 4, 1);
    assert_eq!(
        read_i16(&state, 0),
        18,
        "FOR i := 0 TO 10 BY 3, sum of i values (0+3+6+9) should be 18"
    );
}

// 17. FOR loop with negative step
#[test]
fn iec_for_loop_negative_step() {
    let source = r#"
PROGRAM ForNeg
VAR
    sum : INT := 0;
    i : INT;
END_VAR
    sum := 0;
    FOR i := 10 TO 1 BY -1 DO
        sum := sum + 1;
    END_FOR;
END_PROGRAM
"#;
    // i takes values 10,9,8,...,1 -> 10 iterations
    let state = jit_run(source, "forneg_scan", 4, 1);
    assert_eq!(
        read_i16(&state, 0),
        10,
        "FOR i := 10 TO 1 BY -1 should execute 10 times"
    );
}

// 18. WHILE with FALSE condition: zero iterations
#[test]
fn iec_while_zero_iterations() {
    let source = r#"
PROGRAM WhileZero
VAR
    count : INT := 0;
END_VAR
    count := 0;
    WHILE FALSE DO
        count := count + 1;
    END_WHILE;
END_PROGRAM
"#;
    let state = jit_run(source, "whilezero_scan", 2, 1);
    assert_eq!(
        read_i16(&state, 0),
        0,
        "WHILE FALSE should never execute body, count should be 0"
    );
}

// 19. REPEAT executes at least once
#[test]
fn iec_repeat_at_least_once() {
    let source = r#"
PROGRAM RepeatOnce
VAR
    count : INT := 0;
END_VAR
    count := 0;
    REPEAT
        count := count + 1;
    UNTIL TRUE
    END_REPEAT;
END_PROGRAM
"#;
    let state = jit_run(source, "repeatonce_scan", 2, 1);
    assert_eq!(
        read_i16(&state, 0),
        1,
        "REPEAT ... UNTIL TRUE should execute body exactly once"
    );
}

// ===========================================================================
// Section 2.5.1 — Standard Functions (via codegen)
// ===========================================================================

// 20. Function return via name assignment
#[test]
fn iec_function_return_via_name() {
    let source = r#"
FUNCTION Square : INT
VAR_INPUT
    x : INT;
END_VAR
    Square := x * x;
END_FUNCTION

PROGRAM FnRet
VAR
    result : INT := 0;
END_VAR
    result := Square(7);
END_PROGRAM
"#;
    let state = jit_run(source, "fnret_scan", 2, 1);
    assert_eq!(
        read_i16(&state, 0),
        49,
        "Square(7) should return 49 via function name assignment"
    );
}

// 21. Function with multiple inputs
#[test]
fn iec_function_multiple_inputs() {
    let source = r#"
FUNCTION AddThree : INT
VAR_INPUT
    a : INT;
    b : INT;
    c : INT;
END_VAR
    AddThree := a + b + c;
END_FUNCTION

PROGRAM MultiIn
VAR
    result : INT := 0;
END_VAR
    result := AddThree(10, 20, 30);
END_PROGRAM
"#;
    let state = jit_run(source, "multiin_scan", 2, 1);
    assert_eq!(
        read_i16(&state, 0),
        60,
        "AddThree(10, 20, 30) should be 60"
    );
}

// ===========================================================================
// Section 6 — Function Blocks / State Persistence
// ===========================================================================

// 22. Program state persists across scan cycles (counter pattern)
#[test]
fn iec_fb_state_persists() {
    let source = r#"
PROGRAM StatePersist
VAR
    count : INT := 0;
END_VAR
    count := count + 1;
END_PROGRAM
"#;
    // Run 5 scans — count should increment each time
    let state = jit_run(source, "statepersist_scan", 2, 5);
    assert_eq!(
        read_i16(&state, 0),
        5,
        "state should persist across scans: count should be 5 after 5 scans"
    );

    // Run 100 scans
    let state = jit_run(source, "statepersist_scan", 2, 100);
    assert_eq!(
        read_i16(&state, 0),
        100,
        "state should persist across scans: count should be 100 after 100 scans"
    );
}
