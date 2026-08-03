// SPDX-License-Identifier: MPL-2.0

//! Forward references between TYPE declarations resolve to a fixed point.
//!
//! Resolution used to run exactly two passes, so a two-deep forward chain worked and
//! a three-deep one was rejected as an unknown type — a property of the loop bound,
//! not of the language. Iterating instead until nothing more resolves handles a chain
//! of any depth, and a round that resolves nothing new means a genuine cycle, which
//! now gets its own diagnostic instead of an "unknown type" or an infinite loop.
//!
//! The layout assertions matter as much as the accept/reject: a chain that "resolves"
//! but lays out as an i32 is the silent miscompile all over again.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::{Compiler, compiler::CodegenError};

fn compile(source: &str) -> Result<String, CodegenError> {
    let ctx = Context::create();
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let mut compiler = Compiler::new(&ctx, "fwd");
    compiler.compile(&unit)?;
    Ok(compiler.emit_ir())
}

fn jit_scan(source: &str, pou: &str) -> Vec<u8> {
    let ctx = Context::create();
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let mut compiler = Compiler::new(&ctx, "fwd");
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

fn read_i16(state: &[u8], offset: usize) -> i16 {
    i16::from_ne_bytes([state[offset], state[offset + 1]])
}

/// Outer -> Middle -> Inner, each declared after its user. Two passes rejected this.
const THREE_DEEP: &str = r#"
TYPE
    Outer  : ARRAY[1..2] OF Middle;
    Middle : ARRAY[1..2] OF Inner;
    Inner  : STRUCT
        a : INT;
        b : INT;
    END_STRUCT;
END_TYPE

PROGRAM Fwd3
VAR
    o : Outer;
    n : INT;
END_VAR
    o[1][2].a := 5;
    o[2][1].b := 6;
    n := o[1][2].a + o[2][1].b;
END_PROGRAM
"#;

#[test]
fn a_three_deep_forward_chain_resolves_and_lays_out() {
    let ir = compile(THREE_DEEP).expect("a three-deep forward chain must compile");
    assert!(
        ir.contains("[2 x [2 x { i16, i16 }]]"),
        "Outer must lay out as a 2x2 array of the Inner struct, not as a fallback \
         scalar:\n{ir}"
    );
}

/// Same depth, expressed as an alias chain, so the resolved type can be exercised by
/// a scan rather than only inspected. (The array form above is checked structurally:
/// multi-level `o[i][j].f` lvalues are dropped by a separate, pre-existing codegen
/// bug, so it cannot carry an execution assertion yet.)
#[test]
fn a_three_deep_forward_chain_executes() {
    let source = r#"
TYPE
    Outer  : Middle;
    Middle : Inner;
    Inner  : STRUCT
        a : INT;
        b : INT;
    END_STRUCT;
END_TYPE

PROGRAM Fwd3
VAR
    o : Outer;
    n : INT;
END_VAR
    o.a := 5;
    o.b := 6;
    n := o.a + o.b;
END_PROGRAM
"#;
    // { { i16 a, i16 b } o, i16 n }
    let state = jit_scan(source, "fwd3");
    assert_eq!(read_i16(&state, 0), 5, "o.a");
    assert_eq!(read_i16(&state, 2), 6, "o.b");
    assert_eq!(read_i16(&state, 4), 11, "5 + 6 read back through the chain");
}

/// Five deep, declared in exactly reverse order — nothing special about three.
#[test]
fn a_five_deep_forward_chain_resolves() {
    let source = r#"
TYPE
    L1 : ARRAY[1..2] OF L2;
    L2 : ARRAY[1..2] OF L3;
    L3 : ARRAY[1..2] OF L4;
    L4 : ARRAY[1..2] OF L5;
    L5 : STRUCT
        v : INT;
    END_STRUCT;
END_TYPE

PROGRAM Deep
VAR
    d : L1;
    n : INT;
END_VAR
    d[1][1][1][1].v := 3;
    n := d[1][1][1][1].v;
END_PROGRAM
"#;
    let ir = compile(source).expect("a five-deep forward chain must compile");
    assert!(
        ir.contains("[2 x [2 x [2 x [2 x { i16 }]]]]"),
        "L1 must lay out four arrays deep:\n{ir}"
    );
}

#[test]
fn a_cyclic_type_gets_its_own_diagnostic() {
    let source = r#"
TYPE
    A : ARRAY[1..2] OF B;
    B : ARRAY[1..2] OF A;
END_TYPE

PROGRAM Cyc
VAR
    x : INT;
END_VAR
    x := 1;
END_PROGRAM
"#;
    let err = compile(source).expect_err("a cyclic TYPE must be rejected, not looped on");
    let msg = err.to_string();
    assert!(
        msg.contains('A') && msg.contains('B'),
        "the diagnostic must name both ends of the cycle: {msg}"
    );
    assert!(
        msg.contains("cannot be laid out"),
        "and say what went wrong: {msg}"
    );
}

/// The legal way to write a recursive type. A pointer is one machine word whatever it
/// points at, so this converges — and it must not be mistaken for a cycle.
#[test]
fn a_self_referential_type_through_a_pointer_is_accepted() {
    let source = r#"
TYPE
    Node : STRUCT
        v : INT;
        next : POINTER TO Node;
    END_STRUCT;
END_TYPE

PROGRAM Lst
VAR
    n : Node;
END_VAR
    n.v := 3;
END_PROGRAM
"#;
    let ir = compile(source).expect("a pointer-recursive TYPE must compile");
    assert!(
        ir.contains("{ i16, ptr }"),
        "Node must lay out as an INT plus a pointer:\n{ir}"
    );
}

/// The pointer relaxation must not swallow a pointee that genuinely does not exist.
#[test]
fn a_pointer_to_an_undeclared_type_is_still_rejected() {
    let source = r#"
PROGRAM Bad
VAR
    p : POINTER TO Bogus;
END_VAR
    ;
END_PROGRAM
"#;
    let err = compile(source).expect_err("POINTER TO an undeclared type must be an error");
    assert!(
        err.to_string().contains("Bogus"),
        "the diagnostic must name the missing type: {err}"
    );
}
