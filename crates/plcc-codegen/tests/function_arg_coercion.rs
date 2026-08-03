// SPDX-License-Identifier: MPL-2.0

//! Arguments to a user FUNCTION are coerced to the declared parameter type.
//!
//! FB inputs and METHOD arguments were already widened at the call site; plain
//! FUNCTION calls were not. The argument reached `build_call` at whatever width the
//! expression happened to produce, so `F(5)` against `FUNCTION F : DINT VAR_INPUT
//! x : DINT` emitted `call i32 @f(i16 5)`.
//!
//! That was invisible until the module verifier was switched on, at which point it
//! became a hard `Call parameter type does not match function signature!` on ordinary
//! ST. Passing an integer literal to a DINT parameter is about as common as ST gets,
//! and nothing in the fixtures, demos, or OSCAT happened to do it.
//!
//! These execute the compiled code and assert the returned values.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;

/// Run `<pou>_init` then one `<pou>_scan` over a zeroed state block.
fn jit_scan(source: &str, pou: &str) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");

    let ctx = Context::create();
    let mut compiler = Compiler::new(&ctx, "fnargs");
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
    f(ptr);
    state
}

fn read_i32(state: &[u8], offset: usize) -> i32 {
    i32::from_ne_bytes(state[offset..offset + 4].try_into().expect("4 bytes"))
}

fn read_i64(state: &[u8], offset: usize) -> i64 {
    i64::from_ne_bytes(state[offset..offset + 8].try_into().expect("8 bytes"))
}

/// The exact shape the merge gate reported: a narrow literal into a DINT parameter.
#[test]
fn integer_literal_widens_to_a_dint_parameter() {
    let src = r#"
FUNCTION SCALE : DINT
VAR_INPUT x : DINT; END_VAR
    SCALE := x * 10;
END_FUNCTION

PROGRAM LITARG
VAR n : DINT := 0; END_VAR
    n := SCALE(5);
END_PROGRAM
"#;
    let state = jit_scan(src, "litarg");
    assert_eq!(read_i32(&state, 0), 50, "SCALE(5) should be 50");
}

/// A narrow *variable*, not just a literal, into a wider parameter.
#[test]
fn narrow_variable_widens_to_a_wider_parameter() {
    let src = r#"
FUNCTION TWICE : LINT
VAR_INPUT v : LINT; END_VAR
    TWICE := v * 2;
END_FUNCTION

PROGRAM NARROWVAR
VAR
    small : INT  := 21;
    out   : LINT := 0;
END_VAR
    out := TWICE(small);
END_PROGRAM
"#;
    let state = jit_scan(src, "narrowvar");
    // small(INT) is 2 bytes, then out(LINT) aligns to 8.
    assert_eq!(read_i64(&state, 8), 42, "TWICE(small) should be 42");
}

/// Widening must take its sign from the SOURCE, exactly as assignments do. A BYTE
/// holding 16#FF is 255, so a DINT parameter must receive 255 and not -1.
#[test]
fn any_bit_argument_zero_extends_into_a_signed_parameter() {
    let src = r#"
FUNCTION IDENT : DINT
VAR_INPUT v : DINT; END_VAR
    IDENT := v;
END_FUNCTION

PROGRAM BITARG
VAR
    raw : BYTE := 16#FF;
    out : DINT := 0;
END_VAR
    out := IDENT(raw);
END_PROGRAM
"#;
    let state = jit_scan(src, "bitarg");
    assert_eq!(
        read_i32(&state, 4),
        255,
        "a BYTE of 16#FF passed to a DINT parameter is 255, not -1"
    );
}

/// A signed source still sign-extends — the fix must not zero-extend everything.
#[test]
fn signed_argument_still_sign_extends() {
    let src = r#"
FUNCTION IDENT2 : DINT
VAR_INPUT v : DINT; END_VAR
    IDENT2 := v;
END_FUNCTION

PROGRAM SIGNEDARG
VAR
    neg : SINT := -7;
    out : DINT := 0;
END_VAR
    out := IDENT2(neg);
END_PROGRAM
"#;
    let state = jit_scan(src, "signedarg");
    assert_eq!(read_i32(&state, 4), -7, "a SINT of -7 stays -7");
}

/// Multiple parameters of differing widths in one call.
#[test]
fn mixed_width_parameters_each_coerce_independently() {
    let src = r#"
FUNCTION COMBINE : LINT
VAR_INPUT
    a : DINT;
    b : LINT;
    c : INT;
END_VAR
    COMBINE := a * 100 + b * 10 + c;
END_FUNCTION

PROGRAM MIXEDARGS
VAR
    out : LINT := 0;
END_VAR
    out := COMBINE(1, 2, 3);
END_PROGRAM
"#;
    let state = jit_scan(src, "mixedargs");
    assert_eq!(read_i64(&state, 0), 123, "COMBINE(1,2,3) should be 123");
}

/// A wider argument narrowing into a smaller parameter is the mirror direction.
#[test]
fn wide_argument_narrows_to_a_smaller_parameter() {
    let src = r#"
FUNCTION SMALL : INT
VAR_INPUT v : INT; END_VAR
    SMALL := v + 1;
END_FUNCTION

PROGRAM NARROWING
VAR
    big : LINT := 41;
    out : INT  := 0;
END_VAR
    out := SMALL(big);
END_PROGRAM
"#;
    let state = jit_scan(src, "narrowing");
    assert_eq!(read_i64(&state, 0), 41, "big unchanged");
    assert_eq!(
        i16::from_ne_bytes(state[8..10].try_into().expect("2 bytes")),
        42,
        "SMALL(big) should be 42"
    );
}
