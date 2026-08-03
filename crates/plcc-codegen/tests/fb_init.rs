// SPDX-License-Identifier: MPL-2.0

//! Execution tests for FUNCTION_BLOCK / CLASS member initializers.
//!
//! IEC 61131-3 requires that *declared* initial values are applied when an
//! instance is initialized. Only *undeclared* initial values default to zero.
//! These tests compile real ST, JIT it, call the generated `_init` entry point
//! and read the raw instance state back, asserting exact values.
//!
//! All fixtures below use only `INT` (i16) members so the LLVM struct layout is
//! a trivially predictable array of 2-byte slots, or `REAL` (f32) members for
//! 4-byte slots.

use inkwell::AddressSpace;
use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::execution_engine::ExecutionEngine;
use plcc_codegen::Compiler;

/// Compile `source` and hand the caller a live JIT engine.
fn with_jit<R>(source: &str, f: impl FnOnce(&ExecutionEngine) -> R) -> R {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");

    let context = Context::create();
    let mut compiler = Compiler::new(&context, "fb_init_test");
    compiler.compile(&unit).expect("codegen failed");

    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT");

    f(&ee)
}

/// Compile `source`, allocate `size` zeroed bytes, call `init_fn(ptr)`, return the bytes.
fn run_init(source: &str, init_fn: &str, size: usize) -> Vec<u8> {
    with_jit(source, |ee| {
        let addr = ee
            .get_function_address(init_fn)
            .unwrap_or_else(|e| panic!("init function `{init_fn}` not found: {e:?}"));
        let init: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(addr) };
        let mut state = vec![0u8; size];
        init(state.as_mut_ptr());
        state
    })
}

/// Compile `source`, call `init_fn` on a fresh state buffer, then return a copy of the
/// first `globals_size` bytes of the JIT-resident `plcc_globals` object.
///
/// MCJIT ignores `add_global_mapping` for globals that have a definition, so the address
/// is obtained by splicing a tiny accessor function into the module that returns
/// `@plcc_globals` and calling it through the JIT.
fn run_init_with_globals(
    source: &str,
    init_fn: &str,
    state_size: usize,
    globals_size: usize,
) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");

    let context = Context::create();
    let mut compiler = Compiler::new(&context, "fb_init_globals_test");
    compiler.compile(&unit).expect("codegen failed");

    {
        let module = compiler.module();
        let gv = module
            .get_global("plcc_globals")
            .expect("module has no `plcc_globals`");
        let ptr_ty = context.ptr_type(AddressSpace::default());
        let accessor = module.add_function("test_globals_addr", ptr_ty.fn_type(&[], false), None);
        let bb = context.append_basic_block(accessor, "entry");
        let b = context.create_builder();
        b.position_at_end(bb);
        b.build_return(Some(&gv.as_pointer_value()))
            .expect("accessor build_return failed");
    }

    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT");

    let addr = ee
        .get_function_address(init_fn)
        .unwrap_or_else(|e| panic!("init function `{init_fn}` not found: {e:?}"));
    let init: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(addr) };
    let mut state = vec![0u8; state_size];
    init(state.as_mut_ptr());

    let acc_addr = ee
        .get_function_address("test_globals_addr")
        .expect("accessor not found");
    let acc: extern "C" fn() -> *const u8 = unsafe { std::mem::transmute(acc_addr) };
    let globals_ptr = acc();
    assert!(!globals_ptr.is_null(), "plcc_globals address is null");
    unsafe { std::slice::from_raw_parts(globals_ptr, globals_size) }.to_vec()
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

// ---------------------------------------------------------------------------
// 1. VAR / VAR_INPUT / VAR_OUTPUT members with non-zero declared initial values
// ---------------------------------------------------------------------------

const WIDGET_SRC: &str = r#"
FUNCTION_BLOCK Widget
VAR_INPUT
    gain : INT := 7;
END_VAR
VAR_OUTPUT
    status : INT := 11;
END_VAR
VAR
    level : INT := 23;
    untouched : INT;
END_VAR
    level := level;
END_FUNCTION_BLOCK
"#;

#[test]
fn fb_member_initializers_applied() {
    // Widget layout: { i16 gain, i16 status, i16 level, i16 untouched } = 8 bytes.
    let state = run_init(WIDGET_SRC, "widget_init", 32);

    assert_eq!(read_i16(&state, 0), 7, "VAR_INPUT gain should init to 7");
    assert_eq!(
        read_i16(&state, 2),
        11,
        "VAR_OUTPUT status should init to 11"
    );
    assert_eq!(read_i16(&state, 4), 23, "VAR level should init to 23");
    assert_eq!(
        read_i16(&state, 6),
        0,
        "member without an initializer must stay zero"
    );
}

// ---------------------------------------------------------------------------
// 2. Program that instantiates an FB — program _init must init the sub-instance
// ---------------------------------------------------------------------------

#[test]
fn program_init_initializes_fb_instance_field() {
    let source = format!(
        "{WIDGET_SRC}
PROGRAM Main
VAR
    w : Widget;
    tail : INT := 99;
END_VAR
    tail := tail;
END_PROGRAM
"
    );

    // Main layout: { Widget(8 bytes), i16 tail } = 10 bytes.
    let state = run_init(&source, "main_init", 32);

    assert_eq!(read_i16(&state, 0), 7, "w.gain");
    assert_eq!(read_i16(&state, 2), 11, "w.status");
    assert_eq!(read_i16(&state, 4), 23, "w.level");
    assert_eq!(read_i16(&state, 6), 0, "w.untouched");
    assert_eq!(read_i16(&state, 8), 99, "tail");
}

// ---------------------------------------------------------------------------
// 3. Nested FBs — both declaration orders
// ---------------------------------------------------------------------------

/// Inner declared *before* Outer (the easy order).
#[test]
fn nested_fb_init_inner_declared_first() {
    let source = r#"
FUNCTION_BLOCK Inner
VAR
    a : INT := 5;
    b : INT := 6;
END_VAR
    a := a;
END_FUNCTION_BLOCK

FUNCTION_BLOCK Outer
VAR
    lead : INT := 1;
    sub : Inner;
    trail : INT := 9;
END_VAR
    lead := lead;
END_FUNCTION_BLOCK

PROGRAM Main
VAR
    o : Outer;
END_VAR
    o();
END_PROGRAM
"#;

    // Outer layout: { i16 lead, Inner{ i16 a, i16 b }, i16 trail } = 8 bytes.
    let state = run_init(source, "main_init", 32);
    assert_eq!(read_i16(&state, 0), 1, "o.lead");
    assert_eq!(read_i16(&state, 2), 5, "o.sub.a");
    assert_eq!(read_i16(&state, 4), 6, "o.sub.b");
    assert_eq!(read_i16(&state, 6), 9, "o.trail");
}

/// Inner declared *after* Outer — forward reference.
#[test]
fn nested_fb_init_inner_declared_last() {
    let source = r#"
FUNCTION_BLOCK Outer
VAR
    lead : INT := 1;
    sub : Inner;
    trail : INT := 9;
END_VAR
    lead := lead;
END_FUNCTION_BLOCK

FUNCTION_BLOCK Inner
VAR
    a : INT := 5;
    b : INT := 6;
END_VAR
    a := a;
END_FUNCTION_BLOCK

PROGRAM Main
VAR
    o : Outer;
END_VAR
    o();
END_PROGRAM
"#;

    let state = run_init(source, "main_init", 32);
    assert_eq!(read_i16(&state, 0), 1, "o.lead");
    assert_eq!(read_i16(&state, 2), 5, "o.sub.a");
    assert_eq!(read_i16(&state, 4), 6, "o.sub.b");
    assert_eq!(read_i16(&state, 6), 9, "o.trail");
}

/// Three levels deep: L1 { L2 { L3 { x := 42 } } }
#[test]
fn nested_fb_init_three_levels() {
    let source = r#"
FUNCTION_BLOCK L1
VAR
    child : L2;
    tag : INT := 100;
END_VAR
    tag := tag;
END_FUNCTION_BLOCK

FUNCTION_BLOCK L2
VAR
    child : L3;
    tag : INT := 200;
END_VAR
    tag := tag;
END_FUNCTION_BLOCK

FUNCTION_BLOCK L3
VAR
    x : INT := 42;
END_VAR
    x := x;
END_FUNCTION_BLOCK

PROGRAM Main
VAR
    top : L1;
END_VAR
    top();
END_PROGRAM
"#;

    // L1 = { L2{ L3{ i16 x }, i16 tag }, i16 tag } = 6 bytes.
    let state = run_init(source, "main_init", 32);
    assert_eq!(read_i16(&state, 0), 42, "top.child.child.x");
    assert_eq!(read_i16(&state, 2), 200, "top.child.tag");
    assert_eq!(read_i16(&state, 4), 100, "top.tag");
}

// ---------------------------------------------------------------------------
// 4. CLASS member initializers
// ---------------------------------------------------------------------------

#[test]
fn class_member_initializers_applied() {
    let source = r#"
CLASS Gadget
VAR
    alpha : INT := 31;
    beta : INT := 37;
    unset : INT;
END_VAR
METHOD Bump : INT
    alpha := alpha + 1;
    Bump := alpha;
END_METHOD
END_CLASS

PROGRAM Main
VAR
    g : Gadget;
    marker : INT := 77;
END_VAR
    marker := marker;
END_PROGRAM
"#;

    // Direct class init.
    let state = run_init(source, "gadget_init", 32);
    assert_eq!(read_i16(&state, 0), 31, "Gadget.alpha");
    assert_eq!(read_i16(&state, 2), 37, "Gadget.beta");
    assert_eq!(read_i16(&state, 4), 0, "Gadget.unset stays zero");

    // And through the program that instantiates it.
    let state = run_init(source, "main_init", 32);
    assert_eq!(read_i16(&state, 0), 31, "g.alpha");
    assert_eq!(read_i16(&state, 2), 37, "g.beta");
    assert_eq!(read_i16(&state, 4), 0, "g.unset");
    assert_eq!(read_i16(&state, 6), 77, "marker");
}

// ---------------------------------------------------------------------------
// 5. VAR_GLOBAL FB instance
// ---------------------------------------------------------------------------

#[test]
fn global_fb_instance_is_initialized() {
    let source = r#"
FUNCTION_BLOCK Widget
VAR_INPUT
    gain : INT := 7;
END_VAR
VAR_OUTPUT
    status : INT := 11;
END_VAR
VAR
    level : INT := 23;
    untouched : INT;
END_VAR
    level := level;
END_FUNCTION_BLOCK

VAR_GLOBAL
    g_widget : Widget;
    g_count : INT;
END_VAR

PROGRAM Main
VAR
    local : INT := 3;
END_VAR
    g_count := g_count + local;
END_PROGRAM
"#;

    // plcc_globals layout: { Widget{ i16, i16, i16, i16 }, i16 g_count }
    let g = run_init_with_globals(source, "main_init", 32, 16);
    assert_eq!(read_i16(&g, 0), 7, "g_widget.gain");
    assert_eq!(read_i16(&g, 2), 11, "g_widget.status");
    assert_eq!(read_i16(&g, 4), 23, "g_widget.level");
    assert_eq!(read_i16(&g, 6), 0, "g_widget.untouched");
    assert_eq!(read_i16(&g, 8), 0, "g_count has no initializer");
}

// ---------------------------------------------------------------------------
// 6. Constant *expression* initializers (not just literals)
// ---------------------------------------------------------------------------

#[test]
fn fb_expression_initializers_applied() {
    let source = r#"
FUNCTION_BLOCK Calc
VAR
    sum : INT := 3 + 4;
    prod : INT := 3 + 4 * 2;
    neg : INT := 0 - 12;
END_VAR
    sum := sum;
END_FUNCTION_BLOCK
"#;

    let state = run_init(source, "calc_init", 32);
    assert_eq!(read_i16(&state, 0), 7, "3 + 4");
    assert_eq!(read_i16(&state, 2), 11, "3 + 4 * 2");
    assert_eq!(read_i16(&state, 4), -12, "0 - 12");
}

// ---------------------------------------------------------------------------
// 7. REAL members — the pid_simple.st shape
// ---------------------------------------------------------------------------

#[test]
fn fb_real_member_initializers_applied() {
    let source = r#"
FUNCTION_BLOCK Ctrl
VAR_INPUT
    kp : REAL := 1.5;
    dt : REAL := 0.25;
END_VAR
VAR_OUTPUT
    out : REAL := -2.5;
END_VAR
VAR
    acc : REAL;
END_VAR
    acc := acc;
END_FUNCTION_BLOCK
"#;

    // Ctrl layout: { f32 kp, f32 dt, f32 out, f32 acc } = 16 bytes.
    let state = run_init(source, "ctrl_init", 32);
    assert_eq!(read_f32(&state, 0), 1.5_f32, "kp");
    assert_eq!(read_f32(&state, 4), 0.25_f32, "dt");
    assert_eq!(read_f32(&state, 8), -2.5_f32, "out");
    assert_eq!(read_f32(&state, 12), 0.0_f32, "acc has no initializer");
}

// ---------------------------------------------------------------------------
// 8. Width coercion — an integer literal initializing a wider / float member
// ---------------------------------------------------------------------------

#[test]
fn fb_initializer_width_coercion() {
    let source = r#"
FUNCTION_BLOCK Widths
VAR
    d : DINT := 0 - 1;
    r : REAL := 3;
END_VAR
    d := d;
END_FUNCTION_BLOCK
"#;

    // Widths layout: { i32 d, f32 r } = 8 bytes.
    let state = run_init(source, "widths_init", 32);
    let d = i32::from_ne_bytes([state[0], state[1], state[2], state[3]]);
    assert_eq!(d, -1, "DINT := 0 - 1 must sign-extend to the full i32");
    assert_eq!(
        read_f32(&state, 4),
        3.0_f32,
        "REAL := 3 must convert to f32"
    );
}

// ---------------------------------------------------------------------------
// 9. Two instances of the same FB in one program are independently initialized
// ---------------------------------------------------------------------------

#[test]
fn two_fb_instances_both_initialized() {
    let source = format!(
        "{WIDGET_SRC}
PROGRAM Main
VAR
    a : Widget;
    b : Widget;
END_VAR
    a();
END_PROGRAM
"
    );

    let state = run_init(&source, "main_init", 32);
    assert_eq!(read_i16(&state, 0), 7, "a.gain");
    assert_eq!(read_i16(&state, 4), 23, "a.level");
    assert_eq!(read_i16(&state, 8), 7, "b.gain");
    assert_eq!(read_i16(&state, 12), 23, "b.level");
}
