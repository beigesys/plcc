// SPDX-License-Identifier: MPL-2.0

//! Output bindings in calls: `fb(IN := a, Q => x)`, `f(a, o => x)`, `inst.M(o => x)`.
//!
//! IEC 61131-3 §6.6.1.4: `name => variable` connects a VAR_OUTPUT to a variable, which
//! receives the output's value *after* the call. plcc used to treat `=>` exactly like
//! `:=` — it stored the target's value into the output slot before the call and never
//! copied anything back, so `x` was never written.
//!
//! Every test JIT-executes `p_init` then `p_scan` and reads the PROGRAM state struct.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;

fn compile_only(source: &str) -> Result<(), String> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let ctx = Context::create();
    let mut compiler = Compiler::new(&ctx, "outputs");
    compiler.compile(&unit).map_err(|e| e.to_string())
}

fn run_scans(source: &str, scans: usize) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let ctx = Context::create();
    let mut compiler = Compiler::new(&ctx, "outputs");
    compiler.compile(&unit).expect("codegen failed");
    if let Err(e) = compiler.module().verify() {
        panic!("invalid IR: {e}\n{}", compiler.emit_ir());
    }
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
    i32::from_ne_bytes(state[o..o + 4].try_into().unwrap())
}

fn real(state: &[u8], idx: usize) -> f32 {
    let o = idx * 4;
    f32::from_ne_bytes(state[o..o + 4].try_into().unwrap())
}

fn cstr(state: &[u8], off: usize) -> String {
    let end = state[off..].iter().position(|&b| b == 0).unwrap() + off;
    String::from_utf8(state[off..end].to_vec()).unwrap()
}

const FB_F: &str = r#"
FUNCTION_BLOCK F
VAR_INPUT
    a : DINT;
END_VAR
VAR_OUTPUT
    q : BOOL;
    n : DINT;
    r : REAL;
    w : INT;
END_VAR
    q := a > 5;
    n := a * 2;
    r := DINT_TO_REAL(a) / 4.0;
    w := DINT_TO_INT(a) + 1;
END_FUNCTION_BLOCK
"#;

#[test]
fn fb_scalar_outputs_are_copied_after_the_call() {
    let src = format!(
        "{FB_F}
PROGRAM p
VAR
    n : DINT;
    r : REAL;
    wide : DINT;
    q : BOOL;
    inst : F;
END_VAR
    inst(a := 7, q => q, n => n, r => r, w => wide);
END_PROGRAM
"
    );
    let s = run(&src);
    assert_eq!(dint(&s, 0), 14, "n => n");
    assert_eq!(real(&s, 1), 1.75, "r => r");
    assert_eq!(dint(&s, 2), 8, "INT output w widened into a DINT target");
    assert_eq!(s[12], 1, "q => q");
}

#[test]
fn fb_output_is_read_after_the_body_runs() {
    // `x` feeds the input and receives the output: after k scans x == k.
    let src = r#"
FUNCTION_BLOCK INC
VAR_INPUT a : DINT; END_VAR
VAR_OUTPUT o : DINT; END_VAR
    o := a + 1;
END_FUNCTION_BLOCK
PROGRAM p
VAR
    x : DINT;
    inst : INC;
END_VAR
    inst(a := x, o => x);
END_PROGRAM
"#;
    let s = run_scans(src, 3);
    assert_eq!(dint(&s, 0), 3);
}

#[test]
fn fb_output_does_not_write_into_the_instance() {
    // Treated as an input, `o => x` stored x (= 99) into the output slot before
    // the call. The FB never writes o, so its value must stay at its initial 0.
    let src = r#"
FUNCTION_BLOCK KEEP
VAR_INPUT a : DINT; END_VAR
VAR_OUTPUT o : DINT; END_VAR
    ;
END_FUNCTION_BLOCK
PROGRAM p
VAR
    x : DINT := 99;
    seen : DINT;
    inst : KEEP;
END_VAR
    inst(a := 1, o => x);
    seen := inst.o;
END_PROGRAM
"#;
    let s = run(src);
    assert_eq!(dint(&s, 0), 0, "x receives the output (0)");
    assert_eq!(dint(&s, 1), 0, "the output slot was not overwritten with x");
}

#[test]
fn fb_string_output() {
    let src = r#"
FUNCTION_BLOCK S
VAR_INPUT a : DINT; END_VAR
VAR_OUTPUT txt : STRING; END_VAR
    IF a > 0 THEN txt := 'positive'; ELSE txt := 'other'; END_IF;
END_FUNCTION_BLOCK
PROGRAM p
VAR
    t : STRING;
    inst : S;
END_VAR
    inst(a := 3, txt => t);
END_PROGRAM
"#;
    let s = run(src);
    assert_eq!(cstr(&s, 0), "positive");
}

#[test]
fn fb_struct_output() {
    let src = r#"
TYPE PT : STRUCT x : DINT; y : DINT; END_STRUCT; END_TYPE
FUNCTION_BLOCK MK
VAR_INPUT a : DINT; END_VAR
VAR_OUTPUT pt : PT; END_VAR
    pt.x := a;
    pt.y := a * 10;
END_FUNCTION_BLOCK
PROGRAM p
VAR
    v : PT;
    inst : MK;
END_VAR
    inst(a := 4, pt => v);
END_PROGRAM
"#;
    let s = run(src);
    assert_eq!(dint(&s, 0), 4);
    assert_eq!(dint(&s, 1), 40);
}

#[test]
fn fb_output_to_array_element_and_struct_field() {
    let src = format!(
        "{FB_F}
TYPE PT : STRUCT x : DINT; y : DINT; END_STRUCT; END_TYPE
PROGRAM p
VAR
    arr : ARRAY[0..2] OF DINT;
    st : PT;
    i : DINT := 2;
    inst : F;
END_VAR
    inst(a := 3, n => arr[1]);
    inst(a := 4, n => st.y);
    inst(a := 5, n => arr[i]);
END_PROGRAM
"
    );
    let s = run(&src);
    assert_eq!(dint(&s, 0), 0);
    assert_eq!(dint(&s, 1), 6, "arr[1]");
    assert_eq!(dint(&s, 2), 10, "arr[i]");
    assert_eq!(dint(&s, 3), 0, "st.x");
    assert_eq!(dint(&s, 4), 8, "st.y");
}

#[test]
fn output_of_fb_in_array_element() {
    let src = format!(
        "{FB_F}
PROGRAM p
VAR
    x : DINT;
    y : DINT;
    fbs : ARRAY[0..1] OF F;
END_VAR
    fbs[1](a := 7, n => x);
    fbs[0](a := 2, n => y);
END_PROGRAM
"
    );
    let s = run(&src);
    assert_eq!(dint(&s, 0), 14);
    assert_eq!(dint(&s, 1), 4);
}

#[test]
fn negated_output_binding() {
    // IEC 61131-3 §6.6.1.4 (Table 71): `NOT Q => x` assigns the inverted output.
    let src = format!(
        "{FB_F}
PROGRAM p
VAR
    hi : BOOL;
    lo : BOOL;
    inst : F;
    inst2 : F;
END_VAR
    inst(a := 9, NOT q => lo);
    inst2(a := 1, NOT q => hi);
END_PROGRAM
"
    );
    let s = run(&src);
    assert_eq!(s[0], 1, "NOT FALSE");
    assert_eq!(s[1], 0, "NOT TRUE");
}

#[test]
fn arrow_on_an_input_is_an_error() {
    let src = format!(
        "{FB_F}
PROGRAM p
VAR x : DINT; inst : F; END_VAR
    inst(a => x);
END_PROGRAM
"
    );
    let err = compile_only(&src).expect_err("`=>` on a VAR_INPUT must be rejected");
    assert!(err.contains("VAR_INPUT") && err.contains("=>"), "{err}");
}

#[test]
fn assign_to_an_output_in_a_call_is_an_error() {
    let src = format!(
        "{FB_F}
PROGRAM p
VAR x : DINT; inst : F; END_VAR
    inst(a := 1, n := x);
END_PROGRAM
"
    );
    let err = compile_only(&src).expect_err("`:=` on a VAR_OUTPUT must be rejected");
    assert!(err.contains("VAR_OUTPUT") && err.contains("=>"), "{err}");
}

const FN_G: &str = r#"
FUNCTION G : DINT
VAR_INPUT a : DINT; END_VAR
VAR_OUTPUT o : DINT; s : BOOL; END_VAR
    o := a * 3;
    s := a < 0;
    G := a + 1;
END_FUNCTION
"#;

#[test]
fn function_var_output_named_call() {
    let src = format!(
        "{FN_G}
PROGRAM p
VAR
    r : DINT;
    o : DINT;
    neg : BOOL;
END_VAR
    r := G(a := -2, o => o, s => neg);
END_PROGRAM
"
    );
    let s = run(&src);
    assert_eq!(dint(&s, 0), -1);
    assert_eq!(dint(&s, 1), -6);
    assert_eq!(s[8], 1);
}

#[test]
fn function_var_output_call_statement_and_positional() {
    let src = format!(
        "{FN_G}
PROGRAM p
VAR
    r : DINT;
    o2 : DINT;
END_VAR
    r := G(5);
    G(a := 7, o => o2);
END_PROGRAM
"
    );
    let s = run(&src);
    assert_eq!(dint(&s, 0), 6, "positional call leaves outputs unconnected");
    assert_eq!(dint(&s, 1), 21, "call statement");
}

#[test]
fn positional_input_with_output_binding_is_an_error() {
    // CODESYS 3: "You cannot mix explicit and implicit parameter assignments in
    // function calls" — `G(5, o => x)` included.
    let err = compile_only(&format!(
        "{FN_G}
PROGRAM p VAR x : DINT; r : DINT; END_VAR r := G(5, o => x); END_PROGRAM"
    ))
    .expect_err("mixing positional inputs with `=>` must be rejected");
    assert!(err.contains("mixed"), "{err}");
}

#[test]
fn function_output_unbound_and_reset_each_call() {
    // A FUNCTION's VAR_OUTPUT starts every call at its initial value.
    let src = r#"
FUNCTION H : DINT
VAR_INPUT a : DINT; END_VAR
VAR_OUTPUT acc : DINT := 100; END_VAR
    acc := acc + a;
    H := 0;
END_FUNCTION
PROGRAM p
VAR
    x : DINT;
    r : DINT;
END_VAR
    r := H(a := 1);
    r := H(a := 2, acc => x);
    r := H(a := 3, acc => x);
END_PROGRAM
"#;
    let s = run(src);
    assert_eq!(dint(&s, 0), 103);
}

#[test]
fn method_var_output() {
    let src = r#"
FUNCTION_BLOCK B
VAR total : DINT; END_VAR
METHOD Add : BOOL
VAR_INPUT v : DINT; END_VAR
VAR_OUTPUT now : DINT; END_VAR
    total := total + v;
    now := total;
    Add := TRUE;
END_METHOD
END_FUNCTION_BLOCK
PROGRAM p
VAR
    a : DINT;
    b : DINT;
    ok : BOOL;
    inst : B;
END_VAR
    inst.Add(v := 5, now => a);
    ok := inst.Add(v := 2, now => b);
END_PROGRAM
"#;
    let s = run(src);
    assert_eq!(dint(&s, 0), 5);
    assert_eq!(dint(&s, 1), 7);
    assert_eq!(s[8], 1);
}

#[test]
fn function_arrow_on_input_and_assign_on_output_are_errors() {
    let e1 = compile_only(&format!(
        "{FN_G}
PROGRAM p VAR x : DINT; END_VAR x := G(a => x); END_PROGRAM"
    ))
    .expect_err("`=>` on a FUNCTION input");
    assert!(e1.contains("VAR_INPUT"), "{e1}");
    let e2 = compile_only(&format!(
        "{FN_G}
PROGRAM p VAR x : DINT; END_VAR x := G(a := 1, o := x); END_PROGRAM"
    ))
    .expect_err("`:=` on a FUNCTION output");
    assert!(e2.contains("VAR_OUTPUT"), "{e2}");
}
