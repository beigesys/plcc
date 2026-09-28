// SPDX-License-Identifier: MPL-2.0

//! Execution tests for defects that compiled cleanly and produced wrong values.
//!
//! Every test compiles ST, verifies the module, JIT-executes `p_init` then `p_scan`,
//! and asserts exact values read from the PROGRAM state struct. Result variables are
//! declared first, all of one width, so their offsets are stable.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;

fn run_scans(source: &str, scans: usize) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let ctx = Context::create();
    let mut compiler = Compiler::new(&ctx, "silent");
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
    i32::from_ne_bytes([state[o], state[o + 1], state[o + 2], state[o + 3]])
}

// ---------------------------------------------------------------------------
// RETURN leaves the POU immediately
// ---------------------------------------------------------------------------

#[test]
fn return_in_program_skips_rest_of_scan() {
    let src = r#"
PROGRAM p
VAR
    a : DINT;
    b : DINT;
    n : DINT;
END_VAR
    n := n + 1;
    a := 1;
    IF n > 1 THEN
        RETURN;
    END_IF;
    b := b + 10;
END_PROGRAM
"#;
    let state = run_scans(src, 3);
    assert_eq!(dint(&state, 0), 1);
    assert_eq!(dint(&state, 1), 10, "only the first scan reaches b := b + 10");
    assert_eq!(dint(&state, 2), 3);
}

#[test]
fn return_unconditional_in_program() {
    let src = r#"
PROGRAM p
VAR
    a : DINT;
    b : DINT;
END_VAR
    a := 1;
    RETURN;
    b := 2;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 1);
    assert_eq!(dint(&state, 1), 0, "code after RETURN must not run");
}

#[test]
fn return_in_function_keeps_result_assigned_so_far() {
    let src = r#"
FUNCTION CLAMP10 : DINT
VAR_INPUT
    x : DINT;
END_VAR
    CLAMP10 := x;
    IF x > 10 THEN
        CLAMP10 := 10;
        RETURN;
    END_IF;
    CLAMP10 := CLAMP10 * 2;
END_FUNCTION

PROGRAM p
VAR
    r1 : DINT;
    r2 : DINT;
END_VAR
    r1 := CLAMP10(3);
    r2 := CLAMP10(50);
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 6);
    assert_eq!(dint(&state, 1), 10, "RETURN must return the value assigned so far");
}

#[test]
fn return_from_inside_nested_loops() {
    let src = r#"
FUNCTION SEEK : DINT
VAR_INPUT
    target : DINT;
END_VAR
VAR
    i : DINT;
    j : DINT;
END_VAR
    SEEK := -1;
    FOR i := 0 TO 9 DO
        j := 0;
        WHILE j < 10 DO
            IF i * 10 + j = target THEN
                SEEK := i * 100 + j;
                RETURN;
            END_IF;
            j := j + 1;
        END_WHILE;
    END_FOR;
    SEEK := -2;
END_FUNCTION

PROGRAM p
VAR
    hit : DINT;
    miss : DINT;
END_VAR
    hit := SEEK(47);
    miss := SEEK(500);
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 407);
    assert_eq!(dint(&state, 1), -2);
}

#[test]
fn return_from_case_and_repeat() {
    let src = r#"
FUNCTION PICK : DINT
VAR_INPUT
    sel : DINT;
END_VAR
VAR
    k : DINT;
END_VAR
    CASE sel OF
        1:
            PICK := 100;
            RETURN;
        2:
            REPEAT
                k := k + 1;
                IF k = 3 THEN
                    PICK := 200 + k;
                    RETURN;
                END_IF;
            UNTIL k > 10
            END_REPEAT;
    END_CASE;
    PICK := PICK + 1;
END_FUNCTION

PROGRAM p
VAR
    a : DINT;
    b : DINT;
    c : DINT;
END_VAR
    a := PICK(1);
    b := PICK(2);
    c := PICK(3);
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 100);
    assert_eq!(dint(&state, 1), 203);
    assert_eq!(dint(&state, 2), 1);
}

#[test]
fn return_in_function_block_body() {
    let src = r#"
FUNCTION_BLOCK FB
VAR_INPUT
    en : BOOL;
END_VAR
VAR_OUTPUT
    cnt : DINT;
    after : DINT;
END_VAR
    IF NOT en THEN
        RETURN;
    END_IF;
    cnt := cnt + 1;
    after := 7;
END_FUNCTION_BLOCK

PROGRAM p
VAR
    c : DINT;
    a : DINT;
    f : FB;
    g : FB;
END_VAR
    f(en := TRUE);
    g(en := FALSE);
    c := f.cnt * 100 + g.cnt;
    a := f.after * 100 + g.after;
END_PROGRAM
"#;
    let state = run_scans(src, 2);
    assert_eq!(dint(&state, 0), 200);
    assert_eq!(dint(&state, 1), 700);
}

#[test]
fn return_in_method() {
    let src = r#"
FUNCTION_BLOCK FB
VAR
    hits : DINT;
END_VAR
METHOD Check : DINT
VAR_INPUT
    x : DINT;
END_VAR
    Check := 1;
    IF x < 0 THEN
        RETURN;
    END_IF;
    hits := hits + 1;
    Check := 2;
END_METHOD
END_FUNCTION_BLOCK

PROGRAM p
VAR
    r1 : DINT;
    r2 : DINT;
    f : FB;
END_VAR
    r1 := f.Check(-5);
    r2 := f.Check(5);
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 1);
    assert_eq!(dint(&state, 1), 2);
}

#[test]
fn return_as_last_statement_and_twice() {
    // RETURN right before END_*, and two RETURNs in a row: both must still verify.
    let src = r#"
FUNCTION F : DINT
    F := 4;
    RETURN;
    RETURN;
END_FUNCTION

PROGRAM p
VAR
    r : DINT;
END_VAR
    r := F();
    RETURN;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 4);
}
