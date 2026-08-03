// SPDX-License-Identifier: MPL-2.0

//! Type-checking integration tests for plcc-hir.
//!
//! Each test parses a snippet of IEC 61131-3 Structured Text via
//! `plcc_st::parse()`, then runs `plcc_hir::check()` and asserts on
//! the resulting errors (or absence thereof).

use plcc_hir::{CheckError, check};
use plcc_st::parse;

/// Helper: parse + check, return errors only.
fn check_src(src: &str) -> Vec<CheckError> {
    let (unit, parse_errors) = parse(src);
    assert!(
        parse_errors.is_empty(),
        "unexpected parse errors: {parse_errors:?}"
    );
    let (_symbols, errors) = check(&unit);
    errors
}

// ---------------------------------------------------------------------------
// Positive tests — should produce zero CheckErrors
// ---------------------------------------------------------------------------

#[test]
fn simple_program_with_int_bool_real() {
    let src = r#"
        PROGRAM test
        VAR
            x : INT;
            y : BOOL;
            z : LREAL;
        END_VAR
            x := 42;
            y := TRUE;
            z := 3.14;
        END_PROGRAM
    "#;
    let errors = check_src(src);
    assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
}

#[test]
fn function_with_return_type_and_inputs() {
    let src = r#"
        FUNCTION add_ints : INT
        VAR_INPUT
            a : INT;
            b : INT;
        END_VAR
            add_ints := a + b;
        END_FUNCTION
    "#;
    let errors = check_src(src);
    assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
}

#[test]
fn for_loop_with_numeric_variable() {
    let src = r#"
        PROGRAM test
        VAR
            i : INT;
            total : INT;
        END_VAR
            FOR i := 0 TO 10 BY 1 DO
                total := total + i;
            END_FOR;
        END_PROGRAM
    "#;
    let errors = check_src(src);
    assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
}

#[test]
fn if_with_bool_condition() {
    let src = r#"
        PROGRAM test
        VAR
            flag : BOOL;
            x : INT;
        END_VAR
            IF flag THEN
                x := 1;
            ELSE
                x := 2;
            END_IF;
        END_PROGRAM
    "#;
    let errors = check_src(src);
    assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
}

#[test]
fn while_with_bool_condition() {
    let src = r#"
        PROGRAM test
        VAR
            running : BOOL;
            cnt : INT;
        END_VAR
            WHILE running DO
                cnt := cnt + 1;
            END_WHILE;
        END_PROGRAM
    "#;
    let errors = check_src(src);
    assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
}

#[test]
fn repeat_with_bool_condition() {
    let src = r#"
        PROGRAM test
        VAR
            done : BOOL;
            cnt : INT;
        END_VAR
            REPEAT
                cnt := cnt + 1;
            UNTIL done
            END_REPEAT;
        END_PROGRAM
    "#;
    let errors = check_src(src);
    assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
}

#[test]
fn implicit_integer_widening_sint_to_int() {
    let src = r#"
        PROGRAM test
        VAR
            small : SINT;
            big : INT;
        END_VAR
            big := small;
        END_PROGRAM
    "#;
    let errors = check_src(src);
    assert!(
        errors.is_empty(),
        "SINT -> INT widening should be allowed, got: {errors:?}"
    );
}

#[test]
fn implicit_integer_widening_int_to_dint() {
    let src = r#"
        PROGRAM test
        VAR
            a : INT;
            b : DINT;
        END_VAR
            b := a;
        END_PROGRAM
    "#;
    let errors = check_src(src);
    assert!(
        errors.is_empty(),
        "INT -> DINT widening should be allowed, got: {errors:?}"
    );
}

#[test]
fn implicit_int_to_real_conversion() {
    let src = r#"
        PROGRAM test
        VAR
            i : INT;
            r : REAL;
        END_VAR
            r := i;
        END_PROGRAM
    "#;
    let errors = check_src(src);
    assert!(
        errors.is_empty(),
        "INT -> REAL implicit conversion should be allowed, got: {errors:?}"
    );
}

#[test]
fn implicit_real_to_lreal_conversion() {
    let src = r#"
        PROGRAM test
        VAR
            r : REAL;
            lr : LREAL;
        END_VAR
            lr := r;
        END_PROGRAM
    "#;
    let errors = check_src(src);
    assert!(
        errors.is_empty(),
        "REAL -> LREAL implicit conversion should be allowed, got: {errors:?}"
    );
}

// ---------------------------------------------------------------------------
// Negative tests — should produce CheckErrors
// ---------------------------------------------------------------------------

#[test]
fn bool_plus_int_arithmetic_rejected() {
    let src = r#"
        PROGRAM test
        VAR
            b : BOOL;
            i : INT;
            result : INT;
        END_VAR
            result := b + i;
        END_PROGRAM
    "#;
    let errors = check_src(src);
    assert!(!errors.is_empty(), "BOOL + INT should produce an error");
    // Should be a type mismatch — BOOL is not in ANY_NUM
    let has_type_mismatch = errors
        .iter()
        .any(|e| matches!(e, CheckError::TypeMismatch { .. }));
    assert!(
        has_type_mismatch,
        "expected TypeMismatch for BOOL + INT, got: {errors:?}"
    );
}

#[test]
fn string_assigned_to_int_rejected() {
    let src = r#"
        PROGRAM test
        VAR
            x : INT;
        END_VAR
            x := 'hello';
        END_PROGRAM
    "#;
    let errors = check_src(src);
    assert!(
        !errors.is_empty(),
        "STRING assigned to INT should produce an error"
    );
    let has_type_mismatch = errors
        .iter()
        .any(|e| matches!(e, CheckError::TypeMismatch { .. }));
    assert!(
        has_type_mismatch,
        "expected TypeMismatch for STRING -> INT, got: {errors:?}"
    );
}

#[test]
fn assign_to_constant_rejected() {
    let src = r#"
        PROGRAM test
        VAR CONSTANT
            c : INT := 42;
        END_VAR
        VAR
            x : INT;
        END_VAR
            c := 99;
        END_PROGRAM
    "#;
    let errors = check_src(src);
    assert!(
        !errors.is_empty(),
        "assignment to CONSTANT should produce an error"
    );
    let has_const_error = errors
        .iter()
        .any(|e| matches!(e, CheckError::AssignToConstant { .. }));
    assert!(
        has_const_error,
        "expected AssignToConstant error, got: {errors:?}"
    );
}

#[test]
fn non_bool_if_condition_rejected() {
    let src = r#"
        PROGRAM test
        VAR
            x : INT;
        END_VAR
            IF x THEN
                x := 0;
            END_IF;
        END_PROGRAM
    "#;
    let errors = check_src(src);
    assert!(
        !errors.is_empty(),
        "INT as IF condition should produce an error"
    );
    let has_type_mismatch = errors.iter().any(|e| {
        matches!(
            e,
            CheckError::TypeMismatch {
                expected,
                ..
            } if expected == "BOOL"
        )
    });
    assert!(
        has_type_mismatch,
        "expected TypeMismatch with expected=BOOL, got: {errors:?}"
    );
}
