// SPDX-License-Identifier: MPL-2.0

//! An FB instance declared inside another FB (or used from a METHOD) must actually
//! be scanned.
//!
//! Only `compile_program` registered its FB-instance fields in `fb_instances`.
//! Everywhere else the map was left empty, so inside `FUNCTION_BLOCK OUTER` a
//! statement like `i(A := X);` was not recognized as an FB call and compiled to
//! nothing — `outer_scan` came out as a load and `ret void`. No diagnostic, no
//! undefined symbol: the inner block simply never ran.
//!
//! Nesting is how every non-trivial ST program is built (a PID with a TON inside, a
//! sequencer with an R_TRIG inside), so these assert the actual values the chain
//! produces, over several scans where state matters.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;

fn jit_scans(source: &str, pou: &str, scans: usize) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let ctx = Context::create();
    let mut compiler = Compiler::new(&ctx, "nested");
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

fn read_i16(state: &[u8], offset: usize) -> i16 {
    i16::from_ne_bytes([state[offset], state[offset + 1]])
}

#[test]
fn an_fb_instance_inside_an_fb_is_scanned() {
    let source = r#"
FUNCTION_BLOCK INNER
VAR_INPUT
    A : INT;
END_VAR
VAR_OUTPUT
    B : INT;
END_VAR
    B := A + 1;
END_FUNCTION_BLOCK

FUNCTION_BLOCK OUTER
VAR_INPUT
    X : INT;
END_VAR
VAR_OUTPUT
    Y : INT;
END_VAR
VAR
    i : INNER;
END_VAR
    i(A := X);
    Y := i.B;
END_FUNCTION_BLOCK

PROGRAM P
VAR
    o : OUTER;
    n : INT;
END_VAR
    o(X := 5);
    n := o.Y;
END_PROGRAM
"#;
    // { { i16 X, i16 Y, { i16 A, i16 B } i } o, i16 n }
    let state = jit_scans(source, "p", 1);
    assert_eq!(read_i16(&state, 4), 5, "the inner FB received A := X");
    assert_eq!(read_i16(&state, 6), 6, "inner_scan ran and set B := A + 1");
    assert_eq!(read_i16(&state, 2), 6, "OUTER read i.B back into Y");
    assert_eq!(read_i16(&state, 8), 6, "and the program read o.Y");
}

#[test]
fn nested_instance_state_persists_across_scans() {
    // A dropped scan call is indistinguishable from a zeroed output on scan one. An
    // accumulator over several scans is not.
    let source = r#"
FUNCTION_BLOCK ACC
VAR_INPUT
    STEP : INT;
END_VAR
VAR_OUTPUT
    TOTAL : INT;
END_VAR
    TOTAL := TOTAL + STEP;
END_FUNCTION_BLOCK

FUNCTION_BLOCK WRAP
VAR_INPUT
    S : INT;
END_VAR
VAR_OUTPUT
    T : INT;
END_VAR
VAR
    a : ACC;
END_VAR
    a(STEP := S);
    T := a.TOTAL;
END_FUNCTION_BLOCK

PROGRAM P2
VAR
    w : WRAP;
    n : INT;
END_VAR
    w(S := 3);
    n := w.T;
END_PROGRAM
"#;
    // { { i16 S, i16 T, { i16 STEP, i16 TOTAL } a } w, i16 n }
    let state = jit_scans(source, "p2", 4);
    assert_eq!(read_i16(&state, 6), 12, "3 accumulated over four scans");
    assert_eq!(read_i16(&state, 2), 12, "WRAP forwarded it");
    assert_eq!(read_i16(&state, 8), 12, "and the program saw it");
}

#[test]
fn three_levels_of_nesting_all_run() {
    let source = r#"
FUNCTION_BLOCK L3
VAR_INPUT
    V : INT;
END_VAR
VAR_OUTPUT
    R : INT;
END_VAR
    R := V * 2;
END_FUNCTION_BLOCK

FUNCTION_BLOCK L2
VAR_INPUT
    V : INT;
END_VAR
VAR_OUTPUT
    R : INT;
END_VAR
VAR
    c : L3;
END_VAR
    c(V := V + 1);
    R := c.R;
END_FUNCTION_BLOCK

FUNCTION_BLOCK L1
VAR_INPUT
    V : INT;
END_VAR
VAR_OUTPUT
    R : INT;
END_VAR
VAR
    b : L2;
END_VAR
    b(V := V + 1);
    R := b.R;
END_FUNCTION_BLOCK

PROGRAM P3
VAR
    a : L1;
    n : INT;
END_VAR
    a(V := 1);
    n := a.R;
END_PROGRAM
"#;
    // (((1 + 1) + 1) * 2) = 6
    // { { i16 V, i16 R, { i16 V, i16 R, { i16 V, i16 R } c } b } a, i16 n }
    // a.V@0 a.R@2 b.V@4 b.R@6 c.V@8 c.R@10 n@12
    let state = jit_scans(source, "p3", 1);
    assert_eq!(
        read_i16(&state, 10),
        6,
        "the innermost block computed 3 * 2"
    );
    assert_eq!(read_i16(&state, 6), 6, "L2 forwarded it");
    assert_eq!(read_i16(&state, 2), 6, "L1 forwarded it");
    assert_eq!(read_i16(&state, 12), 6, "the program read a.R");
}

#[test]
fn an_fb_instance_used_from_a_method_is_scanned() {
    let source = r#"
FUNCTION_BLOCK DOUBLER
VAR_INPUT
    V : INT;
END_VAR
VAR_OUTPUT
    R : INT;
END_VAR
    R := V * 2;
END_FUNCTION_BLOCK

FUNCTION_BLOCK HOST
VAR
    d : DOUBLER;
END_VAR
VAR_INPUT
    IN : INT;
END_VAR
VAR_OUTPUT
    OUT : INT;
END_VAR
METHOD Twice : INT
VAR_INPUT
    v : INT;
END_VAR
    d(V := v);
    Twice := d.R;
END_METHOD
    OUT := IN;
END_FUNCTION_BLOCK

PROGRAM P4
VAR
    h : HOST;
    n : INT;
END_VAR
    h(IN := 1);
    n := h.Twice(v := 21);
END_PROGRAM
"#;
    // { { { i16 V, i16 R } d, i16 IN, i16 OUT } h, i16 n }
    // d.V@0 d.R@2 IN@4 OUT@6 n@8
    let state = jit_scans(source, "p4", 1);
    assert_eq!(read_i16(&state, 0), 21, "the method bound d.V := v");
    assert_eq!(
        read_i16(&state, 2),
        42,
        "the method scanned d and got 21 * 2"
    );
    assert_eq!(read_i16(&state, 6), 1, "host_scan still ran: OUT := IN");
    assert_eq!(read_i16(&state, 8), 42, "the program read the return value");
}
