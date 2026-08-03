// SPDX-License-Identifier: MPL-2.0

//! FOR-loop bounds must widen with the signedness of the value they came from.
//!
//! `match_int_widths` sign-extended unconditionally, so a BYTE holding `16#FF` —
//! which is 255 — reached the comparison as `-1`:
//!
//! ```text
//! raw : BYTE := 16#FF;
//! FOR i := 1 TO raw BY 100 DO n := n + 1; END_FOR;   (* n = 0, expected 3 *)
//! ```
//!
//! `%sext16 = sext i8 %raw to i32`. The loop body simply never ran, with no
//! diagnostic. Hoisting the bound into a DINT first gave the right answer, because
//! *assignment* already went through the source-signedness-aware `coerce_value`;
//! the loop-bound path never did.
//!
//! All three parts of the loop header are covered: TO, FROM and the BY step. And
//! the signed cases are here too, so the fix cannot swing the other way and
//! zero-extend a negative SINT.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;

/// Compile, JIT, run `p_init` then one `p_scan`, and return the leading DINT — every
/// program below declares its counter `n` first.
fn run_n(source: &str) -> i32 {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let ctx = Context::create();
    let mut compiler = Compiler::new(&ctx, "forbounds");
    compiler.compile(&unit).expect("codegen failed");

    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT");
    let mut state = vec![0u8; 4096];
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
fn a_byte_upper_bound_is_zero_extended() {
    // The exact case from the report.
    let n = run_n(
        r#"
PROGRAM P
VAR
    n : DINT := 0;
    i : DINT;
    raw : BYTE := 16#FF;
END_VAR
    FOR i := 1 TO raw BY 100 DO
        n := n + 1;
    END_FOR;
END_PROGRAM
"#,
    );
    assert_eq!(n, 3, "1, 101, 201 are <= 255; 301 is not");
}

#[test]
fn a_word_upper_bound_is_zero_extended() {
    let n = run_n(
        r#"
PROGRAM P
VAR
    n : DINT := 0;
    i : DINT;
    raw : WORD := 16#FFFF;
END_VAR
    FOR i := 1 TO raw BY 20000 DO
        n := n + 1;
    END_FOR;
END_PROGRAM
"#,
    );
    assert_eq!(n, 4, "1, 20001, 40001, 60001 are <= 65535; 80001 is not");
}

#[test]
fn a_uint_upper_bound_is_zero_extended() {
    let n = run_n(
        r#"
PROGRAM P
VAR
    n : DINT := 0;
    i : DINT;
    lim : UINT := 65535;
END_VAR
    FOR i := 1 TO lim BY 20000 DO
        n := n + 1;
    END_FOR;
END_PROGRAM
"#,
    );
    assert_eq!(n, 4, "UINT is unsigned, so 65535 must not read as -1");
}

#[test]
fn a_usint_upper_bound_is_zero_extended() {
    let n = run_n(
        r#"
PROGRAM P
VAR
    n : DINT := 0;
    i : DINT;
    lim : USINT := 200;
END_VAR
    FOR i := 1 TO lim BY 100 DO
        n := n + 1;
    END_FOR;
END_PROGRAM
"#,
    );
    assert_eq!(n, 2, "USINT 200 must not read as -56");
}

#[test]
fn a_byte_lower_bound_is_zero_extended() {
    // The FROM value is stored straight into the loop variable's slot, so it needs
    // the same treatment: an i8 254 written into a DINT must arrive as 254.
    let n = run_n(
        r#"
PROGRAM P
VAR
    n : DINT := 0;
    i : DINT;
    lo : BYTE := 16#FE;
END_VAR
    FOR i := lo TO 260 DO
        n := n + 1;
    END_FOR;
END_PROGRAM
"#,
    );
    assert_eq!(n, 7, "254 through 260 inclusive");
}

#[test]
fn a_byte_step_is_zero_extended() {
    // A sign-extended BYTE step of 16#C8 is -56, which walks the loop variable
    // downward forever against a `<= 600` test. The EXIT guard keeps that from
    // hanging the test run; it fires only when the step went negative.
    let n = run_n(
        r#"
PROGRAM P
VAR
    n : DINT := 0;
    i : DINT;
    st : BYTE := 16#C8;
END_VAR
    FOR i := 0 TO 600 BY st DO
        n := n + 1;
        IF n > 50 THEN
            EXIT;
        END_IF;
    END_FOR;
END_PROGRAM
"#,
    );
    assert_eq!(n, 4, "0, 200, 400, 600 with a step of 200");
}

#[test]
fn a_signed_bound_still_sign_extends() {
    // The other direction: a negative SINT bound must not be zero-extended into a
    // large positive number.
    let n = run_n(
        r#"
PROGRAM P
VAR
    n : DINT := 0;
    i : DINT;
    hi : SINT := -5;
END_VAR
    FOR i := -10 TO hi DO
        n := n + 1;
    END_FOR;
END_PROGRAM
"#,
    );
    assert_eq!(n, 6, "-10 through -5 inclusive");
}

#[test]
fn a_signed_lower_bound_still_sign_extends() {
    let n = run_n(
        r#"
PROGRAM P
VAR
    n : DINT := 0;
    i : DINT;
    lo : SINT := -3;
END_VAR
    FOR i := lo TO 2 DO
        n := n + 1;
    END_FOR;
END_PROGRAM
"#,
    );
    assert_eq!(n, 6, "-3 through 2 inclusive");
}
