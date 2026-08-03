// SPDX-License-Identifier: MPL-2.0

//! An array index may sit anywhere in an assignable chain, not only directly on a
//! named variable.
//!
//! `compile_lvalue_inner`'s `ArrayIndex` arm required the indexed expression to be a
//! bare `Identifier` and returned `Ok(None)` otherwise — the same silent bail-out the
//! `MemberAccess` arm had. `Ok(None)` on an lvalue path means "emit no code", so
//!
//! ```text
//! o[1][2].a := 7;
//! n := o[1][2].a;
//! ```
//!
//! both compiled to nothing at all. `p_scan` came out as four GEPs for the variable
//! slots and `ret void`: no store, no load, no diagnostic, no missing symbol.
//!
//! The rvalue side had the same shape — `compile_expression`'s `ArrayIndex` arm read
//! the element type out of `self.variables` by identifier — so reads were dropped
//! too.
//!
//! Every test writes decoy values into neighbouring elements, so lowering the chain
//! to the *wrong* address fails just as loudly as not lowering it at all.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;

fn compile(source: &str) -> Result<(), String> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let ctx = Context::create();
    let mut compiler = Compiler::new(&ctx, "lvchain");
    compiler.compile(&unit).map_err(|e| e.to_string())
}

/// Compile, JIT, run `p_init` then one `p_scan`, and return the leading DINT — every
/// program below declares its result `n` first.
fn run_n(source: &str) -> i32 {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let ctx = Context::create();
    let mut compiler = Compiler::new(&ctx, "lvchain");
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
    f(ptr);
    i32::from_ne_bytes([state[0], state[1], state[2], state[3]])
}

#[test]
fn array_of_array_of_struct() {
    // The case from the report: `o[1][2].a`.
    let n = run_n(
        r#"
TYPE
    Elem : STRUCT
        a : DINT;
        b : DINT;
    END_STRUCT;
END_TYPE

PROGRAM P
VAR
    n : DINT;
    o : ARRAY[1..2] OF ARRAY[1..3] OF Elem;
END_VAR
    o[1][3].a := 70;
    o[2][2].a := 700;
    o[1][2].b := 7000;
    o[1][2].a := 7;
    n := o[1][2].a;
END_PROGRAM
"#,
    );
    assert_eq!(n, 7, "decoys in the neighbouring elements and in .b");
}

#[test]
fn struct_field_array_element_field() {
    // `s.arr[3].field` — the index sits on a MemberAccess.
    let n = run_n(
        r#"
TYPE
    Elem : STRUCT
        a : DINT;
        b : DINT;
    END_STRUCT;
    Holder : STRUCT
        arr : ARRAY[1..3] OF Elem;
        k : DINT;
    END_STRUCT;
END_TYPE

PROGRAM P
VAR
    n : DINT;
    s : Holder;
END_VAR
    s.k := 1000;
    s.arr[2].b := 500;
    s.arr[3].a := 50;
    s.arr[3].b := 11;
    n := s.arr[3].b;
END_PROGRAM
"#,
    );
    assert_eq!(n, 11, "s.arr[3].b, not s.arr[2].b and not s.arr[3].a");
}

#[test]
fn index_member_index_member() {
    // `a[1].b[2].c` — two indices and two members alternating.
    let n = run_n(
        r#"
TYPE
    Leaf : STRUCT
        c : DINT;
        d : DINT;
    END_STRUCT;
    Mid : STRUCT
        b : ARRAY[1..3] OF Leaf;
    END_STRUCT;
END_TYPE

PROGRAM P
VAR
    n : DINT;
    a : ARRAY[1..2] OF Mid;
END_VAR
    a[2].b[2].c := 99;
    a[1].b[3].c := 88;
    a[1].b[2].d := 77;
    a[1].b[2].c := 17;
    n := a[1].b[2].c;
END_PROGRAM
"#,
    );
    assert_eq!(n, 17, "a[1].b[2].c, with decoys on every neighbour");
}

#[test]
fn an_array_of_array_element_feeds_an_fb_input() {
    let n = run_n(
        r#"
FUNCTION_BLOCK DOUBLER
VAR_INPUT
    V : DINT;
END_VAR
VAR_OUTPUT
    OUT : DINT;
END_VAR
    OUT := V * 2;
END_FUNCTION_BLOCK

PROGRAM P
VAR
    n : DINT;
    o : ARRAY[1..2] OF ARRAY[1..3] OF DINT;
    fb : DOUBLER;
END_VAR
    o[1][1] := 5;
    o[2][3] := 21;
    fb(V := o[2][3]);
    n := fb.OUT;
END_PROGRAM
"#,
    );
    assert_eq!(n, 42, "o[2][3] is 21, doubled");
}

#[test]
fn a_chain_written_through_a_variable_index() {
    // Not just constant folding: the indices come from variables.
    let n = run_n(
        r#"
TYPE
    Elem : STRUCT
        a : DINT;
    END_STRUCT;
END_TYPE

PROGRAM P
VAR
    n : DINT;
    i : DINT := 1;
    j : DINT := 2;
    o : ARRAY[1..2] OF ARRAY[1..3] OF Elem;
END_VAR
    o[2][3].a := 999;
    o[i][j].a := 23;
    n := o[i][j].a;
END_PROGRAM
"#,
    );
    assert_eq!(n, 23, "o[1][2].a reached through index variables");
}

// ---------------------------------------------------------------------------
// The other half of the fix: an lvalue arm that recognizes a construct and
// cannot lower it now says so instead of emitting nothing.
// ---------------------------------------------------------------------------

#[test]
fn a_member_of_a_non_struct_is_a_diagnostic() {
    let err = compile(
        r#"
PROGRAM P
VAR
    x : DINT;
END_VAR
    x.foo := 1;
END_PROGRAM
"#,
    )
    .expect_err("x.foo must not compile to nothing");
    assert!(
        err.contains("foo"),
        "the diagnostic must name the member: {err}"
    );
}

#[test]
fn indexing_a_non_array_is_a_diagnostic() {
    let err = compile(
        r#"
PROGRAM P
VAR
    x : DINT;
END_VAR
    x[1] := 1;
END_PROGRAM
"#,
    )
    .expect_err("x[1] must not compile to nothing");
    assert!(
        err.contains("non-array"),
        "the diagnostic must say what went wrong: {err}"
    );
}

#[test]
fn an_unassignable_target_is_a_diagnostic() {
    // Direct representation has no codegen support. Silently dropping the store
    // produced a program that ran and did nothing.
    let err = compile(
        r#"
PROGRAM P
VAR
    x : BOOL;
END_VAR
    %QX0.0 := TRUE;
    x := TRUE;
END_PROGRAM
"#,
    )
    .expect_err("assigning to %QX0.0 must not compile to nothing");
    assert!(
        err.contains("assignable"),
        "the diagnostic must say the target is not assignable: {err}"
    );
}
