// SPDX-License-Identifier: MPL-2.0

//! Execution tests for IF/ELSIF join blocks.
//!
//! An `IF ... ELSIF ...` with no `ELSE` used to alias the last ELSIF's false edge
//! onto the join block itself, so the fall-through branch was emitted *into* the join:
//!
//! ```text
//! merge:                       ; preds = %merge, %elsif_then, %else, %then
//!   br label %merge            ; <- infinite self-loop
//!   %n4 = load i16, ptr %n     ; <- everything after the IF, unreachable
//! ```
//!
//! Semantically that means every statement following the IF was dead, and every path
//! through the ELSIF chain looped forever. At `OptimizationLevel::Default` LLVM never
//! finished with it, which is why `plcc compile` hung.
//!
//! These tests assert the exact post-IF values, so a regression cannot pass by
//! producing "some" output.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;

fn jit_scan(source: &str, scan_fn: &str, state_size: usize) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");

    let context = Context::create();
    let mut compiler = Compiler::new(&context, "if_elsif");
    compiler.compile(&unit).expect("codegen failed");

    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT");

    let mut state = vec![0u8; state_size];
    let ptr = state.as_mut_ptr();
    if let Ok(addr) = ee.get_function_address(&scan_fn.replace("_scan", "_init")) {
        let f: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(addr) };
        f(ptr);
    }
    let addr = ee.get_function_address(scan_fn).expect("scan fn");
    let f: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(addr) };
    f(ptr);
    state
}

fn read_i16(state: &[u8], offset: usize) -> i16 {
    i16::from_ne_bytes([state[offset], state[offset + 1]])
}

/// `sel` picks a branch; `n` records which one ran; `after` proves the statement
/// following the IF still executes. Layout: { i16 sel, i16 n, i16 after }.
const CHAIN: &str = r#"
PROGRAM Chain
VAR
    sel : INT := SELVAL;
    n : INT := -1;
    after : INT := 0;
END_VAR
    IF sel = 1 THEN
        n := 10;
    ELSIF sel = 2 THEN
        n := 20;
    ELSIF sel = 3 THEN
        n := 30;
    END_IF;
    after := 7;
END_PROGRAM
"#;

fn run_chain(sel: i16) -> (i16, i16) {
    let src = CHAIN.replace("SELVAL", &sel.to_string());
    let state = jit_scan(&src, "chain_scan", 16);
    (read_i16(&state, 2), read_i16(&state, 4))
}

#[test]
fn elsif_chain_without_else_falls_through_to_the_next_statement() {
    // Each of these used to hang or drop `after := 7`.
    assert_eq!(
        run_chain(1),
        (10, 7),
        "first branch taken, then fall-through"
    );
    assert_eq!(run_chain(2), (20, 7), "second branch taken");
    assert_eq!(run_chain(3), (30, 7), "third branch taken");
    assert_eq!(
        run_chain(9),
        (-1, 7),
        "no branch taken, still falls through"
    );
}

/// The exact shape in the bundled CTUD body: a nested IF/ELSIF as the last statement
/// of a second-or-later ELSIF branch.
const NESTED: &str = r#"
PROGRAM Nested
VAR
    a : BOOL := AV;
    b : BOOL := BV;
    c : BOOL := CV;
    d : BOOL := DV;
    n : INT := -1;
    after : INT := 0;
END_VAR
    IF a THEN
        n := 0;
    ELSIF b THEN
        n := 1;
    ELSIF c THEN
        IF d THEN
            n := 2;
        ELSIF a THEN
            n := 3;
        END_IF;
    END_IF;
    after := 7;
END_PROGRAM
"#;

fn run_nested(a: bool, b: bool, c: bool, d: bool) -> (i16, i16) {
    let lit = |v: bool| if v { "TRUE" } else { "FALSE" };
    let src = NESTED
        .replace("AV", lit(a))
        .replace("BV", lit(b))
        .replace("CV", lit(c))
        .replace("DV", lit(d));
    // { i8 a, i8 b, i8 c, i8 d, i16 n, i16 after }
    let state = jit_scan(&src, "nested_scan", 16);
    (read_i16(&state, 4), read_i16(&state, 6))
}

#[test]
fn nested_if_in_the_tail_of_a_later_elsif_branch() {
    assert_eq!(run_nested(true, false, false, false), (0, 7), "outer THEN");
    assert_eq!(run_nested(false, true, false, false), (1, 7), "first ELSIF");
    assert_eq!(
        run_nested(false, false, true, true),
        (2, 7),
        "second ELSIF, nested THEN"
    );
    assert_eq!(
        run_nested(false, false, true, false),
        (-1, 7),
        "second ELSIF, nested chain falls through (a is FALSE)"
    );
    assert_eq!(
        run_nested(false, false, false, true),
        (-1, 7),
        "nothing matches, statement after the IF still runs"
    );
}

/// A loop body that ends in an IF/ELSIF chain: the join has to reach the back edge.
#[test]
fn elsif_inside_a_loop_body_still_iterates() {
    let src = r#"
PROGRAM Loopy
VAR
    i : INT;
    evens : INT := 0;
    odds : INT := 0;
    after : INT := 0;
END_VAR
    FOR i := 1 TO 10 DO
        IF i MOD 2 = 0 THEN
            evens := evens + 1;
        ELSIF i MOD 2 = 1 THEN
            odds := odds + 1;
        END_IF;
    END_FOR;
    after := 7;
END_PROGRAM
"#;
    // { i16 i, i16 evens, i16 odds, i16 after }
    let state = jit_scan(src, "loopy_scan", 16);
    assert_eq!(read_i16(&state, 2), 5, "five even values in 1..10");
    assert_eq!(read_i16(&state, 4), 5, "five odd values in 1..10");
    assert_eq!(read_i16(&state, 6), 7, "statement after the loop runs");
}

/// `WHILE FALSE` used to emit `br i8 0, ...` — a non-i1 branch condition. LLVM's
/// verifier rejects it; nothing checked, so it shipped.
#[test]
fn while_false_executes_zero_times_and_falls_through() {
    let src = r#"
PROGRAM Wz
VAR
    n : INT := 0;
    after : INT := 0;
END_VAR
    WHILE FALSE DO
        n := n + 1;
    END_WHILE;
    after := 7;
END_PROGRAM
"#;
    let state = jit_scan(src, "wz_scan", 16);
    assert_eq!(read_i16(&state, 0), 0, "body must not run");
    assert_eq!(read_i16(&state, 2), 7, "statement after the loop runs");
}
