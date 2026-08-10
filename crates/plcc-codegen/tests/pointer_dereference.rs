// SPDX-License-Identifier: MPL-2.0

//! `p^` on both sides of `:=`, and `ADR` to give a pointer something to point at.
//!
//! A dereference had no lvalue path at all, so `p^ := x;` compiled to nothing an
//! assignment could store into — 39 OSCAT files stop on exactly that. `ADR` is the
//! other half: without a way to take an address, a POINTER can never be given one,
//! and the write path is untestable.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;

fn jit_run(source: &str, prog: &str) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let context = Context::create();
    let mut compiler = Compiler::new(&context, "ptr");
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
    scan(ptr);
    state
}

fn compile_err(source: &str) -> String {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let context = Context::create();
    let mut compiler = Compiler::new(&context, "ptr_err");
    match compiler.compile(&unit) {
        Ok(()) => panic!("expected a codegen error"),
        Err(e) => e.to_string(),
    }
}

fn read_i32(s: &[u8], off: usize) -> i32 {
    i32::from_ne_bytes([s[off], s[off + 1], s[off + 2], s[off + 3]])
}

#[test]
fn a_write_through_a_pointer_reaches_the_variable() {
    let source = r#"
PROGRAM PtrWrite
VAR
    target : DINT;
    p : POINTER TO DINT;
END_VAR
    p := ADR(target);
    p^ := 42;
END_PROGRAM
"#;
    let state = jit_run(source, "ptrwrite");
    assert_eq!(read_i32(&state, 0), 42, "p^ := 42 must store into target");
}

#[test]
fn a_read_through_a_pointer_loads_the_variable() {
    let source = r#"
PROGRAM PtrRead
VAR
    result : DINT;
    src : DINT := 7;
    p : POINTER TO DINT;
END_VAR
    p := ADR(src);
    result := p^ * 2;
END_PROGRAM
"#;
    let state = jit_run(source, "ptrread");
    assert_eq!(
        read_i32(&state, 0),
        14,
        "p^ must read 7, not the pointer word"
    );
}

#[test]
fn a_pointer_can_address_an_array_element() {
    let source = r#"
PROGRAM PtrArr
VAR
    arr : ARRAY[1..4] OF DINT;
    p : POINTER TO DINT;
END_VAR
    p := ADR(arr[2]);
    p^ := 9;
    arr[1] := p^ + 1;
END_PROGRAM
"#;
    let state = jit_run(source, "ptrarr");
    assert_eq!(read_i32(&state, 0), 10, "arr[1] = p^ + 1");
    assert_eq!(read_i32(&state, 4), 9, "arr[2] written through the pointer");
}

#[test]
fn a_pointer_can_address_a_struct_field() {
    let source = r#"
TYPE Counter : STRUCT
    hits : DINT;
    misses : DINT;
END_STRUCT; END_TYPE

PROGRAM PtrStruct
VAR
    c : Counter;
    p : POINTER TO DINT;
END_VAR
    p := ADR(c.misses);
    p^ := 5;
    c.hits := p^ * 3;
END_PROGRAM
"#;
    let state = jit_run(source, "ptrstruct");
    assert_eq!(read_i32(&state, 0), 15, "c.hits = c.misses * 3");
    assert_eq!(
        read_i32(&state, 4),
        5,
        "c.misses written through the pointer"
    );
}

#[test]
fn the_stored_width_follows_what_the_pointer_points_at() {
    // The narrow-literal bug in a new place: an INT literal (i16) stored through a
    // POINTER TO LINT must write all eight bytes, not two over a stale value.
    let source = r#"
PROGRAM PtrWidth
VAR
    wide : LINT;
    p : POINTER TO LINT;
    pass : INT;
END_VAR
    p := ADR(wide);
    IF pass = 0 THEN
        p^ := 16#7FFF_FFFF_FFFF;
        pass := 1;
    ELSE
        p^ := 0;
    END_IF;
END_PROGRAM
"#;
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let context = Context::create();
    let mut compiler = Compiler::new(&context, "ptrwidth");
    compiler.compile(&unit).expect("codegen failed");
    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT");
    let mut state = vec![0u8; 4096];
    let ptr = state.as_mut_ptr();
    if let Ok(a) = ee.get_function_address("ptrwidth_init") {
        let f: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(a) };
        f(ptr);
    }
    let a = ee
        .get_function_address("ptrwidth_scan")
        .expect("scan missing");
    let scan: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(a) };
    let read = |s: &[u8]| {
        let mut b = [0u8; 8];
        b.copy_from_slice(&s[0..8]);
        i64::from_ne_bytes(b)
    };
    scan(ptr);
    assert_eq!(
        read(&state),
        0x7FFF_FFFF_FFFF,
        "the first scan must write all eight bytes through the pointer"
    );
    scan(ptr);
    assert_eq!(
        read(&state),
        0,
        "the second scan's `p^ := 0` must clear all eight bytes, not just two"
    );
}

#[test]
fn dereferencing_a_non_pointer_is_reported() {
    let source = r#"
PROGRAM BadDeref
VAR
    x : DINT;
END_VAR
    x^ := 1;
END_PROGRAM
"#;
    let msg = compile_err(source);
    assert!(
        msg.contains('x') && msg.contains("POINTER"),
        "the error must name the variable and say what is wrong, got: {msg}"
    );
}

#[test]
fn taking_the_address_of_a_literal_is_reported() {
    let source = r#"
PROGRAM BadAdr
VAR
    p : POINTER TO DINT;
END_VAR
    p := ADR(3);
END_PROGRAM
"#;
    let msg = compile_err(source);
    assert!(
        msg.contains("ADR"),
        "the error must name ADR rather than fail silently, got: {msg}"
    );
}
