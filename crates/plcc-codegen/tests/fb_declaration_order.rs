// SPDX-License-Identifier: MPL-2.0

//! Compiling an FB must not depend on where its callees appear in the source.
//!
//! `compile_fb_call` resolves the callee with `module.get_function(scan_fn_name)`,
//! and that `FunctionValue` used to be created by `compile_function_block` — i.e.
//! only once the callee's *own definition* had been compiled. So an `OUTER` declared
//! before the `INNER` it instantiates failed with
//! `LlvmError("FB scan function 'inner_scan' not found")`, and, because `plcc` merges
//! its input files in argument order, `plcc compile B.st A.st` failed where
//! `plcc compile A.st B.st` succeeded. The prototypes are now declared during
//! `layout_pou_structs`, which runs over every POU before any body is compiled.
//!
//! Every assertion here is on an executed value: an FB whose scan call is silently
//! dropped still compiles and still links, so "it compiled" proves nothing. The
//! inner blocks all accumulate across scans, so a dropped call shows up as a wrong
//! number rather than as a build failure.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;

/// Compile, JIT, run `<pou>_init` then `scans` iterations of `<pou>_scan`.
fn jit_scans(source: &str, pou: &str, scans: usize) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let ctx = Context::create();
    let mut compiler = Compiler::new(&ctx, "declorder");
    compiler.compile(&unit).expect("codegen failed");

    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT");
    let mut state = vec![0u8; 4096];
    let ptr = state.as_mut_ptr();
    if let Ok(addr) = ee.get_function_address(&format!("{pou}_init")) {
        let f: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(addr) };
        f(ptr);
    }
    let addr = ee
        .get_function_address(&format!("{pou}_scan"))
        .expect("scan function");
    let f: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(addr) };
    for _ in 0..scans {
        f(ptr);
    }
    state
}

/// Read a DINT at `offset`. Every program below declares its result DINTs first,
/// so they sit at predictable offsets ahead of any FB-instance member.
fn read_i32(state: &[u8], offset: usize) -> i32 {
    i32::from_ne_bytes([
        state[offset],
        state[offset + 1],
        state[offset + 2],
        state[offset + 3],
    ])
}

/// An FB that counts its own invocations, so a dropped scan call is visible as a
/// wrong value rather than as a missing symbol.
const COUNTER_FB: &str = r#"
FUNCTION_BLOCK COUNTER
VAR_INPUT
    STEP : DINT;
END_VAR
VAR_OUTPUT
    TOTAL : DINT;
END_VAR
VAR
    ACC : DINT := 0;
END_VAR
    ACC := ACC + STEP;
    TOTAL := ACC;
END_FUNCTION_BLOCK
"#;

#[test]
fn callee_declared_after_its_caller() {
    let source = format!(
        r#"
FUNCTION_BLOCK OUTER
VAR_INPUT
    STEP : DINT;
END_VAR
VAR_OUTPUT
    OUT : DINT;
END_VAR
VAR
    c : COUNTER;
END_VAR
    c(STEP := STEP);
    OUT := c.TOTAL;
END_FUNCTION_BLOCK
{COUNTER_FB}
PROGRAM P
VAR
    r : DINT;
    o : OUTER;
END_VAR
    o(STEP := 3);
    r := o.OUT;
END_PROGRAM
"#
    );
    let state = jit_scans(&source, "p", 4);
    assert_eq!(
        read_i32(&state, 0),
        12,
        "4 scans of STEP := 3 must total 12"
    );
}

#[test]
fn callee_declared_before_its_caller() {
    let source = format!(
        r#"
{COUNTER_FB}
FUNCTION_BLOCK OUTER
VAR_INPUT
    STEP : DINT;
END_VAR
VAR_OUTPUT
    OUT : DINT;
END_VAR
VAR
    c : COUNTER;
END_VAR
    c(STEP := STEP);
    OUT := c.TOTAL;
END_FUNCTION_BLOCK
PROGRAM P
VAR
    r : DINT;
    o : OUTER;
END_VAR
    o(STEP := 3);
    r := o.OUT;
END_PROGRAM
"#
    );
    let state = jit_scans(&source, "p", 4);
    assert_eq!(
        read_i32(&state, 0),
        12,
        "4 scans of STEP := 3 must total 12"
    );
}

#[test]
fn both_declaration_orders_in_the_same_unit() {
    // BEFORE_KIND is declared ahead of COUNTER, AFTER_KIND behind it. Both wrap the
    // same FB, so both must produce the same running total.
    let source = format!(
        r#"
FUNCTION_BLOCK BEFORE_KIND
VAR_OUTPUT
    OUT : DINT;
END_VAR
VAR
    c : COUNTER;
END_VAR
    c(STEP := 2);
    OUT := c.TOTAL;
END_FUNCTION_BLOCK
{COUNTER_FB}
FUNCTION_BLOCK AFTER_KIND
VAR_OUTPUT
    OUT : DINT;
END_VAR
VAR
    c : COUNTER;
END_VAR
    c(STEP := 2);
    OUT := c.TOTAL;
END_FUNCTION_BLOCK
PROGRAM P
VAR
    r1 : DINT;
    r2 : DINT;
    a : BEFORE_KIND;
    b : AFTER_KIND;
END_VAR
    a();
    b();
    r1 := a.OUT;
    r2 := b.OUT;
END_PROGRAM
"#
    );
    let state = jit_scans(&source, "p", 5);
    assert_eq!(read_i32(&state, 0), 10, "FB declared before its callee");
    assert_eq!(read_i32(&state, 4), 10, "FB declared after its callee");
}

#[test]
fn three_deep_forward_chain() {
    // A -> B -> C, every caller declared ahead of its callee. Each level adds a
    // distinct amount, so a call dropped at any level gives a distinguishable answer.
    let source = r#"
FUNCTION_BLOCK A
VAR_OUTPUT
    OUT : DINT;
END_VAR
VAR
    inner : B;
END_VAR
    inner();
    OUT := inner.OUT + 1;
END_FUNCTION_BLOCK

FUNCTION_BLOCK B
VAR_OUTPUT
    OUT : DINT;
END_VAR
VAR
    inner : C;
END_VAR
    inner();
    OUT := inner.OUT + 10;
END_FUNCTION_BLOCK

FUNCTION_BLOCK C
VAR_OUTPUT
    OUT : DINT;
END_VAR
VAR
    n : DINT := 0;
END_VAR
    n := n + 100;
    OUT := n;
END_FUNCTION_BLOCK

PROGRAM P
VAR
    r : DINT;
    a : A;
END_VAR
    a();
    r := a.OUT;
END_PROGRAM
"#;
    let state = jit_scans(source, "p", 1);
    assert_eq!(read_i32(&state, 0), 111, "one scan of A -> B -> C");
    let state = jit_scans(source, "p", 3);
    assert_eq!(
        read_i32(&state, 0),
        311,
        "three scans: C's counter is stateful"
    );
}

#[test]
fn a_class_declared_after_the_fb_that_uses_it() {
    // A CLASS gets a `<cls>_scan` prototype too, and `compile_class` used to create
    // it. An FB holding a class instance and calling a method on it exercises the
    // same lookup.
    let source = r#"
FUNCTION_BLOCK HOLDER
VAR_OUTPUT
    OUT : DINT;
END_VAR
VAR
    acc : ACCUM;
END_VAR
    acc.Bump(amount := 7);
    OUT := acc.Value();
END_FUNCTION_BLOCK

CLASS ACCUM
VAR
    total : DINT := 0;
END_VAR
    METHOD Bump
    VAR_INPUT
        amount : DINT;
    END_VAR
        total := total + amount;
    END_METHOD

    METHOD Value : DINT
        Value := total;
    END_METHOD
END_CLASS

PROGRAM P
VAR
    r : DINT;
    h : HOLDER;
END_VAR
    h();
    r := h.OUT;
END_PROGRAM
"#;
    let state = jit_scans(source, "p", 3);
    assert_eq!(read_i32(&state, 0), 21, "3 scans of Bump(amount := 7)");
}
