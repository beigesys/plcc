// SPDX-License-Identifier: MPL-2.0

//! Execution tests for five silent-wrong-answer semantic defects.
//!
//! Every test here compiles ST, JIT-executes it, and asserts an exact value read
//! out of the PROGRAM state struct. Each program declares its result variables
//! first so the offsets are stable.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;

/// Compile, JIT, run `p_init` then `scans` iterations of `p_scan`, return state.
fn run_scans(source: &str, scans: usize) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let ctx = Context::create();
    let mut compiler = Compiler::new(&ctx, "semdef");
    compiler.compile(&unit).expect("codegen failed");

    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT");
    let mut state = vec![0u8; 8192];
    let ptr = state.as_mut_ptr();
    if let Ok(addr) = ee.get_function_address("p_init") {
        let f: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(addr) };
        f(ptr);
    }
    let addr = ee.get_function_address("p_scan").expect("p_scan");
    let f: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(addr) };
    for _ in 0..scans {
        f(ptr);
    }
    state
}

fn run(source: &str) -> Vec<u8> {
    run_scans(source, 1)
}

fn dint(state: &[u8], idx: usize) -> i32 {
    let o = idx * 4;
    i32::from_ne_bytes([state[o], state[o + 1], state[o + 2], state[o + 3]])
}

/// Compile only, returning the codegen error if any.
fn compile_err(source: &str) -> Option<String> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let ctx = Context::create();
    let mut compiler = Compiler::new(&ctx, "semdef");
    compiler.compile(&unit).err().map(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// C1: VAR_IN_OUT is pass-by-reference and must write back
// ---------------------------------------------------------------------------

#[test]
fn fb_var_in_out_writes_back() {
    let src = r#"
FUNCTION_BLOCK BUMP
VAR_IN_OUT
    t : DINT;
END_VAR
    t := t + 5;
END_FUNCTION_BLOCK

PROGRAM p
VAR
    v : DINT := 10;
    f : BUMP;
END_VAR
    f(t := v);
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 15, "VAR_IN_OUT must write back to caller");
}

#[test]
fn fb_var_in_out_accumulates_across_scans() {
    let src = r#"
FUNCTION_BLOCK BUMP
VAR_IN_OUT
    t : DINT;
END_VAR
    t := t + 5;
END_FUNCTION_BLOCK

PROGRAM p
VAR
    v : DINT := 10;
    f : BUMP;
END_VAR
    f(t := v);
END_PROGRAM
"#;
    let state = run_scans(src, 3);
    assert_eq!(dint(&state, 0), 25, "three scans of +5 starting at 10");
}

#[test]
fn fb_var_in_out_reads_caller_updates() {
    // The FB observes writes the caller made between scans — only possible with a
    // real reference, not a copy-in/copy-out shim that snapshots at call time.
    let src = r#"
FUNCTION_BLOCK DOUBLER
VAR_IN_OUT
    t : DINT;
END_VAR
VAR_OUTPUT
    seen : DINT;
END_VAR
    seen := t;
    t := t * 2;
END_FUNCTION_BLOCK

PROGRAM p
VAR
    v : DINT := 3;
    seen : DINT;
    f : DOUBLER;
END_VAR
    f(t := v);
    seen := f.seen;
    v := v + 1;
END_PROGRAM
"#;
    // scan1: seen=3, v=6, then v=7. scan2: seen=7, v=14, then v=15.
    let state = run_scans(src, 2);
    assert_eq!(dint(&state, 0), 15, "v");
    assert_eq!(dint(&state, 1), 7, "seen");
}

#[test]
fn fb_var_in_out_mixed_with_inputs_and_outputs() {
    let src = r#"
FUNCTION_BLOCK MIX
VAR_INPUT
    k : DINT;
END_VAR
VAR_IN_OUT
    acc : DINT;
END_VAR
VAR_OUTPUT
    o : DINT;
END_VAR
    acc := acc + k;
    o := acc * 10;
END_FUNCTION_BLOCK

PROGRAM p
VAR
    a : DINT := 1;
    o : DINT;
    f : MIX;
END_VAR
    f(k := 4, acc := a);
    o := f.o;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 5, "acc written back");
    assert_eq!(dint(&state, 1), 50, "output");
}

#[test]
fn fb_var_in_out_two_params() {
    let src = r#"
FUNCTION_BLOCK SWAPADD
VAR_IN_OUT
    a : DINT;
    b : DINT;
END_VAR
    a := a + b;
    b := a - b;
END_FUNCTION_BLOCK

PROGRAM p
VAR
    x : DINT := 7;
    y : DINT := 2;
    f : SWAPADD;
END_VAR
    f(a := x, b := y);
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 9, "x");
    assert_eq!(dint(&state, 1), 7, "y");
}

#[test]
fn fb_var_in_out_on_struct_field_argument() {
    let src = r#"
TYPE Holder : STRUCT
    v : DINT;
END_STRUCT; END_TYPE

FUNCTION_BLOCK BUMP
VAR_IN_OUT
    t : DINT;
END_VAR
    t := t + 5;
END_FUNCTION_BLOCK

PROGRAM p
VAR
    n : DINT;
    h : Holder;
    f : BUMP;
END_VAR
    h.v := 10;
    f(t := h.v);
    n := h.v;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 15);
}

#[test]
fn function_var_in_out_writes_back() {
    let src = r#"
FUNCTION BUMPF : DINT
VAR_IN_OUT
    t : DINT;
END_VAR
    t := t + 5;
    BUMPF := t * 2;
END_FUNCTION

PROGRAM p
VAR
    n : DINT;
    v : DINT := 10;
END_VAR
    n := BUMPF(t := v);
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 30, "return value");
    assert_eq!(dint(&state, 1), 15, "v written back");
}

#[test]
fn method_var_in_out_writes_back() {
    let src = r#"
FUNCTION_BLOCK HOLDER
METHOD bump : DINT
VAR_IN_OUT
    t : DINT;
END_VAR
    t := t + 5;
    bump := t * 2;
END_METHOD
END_FUNCTION_BLOCK

PROGRAM p
VAR
    n : DINT;
    v : DINT := 10;
    h : HOLDER;
END_VAR
    n := h.bump(t := v);
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 30, "return value");
    assert_eq!(dint(&state, 1), 15, "v written back");
}

// ---------------------------------------------------------------------------
// C2: named FUNCTION arguments must bind by name, not position
// ---------------------------------------------------------------------------

#[test]
fn function_named_args_bind_by_name() {
    let src = r#"
FUNCTION SUB2 : DINT
VAR_INPUT
    a : DINT;
    b : DINT;
END_VAR
    SUB2 := a - b;
END_FUNCTION

PROGRAM p
VAR
    n : DINT;
END_VAR
    n := SUB2(b := 3, a := 10);
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 7);
}

#[test]
fn function_named_args_in_order_still_work() {
    let src = r#"
FUNCTION SUB2 : DINT
VAR_INPUT
    a : DINT;
    b : DINT;
END_VAR
    SUB2 := a - b;
END_FUNCTION

PROGRAM p
VAR
    n : DINT;
END_VAR
    n := SUB2(a := 10, b := 3);
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 7);
}

#[test]
fn function_positional_args_still_work() {
    let src = r#"
FUNCTION SUB2 : DINT
VAR_INPUT
    a : DINT;
    b : DINT;
END_VAR
    SUB2 := a - b;
END_FUNCTION

PROGRAM p
VAR
    n : DINT;
END_VAR
    n := SUB2(10, 3);
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 7);
}

#[test]
fn function_named_args_three_params_shuffled() {
    let src = r#"
FUNCTION F3 : DINT
VAR_INPUT
    a : DINT;
    b : DINT;
    c : DINT;
END_VAR
    F3 := a * 100 + b * 10 + c;
END_FUNCTION

PROGRAM p
VAR
    n : DINT;
END_VAR
    n := F3(c := 3, a := 1, b := 2);
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 123);
}

#[test]
fn function_unknown_named_arg_is_an_error() {
    let src = r#"
FUNCTION SUB2 : DINT
VAR_INPUT
    a : DINT;
    b : DINT;
END_VAR
    SUB2 := a - b;
END_FUNCTION

PROGRAM p
VAR
    n : DINT;
END_VAR
    n := SUB2(a := 10, zz := 3);
END_PROGRAM
"#;
    let err = compile_err(src).expect("unknown parameter name must be rejected");
    assert!(err.to_lowercase().contains("zz"), "diagnostic: {err}");
}

#[test]
fn function_mixed_named_and_positional_is_an_error() {
    let src = r#"
FUNCTION SUB2 : DINT
VAR_INPUT
    a : DINT;
    b : DINT;
END_VAR
    SUB2 := a - b;
END_FUNCTION

PROGRAM p
VAR
    n : DINT;
END_VAR
    n := SUB2(10, b := 3);
END_PROGRAM
"#;
    let err = compile_err(src).expect("mixing named and positional must be rejected");
    assert!(
        err.to_lowercase().contains("mix") || err.to_lowercase().contains("positional"),
        "diagnostic: {err}"
    );
}

// ---------------------------------------------------------------------------
// C3: STRUCT field default initializers
// ---------------------------------------------------------------------------

#[test]
fn struct_field_default_in_program_var() {
    let src = r#"
TYPE S : STRUCT
    a : DINT := 5;
    b : DINT := 9;
END_STRUCT; END_TYPE

PROGRAM p
VAR
    n : DINT;
    m : DINT;
    x : S;
END_VAR
    n := x.a;
    m := x.b;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 5);
    assert_eq!(dint(&state, 1), 9);
}

#[test]
fn struct_field_default_nested() {
    let src = r#"
TYPE Inner : STRUCT
    a : DINT := 5;
END_STRUCT; END_TYPE

TYPE Outer : STRUCT
    i : Inner;
    b : DINT := 7;
END_STRUCT; END_TYPE

PROGRAM p
VAR
    n : DINT;
    m : DINT;
    x : Outer;
END_VAR
    n := x.i.a;
    m := x.b;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 5);
    assert_eq!(dint(&state, 1), 7);
}

#[test]
fn struct_field_default_in_array_of_structs() {
    let src = r#"
TYPE S : STRUCT
    a : DINT := 5;
END_STRUCT; END_TYPE

PROGRAM p
VAR
    n : DINT;
    m : DINT;
    arr : ARRAY[1..3] OF S;
END_VAR
    n := arr[1].a;
    m := arr[3].a;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 5);
    assert_eq!(dint(&state, 1), 5);
}

#[test]
fn struct_field_default_in_fb_member() {
    let src = r#"
TYPE S : STRUCT
    a : DINT := 5;
END_STRUCT; END_TYPE

FUNCTION_BLOCK FB1
VAR_OUTPUT
    o : DINT;
END_VAR
VAR
    x : S;
END_VAR
    o := x.a;
END_FUNCTION_BLOCK

PROGRAM p
VAR
    n : DINT;
    f : FB1;
END_VAR
    f();
    n := f.o;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 5);
}

#[test]
fn struct_field_default_in_global() {
    let src = r#"
TYPE S : STRUCT
    a : DINT := 5;
END_STRUCT; END_TYPE

VAR_GLOBAL
    g : S;
END_VAR

PROGRAM p
VAR
    n : DINT;
END_VAR
    n := g.a;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 5);
}

/// A field default must not survive a later assignment, and must be re-applied on
/// every `_init` — not smeared into whatever the caller left in the buffer.
#[test]
fn struct_field_default_is_overwritable_and_reapplied() {
    let src = r#"
TYPE S : STRUCT
    a : DINT := 5;
END_STRUCT; END_TYPE

PROGRAM p
VAR
    n : DINT;
    x : S;
END_VAR
    n := x.a;
    x.a := 42;
END_PROGRAM
"#;
    // Two scans without a re-init: the second sees the value the first stored.
    let state = run_scans(src, 2);
    assert_eq!(dint(&state, 0), 42);
}

// ---------------------------------------------------------------------------
// C4: CONTINUE skips the rest of the loop body
// ---------------------------------------------------------------------------

#[test]
fn continue_in_for_skips_body_remainder() {
    let src = r#"
PROGRAM p
VAR
    n : DINT := 0;
    i : DINT;
END_VAR
    FOR i := 1 TO 4 DO
        IF i <= 2 THEN
            CONTINUE;
        END_IF;
        n := n + 1;
    END_FOR;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 2);
}

#[test]
fn continue_in_for_with_by_step_still_increments() {
    let src = r#"
PROGRAM p
VAR
    n : DINT := 0;
    i : DINT;
END_VAR
    FOR i := 0 TO 10 BY 2 DO
        IF i < 6 THEN
            CONTINUE;
        END_IF;
        n := n + i;
    END_FOR;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 24, "6+8+10");
}

#[test]
fn continue_in_while_skips_body_remainder() {
    let src = r#"
PROGRAM p
VAR
    n : DINT := 0;
    i : DINT := 0;
END_VAR
    WHILE i < 4 DO
        i := i + 1;
        IF i <= 2 THEN
            CONTINUE;
        END_IF;
        n := n + 1;
    END_WHILE;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 2);
}

#[test]
fn continue_in_repeat_skips_body_remainder() {
    let src = r#"
PROGRAM p
VAR
    n : DINT := 0;
    i : DINT := 0;
END_VAR
    REPEAT
        i := i + 1;
        IF i <= 2 THEN
            CONTINUE;
        END_IF;
        n := n + 1;
    UNTIL i >= 4 END_REPEAT;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(
        dint(&state, 0),
        2,
        "CONTINUE in REPEAT jumps to the UNTIL test"
    );
}

#[test]
fn continue_in_nested_for_targets_inner_loop() {
    let src = r#"
PROGRAM p
VAR
    n : DINT := 0;
    i : DINT;
    j : DINT;
END_VAR
    FOR i := 1 TO 3 DO
        FOR j := 1 TO 3 DO
            IF j = 2 THEN
                CONTINUE;
            END_IF;
            n := n + 1;
        END_FOR;
        n := n + 10;
    END_FOR;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 36, "3*(2 inner + 10)");
}

// ---------------------------------------------------------------------------
// C5: FB instance held in a STRUCT field
// ---------------------------------------------------------------------------

#[test]
fn fb_instance_in_struct_field_is_called() {
    let src = r#"
FUNCTION_BLOCK FB1
VAR_INPUT
    s : DINT;
END_VAR
VAR_OUTPUT
    o : DINT;
END_VAR
    o := s;
END_FUNCTION_BLOCK

TYPE Holder : STRUCT
    f : FB1;
END_STRUCT; END_TYPE

PROGRAM p
VAR
    n : DINT;
    x : Holder;
END_VAR
    x.f(s := 7);
    n := x.f.o;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 7);
}

#[test]
fn fb_instances_in_struct_fields_keep_separate_state() {
    let src = r#"
FUNCTION_BLOCK CTR
VAR_INPUT
    k : DINT;
END_VAR
VAR_OUTPUT
    o : DINT;
END_VAR
VAR
    acc : DINT := 0;
END_VAR
    acc := acc + k;
    o := acc;
END_FUNCTION_BLOCK

TYPE Holder : STRUCT
    a : CTR;
    b : CTR;
END_STRUCT; END_TYPE

PROGRAM p
VAR
    n : DINT;
    m : DINT;
    x : Holder;
END_VAR
    x.a(k := 1);
    x.b(k := 10);
    n := x.a.o;
    m := x.b.o;
END_PROGRAM
"#;
    let state = run_scans(src, 3);
    assert_eq!(dint(&state, 0), 3);
    assert_eq!(dint(&state, 1), 30);
}

#[test]
fn fb_instance_in_nested_struct_field_is_called() {
    let src = r#"
FUNCTION_BLOCK FB1
VAR_INPUT
    s : DINT;
END_VAR
VAR_OUTPUT
    o : DINT;
END_VAR
    o := s * 2;
END_FUNCTION_BLOCK

TYPE Inner : STRUCT
    f : FB1;
END_STRUCT; END_TYPE

TYPE Outer : STRUCT
    i : Inner;
END_STRUCT; END_TYPE

PROGRAM p
VAR
    n : DINT;
    x : Outer;
END_VAR
    x.i.f(s := 7);
    n := x.i.f.o;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 14);
}

/// Resolving the callee from its *type* rather than from a map of bare identifier
/// names also reaches an FB instance in an array element.
#[test]
fn fb_instance_in_array_element_is_called() {
    let src = r#"
FUNCTION_BLOCK CTR
VAR_INPUT
    k : DINT;
END_VAR
VAR_OUTPUT
    o : DINT;
END_VAR
VAR
    acc : DINT := 0;
END_VAR
    acc := acc + k;
    o := acc;
END_FUNCTION_BLOCK

PROGRAM p
VAR
    n : DINT;
    m : DINT;
    a : ARRAY[1..3] OF CTR;
END_VAR
    a[1](k := 1);
    a[3](k := 10);
    n := a[1].o;
    m := a[3].o;
END_PROGRAM
"#;
    let state = run_scans(src, 2);
    assert_eq!(dint(&state, 0), 2);
    assert_eq!(dint(&state, 1), 20);
}

/// …and a VAR_GLOBAL FB instance, which was never in the per-POU instance map at all.
#[test]
fn global_fb_instance_is_called() {
    let src = r#"
FUNCTION_BLOCK FB1
VAR_INPUT
    s : DINT;
END_VAR
VAR_OUTPUT
    o : DINT;
END_VAR
    o := s * 3;
END_FUNCTION_BLOCK

VAR_GLOBAL
    g : FB1;
END_VAR

PROGRAM p
VAR
    n : DINT;
END_VAR
    g(s := 7);
    n := g.o;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 21);
}
