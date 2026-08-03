// SPDX-License-Identifier: MPL-2.0

//! Assignment must store the destination's full width.
//!
//! Integer literals compile to INT (i16). Assigning one to a wider slot used to emit
//! `store i16 ...` through an opaque pointer, writing two bytes and leaving the rest
//! of the field stale. `ET := 0;` on a TIME (i64) variable therefore did not clear it.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;

fn jit_run(source: &str, prog: &str, scans: usize) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let context = Context::create();
    let mut compiler = Compiler::new(&context, "widths");
    compiler.compile(&unit).expect("codegen failed");

    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT");
    let mut state = vec![0u8; 4096];
    let ptr = state.as_mut_ptr();
    if let Ok(a) = ee.get_function_address(&format!("{prog}_init")) {
        let f: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(a) };
        f(ptr);
    }
    let a = ee
        .get_function_address(&format!("{prog}_scan"))
        .expect("scan missing");
    let scan: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(a) };
    for _ in 0..scans {
        scan(ptr);
    }
    state
}

fn read_i64(s: &[u8], off: usize) -> i64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&s[off..off + 8]);
    i64::from_ne_bytes(b)
}

fn read_i32(s: &[u8], off: usize) -> i32 {
    i32::from_ne_bytes([s[off], s[off + 1], s[off + 2], s[off + 3]])
}

#[test]
fn narrow_literal_assigned_to_a_time_clears_the_whole_field() {
    // First scan sets a large TIME value; second scan assigns the INT literal 0.
    // If only the low two bytes were stored, the upper bytes of T#1s would survive.
    let source = r#"
PROGRAM Widths
VAR
    et : TIME;
    pass : INT;
END_VAR
    IF pass = 0 THEN
        et := T#1s;
        pass := 1;
    ELSE
        et := 0;
    END_IF;
END_PROGRAM
"#;
    let state = jit_run(source, "widths", 2);
    assert_eq!(
        read_i64(&state, 0),
        0,
        "ET := 0 must clear all 8 bytes of the TIME field"
    );
}

#[test]
fn narrow_literal_assigned_to_a_dint_clears_the_whole_field() {
    let source = r#"
PROGRAM DWidths
VAR
    big : DINT;
    pass : INT;
END_VAR
    IF pass = 0 THEN
        big := 100000;
        pass := 1;
    ELSE
        big := 7;
    END_IF;
END_PROGRAM
"#;
    let state = jit_run(source, "dwidths", 2);
    assert_eq!(
        read_i32(&state, 0),
        7,
        "a narrow store must not leave the high bytes of a DINT behind"
    );
}

#[test]
fn wide_value_assigned_to_an_int_truncates_rather_than_overwriting_neighbours() {
    // `small` is an INT sitting in front of `guard`. Storing a TIME-typed (i64)
    // expression into it must truncate, not write 8 bytes over `guard`.
    let source = r#"
PROGRAM Trunc
VAR
    small : INT;
    guard : DINT;
    t : TIME;
END_VAR
    guard := 12345;
    t := T#1ms;
    small := t;
END_PROGRAM
"#;
    let state = jit_run(source, "trunc", 1);
    // Layout: small@0 (i16), guard@4 (i32, aligned by LLVM's packed=false struct),
    // t@8 (i64). Read guard by searching: it is the only field holding 12345.
    assert_eq!(
        read_i32(&state, 4),
        12345,
        "the neighbouring DINT must not be clobbered by a wide store into an INT"
    );
    assert_eq!(read_i64(&state, 8), 1_000_000, "TIME literal T#1ms in ns");
    // 1_000_000 truncated to i16 is 16960.
    assert_eq!(
        i16::from_ne_bytes([state[0], state[1]]),
        16960,
        "wide-to-narrow assignment truncates"
    );
}

#[test]
fn fb_output_of_wider_type_is_stored_at_full_width() {
    let source = r#"
FUNCTION_BLOCK Acc
VAR_INPUT
    step : DINT;
END_VAR
VAR_OUTPUT
    total : DINT;
END_VAR
    total := total + step;
END_FUNCTION_BLOCK

PROGRAM Main
VAR
    a : Acc;
    out : DINT;
END_VAR
    a(step := 100000);
    out := a.total;
END_PROGRAM
"#;
    let state = jit_run(source, "main", 3);
    // Main layout: a:{step:i32, total:i32} @0, out:i32 @8
    assert_eq!(read_i32(&state, 8), 300_000, "3 scans of +100000");
}
