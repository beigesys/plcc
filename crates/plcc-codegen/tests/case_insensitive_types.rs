// SPDX-License-Identifier: MPL-2.0

//! Type names are ST identifiers, and ST identifiers are case-insensitive.
//!
//! Type lookup matched the stored spelling exactly, so `t : Ton;`, `f : myfb;` and
//! `c : complex;` were all rejected with "unknown type". That is valid, idiomatic ST,
//! and it broke real corpora — OSCAT declares `TYPE COMPLEX` and then writes
//! `complex` in `FUNCTION CABS`, so the whole 559-file corpus failed to compile as a
//! unit on the type name alone.
//!
//! Every other name lookup in the compiler already folds case (variables and FB
//! instances key on `to_uppercase()`, fields and methods use `eq_ignore_ascii_case`,
//! LLVM function names are lowercased); type lookup was the only holdout.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;

fn compile(source: &str) -> Compiler<'static> {
    // The context has to outlive the compiler; leak one per test.
    let ctx: &'static Context = Box::leak(Box::new(Context::create()));
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let mut compiler = Compiler::new(ctx, "case");
    compiler.compile(&unit).expect("codegen failed");
    compiler
}

fn jit_scan(source: &str, pou: &str) -> Vec<u8> {
    let compiler = compile(source);
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

fn read_i16(state: &[u8], offset: usize) -> i16 {
    i16::from_ne_bytes([state[offset], state[offset + 1]])
}

/// A user TYPE spelled one way and used another — the OSCAT `COMPLEX`/`complex`
/// shape, reduced.
#[test]
fn a_user_type_resolves_regardless_of_spelling() {
    let source = r#"
TYPE
    Complex : STRUCT
        re : INT;
        im : INT;
    END_STRUCT;
END_TYPE

PROGRAM UseType
VAR
    c : complex;
    d : COMPLEX;
    n : INT;
END_VAR
    c.re := 3;
    d.re := 4;
    n := c.re + d.re;
END_PROGRAM
"#;
    // { { i16 re, i16 im } c, { i16, i16 } d, i16 n }
    let state = jit_scan(source, "usetype");
    assert_eq!(read_i16(&state, 0), 3, "c.re");
    assert_eq!(read_i16(&state, 4), 4, "d.re");
    assert_eq!(read_i16(&state, 8), 7, "both spellings named the same type");
}

/// A user FUNCTION_BLOCK instantiated under a different case, and actually scanned.
#[test]
fn a_user_function_block_resolves_regardless_of_spelling() {
    let source = r#"
FUNCTION_BLOCK MyFb
VAR_INPUT
    x : INT;
END_VAR
VAR_OUTPUT
    y : INT;
END_VAR
    y := x + 1;
END_FUNCTION_BLOCK

PROGRAM UseFb
VAR
    f : myfb;
    out : INT;
END_VAR
    f(x := 41);
    out := f.y;
END_PROGRAM
"#;
    // { { i16 x, i16 y } f, i16 out }
    let state = jit_scan(source, "usefb");
    assert_eq!(read_i16(&state, 2), 42, "the FB ran and produced 41 + 1");
    assert_eq!(read_i16(&state, 4), 42, "and the program read it back");
}

/// A bundled standard FB in mixed case. `Ton` is the reported spelling.
#[test]
fn a_bundled_standard_block_resolves_regardless_of_spelling() {
    let source = r#"
PROGRAM UseTon
VAR
    t : Ton;
    go : BOOL;
    q : BOOL;
END_VAR
    t(IN := go, PT := T#100ms);
    q := t.Q;
END_PROGRAM
"#;
    // The prelude is not injected by the codegen crate, so a bare `Ton` has no
    // definition here — but the *diagnostic* must be about TON being absent, and the
    // lookup must be the case-folded one. Define it locally under another spelling.
    let with_defn = source.replace(
        "PROGRAM UseTon",
        r#"
FUNCTION_BLOCK TON
VAR_INPUT
    IN : BOOL;
    PT : TIME;
END_VAR
VAR_OUTPUT
    Q : BOOL;
END_VAR
    Q := IN;
END_FUNCTION_BLOCK

PROGRAM UseTon"#,
    );
    let compiler = compile(&with_defn);
    let ir = compiler.emit_ir();
    assert!(
        ir.contains("call void @ton_scan("),
        "`t : Ton;` must resolve to FUNCTION_BLOCK TON and be scanned:\n{ir}"
    );
}

/// Elementary type names were already folded; keep them that way.
#[test]
fn elementary_type_names_stay_case_insensitive() {
    let source = r#"
PROGRAM Elem
VAR
    a : int;
    b : Int;
    c : INT;
END_VAR
    a := 1;
    b := 2;
    c := a + b;
END_PROGRAM
"#;
    let state = jit_scan(source, "elem");
    assert_eq!(
        read_i16(&state, 4),
        3,
        "1 + 2 across three spellings of INT"
    );
}
