// SPDX-License-Identifier: MPL-2.0

//! IEC 61131-3 type system conformance tests.
//!
//! Tests verify the type checker enforces the IEC 61131-3:2013 type hierarchy
//! and implicit conversion rules. Each test parses a snippet via
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

// ===========================================================================
// Negative tests — should produce errors
// ===========================================================================

// 1. BOOL + INT should error: BOOL is not in ANY_NUM
#[test]
fn iec_any_num_required_for_arithmetic() {
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
    assert!(
        !errors.is_empty(),
        "BOOL + INT should produce an error (BOOL not in ANY_NUM)"
    );
    let has_type_mismatch = errors
        .iter()
        .any(|e| matches!(e, CheckError::TypeMismatch { .. }));
    assert!(
        has_type_mismatch,
        "expected TypeMismatch for BOOL + INT, got: {errors:?}"
    );
}

// 2. INT AND REAL should error: REAL is not in ANY_BIT
#[test]
fn iec_any_bit_required_for_logical() {
    let src = r#"
        PROGRAM test
        VAR
            i : INT;
            r : REAL;
            result : INT;
        END_VAR
            result := i AND r;
        END_PROGRAM
    "#;
    let errors = check_src(src);
    assert!(
        !errors.is_empty(),
        "INT AND REAL should produce an error (REAL not in ANY_BIT)"
    );
    let has_type_mismatch = errors
        .iter()
        .any(|e| matches!(e, CheckError::TypeMismatch { .. }));
    assert!(
        has_type_mismatch,
        "expected TypeMismatch for INT AND REAL, got: {errors:?}"
    );
}

// 3. Implicit widening: SINT -> INT -> DINT -> LINT allowed
#[test]
fn iec_implicit_widening_allowed() {
    let src = r#"
        PROGRAM test
        VAR
            s : SINT;
            i : INT;
            d : DINT;
            l : LINT;
        END_VAR
            i := s;
            d := i;
            l := d;
        END_PROGRAM
    "#;
    let errors = check_src(src);
    assert!(
        errors.is_empty(),
        "SINT -> INT -> DINT -> LINT widening should be allowed, got: {errors:?}"
    );
}

// 4. Implicit INT to REAL allowed
#[test]
fn iec_implicit_int_to_real_allowed() {
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

// 5. No implicit narrowing: DINT -> INT should error
#[test]
fn iec_no_implicit_narrowing() {
    let src = r#"
        PROGRAM test
        VAR
            d : DINT;
            i : INT;
        END_VAR
            i := d;
        END_PROGRAM
    "#;
    let errors = check_src(src);
    assert!(
        !errors.is_empty(),
        "DINT -> INT narrowing should produce an error"
    );
    let has_type_mismatch = errors
        .iter()
        .any(|e| matches!(e, CheckError::TypeMismatch { .. }));
    assert!(
        has_type_mismatch,
        "expected TypeMismatch for DINT -> INT narrowing, got: {errors:?}"
    );
}

// 6. No implicit REAL to INT
#[test]
fn iec_no_implicit_real_to_int() {
    let src = r#"
        PROGRAM test
        VAR
            r : REAL;
            i : INT;
        END_VAR
            i := r;
        END_PROGRAM
    "#;
    let errors = check_src(src);
    assert!(
        !errors.is_empty(),
        "REAL -> INT should produce an error (no implicit narrowing)"
    );
    let has_type_mismatch = errors
        .iter()
        .any(|e| matches!(e, CheckError::TypeMismatch { .. }));
    assert!(
        has_type_mismatch,
        "expected TypeMismatch for REAL -> INT, got: {errors:?}"
    );
}

// 7. IF condition must be BOOL
#[test]
fn iec_bool_condition_required() {
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
        "IF with INT condition should produce an error"
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
        "expected TypeMismatch with expected=BOOL for IF condition, got: {errors:?}"
    );
}

// 8. FOR loop variable must be numeric (not BOOL)
#[test]
fn iec_for_variable_must_be_numeric() {
    let src = r#"
        PROGRAM test
        VAR
            b : BOOL;
            x : INT;
        END_VAR
            FOR b := FALSE TO TRUE DO
                x := x + 1;
            END_FOR;
        END_PROGRAM
    "#;
    let errors = check_src(src);
    assert!(
        !errors.is_empty(),
        "FOR with BOOL iterator variable should produce an error"
    );
    // The error could be TypeMismatch or General — just verify it's rejected
    let has_relevant_error = errors.iter().any(|e| {
        matches!(
            e,
            CheckError::TypeMismatch { .. } | CheckError::General { .. }
        )
    });
    assert!(
        has_relevant_error,
        "expected a type error for BOOL FOR variable, got: {errors:?}"
    );
}
