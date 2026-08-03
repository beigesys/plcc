// SPDX-License-Identifier: MPL-2.0

//! Advanced end-to-end execution tests: function calls, comparisons, bitwise ops,
//! unary minus, precedence, REAL arithmetic, deeply nested IF, CASE ELSE, and
//! multi-scan counters.

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

// ---------------------------------------------------------------------------
// 1. Function calls with return values
// ---------------------------------------------------------------------------
#[test]
fn function_call_with_return_value() {
    let source = r#"
FUNCTION AddTwo : INT
VAR_INPUT
    a : INT;
    b : INT;
END_VAR
    AddTwo := a + b;
END_FUNCTION

PROGRAM CallTest
VAR
    result : INT := 0;
END_VAR
    result := AddTwo(10, 20);
END_PROGRAM
"#;
    // State: { result: i16 } = 2 bytes
    let state = jit_run(source, "calltest_scan", 2, 1);
    let result = read_i16(&state, 0);
    assert_eq!(result, 30, "AddTwo(10, 20) should be 30, got {result}");
}

// ---------------------------------------------------------------------------
// 2. Nested function calls
// ---------------------------------------------------------------------------
#[test]
fn nested_function_calls() {
    let source = r#"
FUNCTION AddOne : INT
VAR_INPUT
    x : INT;
END_VAR
    AddOne := x + 1;
END_FUNCTION

FUNCTION Double : INT
VAR_INPUT
    x : INT;
END_VAR
    Double := x * 2;
END_FUNCTION

PROGRAM NestCall
VAR
    result : INT := 0;
END_VAR
    result := Double(AddOne(5));
END_PROGRAM
"#;
    // AddOne(5) = 6, Double(6) = 12
    let state = jit_run(source, "nestcall_scan", 2, 1);
    let result = read_i16(&state, 0);
    assert_eq!(result, 12, "Double(AddOne(5)) should be 12, got {result}");
}

// ---------------------------------------------------------------------------
// 3. Comparison operators — all six, via IF/THEN to store 1 or 0 into INT
// ---------------------------------------------------------------------------
#[test]
fn comparison_operators_comprehensive() {
    let source = r#"
PROGRAM CmpOps
VAR
    eq_true  : INT := 0;
    eq_false : INT := 0;
    ne_true  : INT := 0;
    ne_false : INT := 0;
    lt_true  : INT := 0;
    lt_false : INT := 0;
    le_true  : INT := 0;
    le_eq    : INT := 0;
    le_false : INT := 0;
    gt_true  : INT := 0;
    gt_false : INT := 0;
    ge_true  : INT := 0;
    ge_eq    : INT := 0;
    ge_false : INT := 0;
END_VAR
    (* = *)
    IF 5 = 5 THEN eq_true := 1; ELSE eq_true := 0; END_IF;
    IF 5 = 6 THEN eq_false := 1; ELSE eq_false := 0; END_IF;

    (* <> *)
    IF 5 <> 6 THEN ne_true := 1; ELSE ne_true := 0; END_IF;
    IF 5 <> 5 THEN ne_false := 1; ELSE ne_false := 0; END_IF;

    (* < *)
    IF 3 < 7 THEN lt_true := 1; ELSE lt_true := 0; END_IF;
    IF 7 < 3 THEN lt_false := 1; ELSE lt_false := 0; END_IF;

    (* <= *)
    IF 3 <= 7 THEN le_true := 1; ELSE le_true := 0; END_IF;
    IF 5 <= 5 THEN le_eq := 1; ELSE le_eq := 0; END_IF;
    IF 7 <= 3 THEN le_false := 1; ELSE le_false := 0; END_IF;

    (* > *)
    IF 7 > 3 THEN gt_true := 1; ELSE gt_true := 0; END_IF;
    IF 3 > 7 THEN gt_false := 1; ELSE gt_false := 0; END_IF;

    (* >= *)
    IF 7 >= 3 THEN ge_true := 1; ELSE ge_true := 0; END_IF;
    IF 5 >= 5 THEN ge_eq := 1; ELSE ge_eq := 0; END_IF;
    IF 3 >= 7 THEN ge_false := 1; ELSE ge_false := 0; END_IF;
END_PROGRAM
"#;
    // 14 INT fields x 2 bytes = 28 bytes
    let state = jit_run(source, "cmpops_scan", 28, 1);
    assert_eq!(read_i16(&state, 0), 1, "5 = 5 should be true");
    assert_eq!(read_i16(&state, 2), 0, "5 = 6 should be false");
    assert_eq!(read_i16(&state, 4), 1, "5 <> 6 should be true");
    assert_eq!(read_i16(&state, 6), 0, "5 <> 5 should be false");
    assert_eq!(read_i16(&state, 8), 1, "3 < 7 should be true");
    assert_eq!(read_i16(&state, 10), 0, "7 < 3 should be false");
    assert_eq!(read_i16(&state, 12), 1, "3 <= 7 should be true");
    assert_eq!(read_i16(&state, 14), 1, "5 <= 5 should be true");
    assert_eq!(read_i16(&state, 16), 0, "7 <= 3 should be false");
    assert_eq!(read_i16(&state, 18), 1, "7 > 3 should be true");
    assert_eq!(read_i16(&state, 20), 0, "3 > 7 should be false");
    assert_eq!(read_i16(&state, 22), 1, "7 >= 3 should be true");
    assert_eq!(read_i16(&state, 24), 1, "5 >= 5 should be true");
    assert_eq!(read_i16(&state, 26), 0, "3 >= 7 should be false");
}

// ---------------------------------------------------------------------------
// 4. Bitwise operations: AND, OR, XOR, NOT on INT values
// ---------------------------------------------------------------------------
#[test]
fn bitwise_operations() {
    let source = r#"
PROGRAM BitOps
VAR
    and_result : INT := 0;
    or_result  : INT := 0;
    xor_result : INT := 0;
    not_result : INT := 0;
END_VAR
    and_result := 16#FF AND 16#0F;
    or_result  := 16#F0 OR 16#0F;
    xor_result := 16#FF XOR 16#0F;
    not_result := NOT 0;
END_PROGRAM
"#;
    // State: 4 x i16 = 8 bytes
    let state = jit_run(source, "bitops_scan", 8, 1);
    assert_eq!(
        read_i16(&state, 0),
        0x0F,
        "0xFF AND 0x0F should be 0x0F (15), got {}",
        read_i16(&state, 0)
    );
    assert_eq!(
        read_i16(&state, 2),
        0xFF_u8 as i16,
        "0xF0 OR 0x0F should be 0xFF (255), got {}",
        read_i16(&state, 2)
    );
    assert_eq!(
        read_i16(&state, 4),
        0xF0_u8 as i16,
        "0xFF XOR 0x0F should be 0xF0 (240), got {}",
        read_i16(&state, 4)
    );
    // NOT 0 on i16 is -1 (all bits set, two's complement)
    assert_eq!(
        read_i16(&state, 6),
        -1,
        "NOT 0 should be -1 (all bits set), got {}",
        read_i16(&state, 6)
    );
}

// ---------------------------------------------------------------------------
// 5. Negative numbers and unary minus
// ---------------------------------------------------------------------------
#[test]
fn negative_numbers_and_unary_minus() {
    let source = r#"
PROGRAM NegTest
VAR
    x : INT := 0;
    y : INT := 0;
    z : INT := 0;
END_VAR
    x := -5;
    y := -x;
    z := -10 + 3;
END_PROGRAM
"#;
    // State: 3 x i16 = 6 bytes
    let state = jit_run(source, "negtest_scan", 6, 1);
    assert_eq!(
        read_i16(&state, 0),
        -5,
        "x should be -5, got {}",
        read_i16(&state, 0)
    );
    assert_eq!(
        read_i16(&state, 2),
        5,
        "y := -x should be 5, got {}",
        read_i16(&state, 2)
    );
    assert_eq!(
        read_i16(&state, 4),
        -7,
        "z := -10 + 3 should be -7, got {}",
        read_i16(&state, 4)
    );
}

// ---------------------------------------------------------------------------
// 6. Complex expressions with operator precedence
// ---------------------------------------------------------------------------
#[test]
fn operator_precedence() {
    let source = r#"
PROGRAM Prec
VAR
    a : INT := 0;
    b : INT := 0;
    c : INT := 0;
    d : INT := 0;
END_VAR
    a := 2 + 3 * 4;
    b := (2 + 3) * 4;
    c := 10 - 2 * 3 + 1;
    d := 100 / 5 / 4;
END_PROGRAM
"#;
    // State: 4 x i16 = 8 bytes
    let state = jit_run(source, "prec_scan", 8, 1);
    assert_eq!(
        read_i16(&state, 0),
        14,
        "2 + 3 * 4 should be 14, got {}",
        read_i16(&state, 0)
    );
    assert_eq!(
        read_i16(&state, 2),
        20,
        "(2 + 3) * 4 should be 20, got {}",
        read_i16(&state, 2)
    );
    assert_eq!(
        read_i16(&state, 4),
        5,
        "10 - 2 * 3 + 1 should be 5, got {}",
        read_i16(&state, 4)
    );
    assert_eq!(
        read_i16(&state, 6),
        5,
        "100 / 5 / 4 should be 5, got {}",
        read_i16(&state, 6)
    );
}

// ---------------------------------------------------------------------------
// 7. REAL (f32) arithmetic
// ---------------------------------------------------------------------------
#[test]
fn real_arithmetic() {
    let source = r#"
PROGRAM RealMath
VAR
    sum  : REAL := 0.0;
    prod : REAL := 0.0;
    quot : REAL := 0.0;
    diff : REAL := 0.0;
END_VAR
    sum  := 1.5 + 2.5;
    prod := 3.0 * 4.5;
    quot := 10.0 / 4.0;
    diff := 100.0 - 37.5;
END_PROGRAM
"#;
    // State: 4 x f32 = 16 bytes
    let state = jit_run(source, "realmath_scan", 16, 1);
    let sum = read_f32(&state, 0);
    let prod = read_f32(&state, 4);
    let quot = read_f32(&state, 8);
    let diff = read_f32(&state, 12);
    assert!(
        (sum - 4.0).abs() < 0.001,
        "1.5 + 2.5 should be 4.0, got {sum}"
    );
    assert!(
        (prod - 13.5).abs() < 0.001,
        "3.0 * 4.5 should be 13.5, got {prod}"
    );
    assert!(
        (quot - 2.5).abs() < 0.001,
        "10.0 / 4.0 should be 2.5, got {quot}"
    );
    assert!(
        (diff - 62.5).abs() < 0.001,
        "100.0 - 37.5 should be 62.5, got {diff}"
    );
}

// ---------------------------------------------------------------------------
// 8. Deeply nested IF (4 levels)
// ---------------------------------------------------------------------------
#[test]
fn deeply_nested_if() {
    let source = r#"
PROGRAM DeepIf
VAR
    a : INT := 0;
    b : INT := 0;
    c : INT := 0;
    d : INT := 0;
    result : INT := 0;
END_VAR
    a := 10;
    b := 20;
    c := 30;
    d := 40;

    IF a > 5 THEN
        IF b > 15 THEN
            IF c > 25 THEN
                IF d > 35 THEN
                    result := 1111;
                ELSE
                    result := 1110;
                END_IF;
            ELSE
                result := 1100;
            END_IF;
        ELSE
            result := 1000;
        END_IF;
    ELSE
        result := 0;
    END_IF;
END_PROGRAM
"#;
    // All conditions true: a=10>5, b=20>15, c=30>25, d=40>35 => result=1111
    // State: 5 x i16 = 10 bytes; result is at offset 8
    let state = jit_run(source, "deepif_scan", 10, 1);
    assert_eq!(
        read_i16(&state, 8),
        1111,
        "all branches true should give 1111, got {}",
        read_i16(&state, 8)
    );
}

#[test]
fn deeply_nested_if_inner_else() {
    let source = r#"
PROGRAM DeepIf2
VAR
    a : INT := 0;
    b : INT := 0;
    c : INT := 0;
    d : INT := 0;
    result : INT := 0;
END_VAR
    a := 10;
    b := 20;
    c := 30;
    d := 10;

    IF a > 5 THEN
        IF b > 15 THEN
            IF c > 25 THEN
                IF d > 35 THEN
                    result := 1111;
                ELSE
                    result := 1110;
                END_IF;
            ELSE
                result := 1100;
            END_IF;
        ELSE
            result := 1000;
        END_IF;
    ELSE
        result := 0;
    END_IF;
END_PROGRAM
"#;
    // d=10 not >35, so innermost ELSE => result=1110
    let state = jit_run(source, "deepif2_scan", 10, 1);
    assert_eq!(
        read_i16(&state, 8),
        1110,
        "d <= 35 should give 1110, got {}",
        read_i16(&state, 8)
    );
}

// ---------------------------------------------------------------------------
// 9. CASE with fallthrough to ELSE (unmatched selector)
// ---------------------------------------------------------------------------
#[test]
fn case_else_fallthrough() {
    let source = r#"
PROGRAM CaseElse
VAR
    selector : INT := 0;
    output   : INT := 0;
END_VAR
    selector := 99;
    CASE selector OF
        1:
            output := 10;
        2:
            output := 20;
        3:
            output := 30;
    ELSE
        output := 999;
    END_CASE;
END_PROGRAM
"#;
    // selector=99 matches no case => ELSE => output=999
    let state = jit_run(source, "caseelse_scan", 4, 1);
    assert_eq!(
        read_i16(&state, 2),
        999,
        "unmatched CASE should fall to ELSE (999), got {}",
        read_i16(&state, 2)
    );
}

#[test]
fn case_matches_specific_value() {
    let source = r#"
PROGRAM CaseMatch
VAR
    selector : INT := 0;
    output   : INT := 0;
END_VAR
    selector := 2;
    CASE selector OF
        1:
            output := 10;
        2:
            output := 20;
        3:
            output := 30;
    ELSE
        output := 999;
    END_CASE;
END_PROGRAM
"#;
    let state = jit_run(source, "casematch_scan", 4, 1);
    assert_eq!(
        read_i16(&state, 2),
        20,
        "CASE 2 should give output=20, got {}",
        read_i16(&state, 2)
    );
}

// ---------------------------------------------------------------------------
// 10. Counter pattern across scans
// ---------------------------------------------------------------------------
#[test]
fn counter_100_scans() {
    let source = r#"
PROGRAM Counter
VAR
    count : INT := 0;
END_VAR
    count := count + 1;
END_PROGRAM
"#;
    let state = jit_run(source, "counter_scan", 2, 100);
    let count = read_i16(&state, 0);
    assert_eq!(
        count, 100,
        "after 100 scans count should be 100, got {count}"
    );
}

#[test]
fn counter_1000_scans() {
    let source = r#"
PROGRAM Counter2
VAR
    count : INT := 0;
END_VAR
    count := count + 1;
END_PROGRAM
"#;
    let state = jit_run(source, "counter2_scan", 2, 1000);
    let count = read_i16(&state, 0);
    assert_eq!(
        count, 1000,
        "after 1000 scans count should be 1000, got {count}"
    );
}

// ---------------------------------------------------------------------------
// Bonus: multi-function chain (3 functions deep)
// ---------------------------------------------------------------------------
#[test]
fn triple_nested_function_call() {
    let source = r#"
FUNCTION Inc : INT
VAR_INPUT
    v : INT;
END_VAR
    Inc := v + 1;
END_FUNCTION

FUNCTION Square : INT
VAR_INPUT
    v : INT;
END_VAR
    Square := v * v;
END_FUNCTION

FUNCTION Negate : INT
VAR_INPUT
    v : INT;
END_VAR
    Negate := 0 - v;
END_FUNCTION

PROGRAM Chain
VAR
    result : INT := 0;
END_VAR
    result := Negate(Square(Inc(2)));
END_PROGRAM
"#;
    // Inc(2) = 3, Square(3) = 9, Negate(9) = -9
    let state = jit_run(source, "chain_scan", 2, 1);
    let result = read_i16(&state, 0);
    assert_eq!(
        result, -9,
        "Negate(Square(Inc(2))) should be -9, got {result}"
    );
}

// ---------------------------------------------------------------------------
// Bonus: function used in arithmetic expression
// ---------------------------------------------------------------------------
#[test]
fn function_in_expression() {
    let source = r#"
FUNCTION Triple : INT
VAR_INPUT
    v : INT;
END_VAR
    Triple := v * 3;
END_FUNCTION

PROGRAM FnExpr
VAR
    result : INT := 0;
END_VAR
    result := Triple(4) + Triple(5);
END_PROGRAM
"#;
    // Triple(4) = 12, Triple(5) = 15, sum = 27
    let state = jit_run(source, "fnexpr_scan", 2, 1);
    let result = read_i16(&state, 0);
    assert_eq!(
        result, 27,
        "Triple(4) + Triple(5) should be 27, got {result}"
    );
}

// ---------------------------------------------------------------------------
// Bonus: accumulator with conditional increment (combines multi-scan + IF)
// ---------------------------------------------------------------------------
#[test]
fn conditional_accumulator() {
    let source = r#"
PROGRAM CondAccum
VAR
    tick   : INT := 0;
    evens  : INT := 0;
    odds   : INT := 0;
END_VAR
    tick := tick + 1;
    IF (tick MOD 2) = 0 THEN
        evens := evens + 1;
    ELSE
        odds := odds + 1;
    END_IF;
END_PROGRAM
"#;
    // After 20 scans: tick=20, evens=10 (ticks 2,4,...,20), odds=10 (ticks 1,3,...,19)
    let state = jit_run(source, "condaccum_scan", 6, 20);
    let tick = read_i16(&state, 0);
    let evens = read_i16(&state, 2);
    let odds = read_i16(&state, 4);
    assert_eq!(tick, 20, "tick should be 20, got {tick}");
    assert_eq!(evens, 10, "evens should be 10, got {evens}");
    assert_eq!(odds, 10, "odds should be 10, got {odds}");
}
