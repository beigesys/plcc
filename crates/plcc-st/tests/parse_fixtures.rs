// SPDX-License-Identifier: MPL-2.0

use std::path::Path;

fn parse_file(path: &Path) {
    let source = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    let (unit, errors) = plcc_st::parse(&source);
    assert!(
        errors.is_empty(),
        "parse errors in {}:\n{errors:#?}",
        path.display()
    );
    assert!(
        !unit.declarations.is_empty(),
        "no declarations in {}",
        path.display()
    );
}

#[test]
fn parse_fixture_literals() {
    parse_file(Path::new("../../tests/fixtures/parse/literals.st"));
}

#[test]
fn parse_fixture_expressions() {
    parse_file(Path::new("../../tests/fixtures/parse/expressions.st"));
}

#[test]
fn parse_fixture_statements() {
    parse_file(Path::new("../../tests/fixtures/parse/statements.st"));
}

#[test]
fn parse_fixture_variables() {
    parse_file(Path::new("../../tests/fixtures/parse/variables.st"));
}

#[test]
fn parse_fixture_types() {
    parse_file(Path::new("../../tests/fixtures/parse/types.st"));
}

#[test]
fn parse_fixture_pou_function() {
    parse_file(Path::new("../../tests/fixtures/parse/pou_function.st"));
}

#[test]
fn parse_fixture_pou_function_block() {
    parse_file(Path::new(
        "../../tests/fixtures/parse/pou_function_block.st",
    ));
}

#[test]
fn parse_fixture_pou_program() {
    parse_file(Path::new("../../tests/fixtures/parse/pou_program.st"));
}

#[test]
fn parse_fixture_oop() {
    parse_file(Path::new("../../tests/fixtures/parse/oop.st"));
}

#[test]
fn parse_program_blink() {
    parse_file(Path::new("../../tests/fixtures/programs/blink.st"));
}

#[test]
fn parse_program_state_machine() {
    parse_file(Path::new("../../tests/fixtures/programs/state_machine.st"));
}

#[test]
fn parse_program_pid_simple() {
    parse_file(Path::new("../../tests/fixtures/programs/pid_simple.st"));
}

#[test]
fn parse_fixture_direct_representation() {
    parse_file(Path::new(
        "../../tests/fixtures/parse/direct_representation.st",
    ));
}

/// Helper that parses a file and expects errors (for error recovery tests).
fn parse_file_expect_errors(path: &Path) {
    let source = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    let (_unit, errors) = plcc_st::parse(&source);
    assert!(
        !errors.is_empty(),
        "expected parse errors in {} but got none",
        path.display()
    );
}

#[test]
fn parse_error_recovery_missing_end() {
    parse_file_expect_errors(Path::new(
        "../../tests/fixtures/parse/error_recovery/missing_end.st",
    ));
}

#[test]
fn parse_error_recovery_bad_expression() {
    parse_file_expect_errors(Path::new(
        "../../tests/fixtures/parse/error_recovery/bad_expression.st",
    ));
}

#[test]
fn parse_fixture_typecheck_type_errors() {
    parse_file(Path::new("../../tests/fixtures/typecheck/type_errors.st"));
}

#[test]
fn parse_fixture_typecheck_implicit_conversions() {
    parse_file(Path::new(
        "../../tests/fixtures/typecheck/implicit_conversions.st",
    ));
}

#[test]
fn parse_fixture_typecheck_fb_instantiation() {
    parse_file(Path::new(
        "../../tests/fixtures/typecheck/fb_instantiation.st",
    ));
}

#[test]
fn parse_fixture_codegen_arithmetic() {
    parse_file(Path::new("../../tests/fixtures/codegen/arithmetic.st"));
}

#[test]
fn parse_fixture_codegen_control_flow() {
    parse_file(Path::new("../../tests/fixtures/codegen/control_flow.st"));
}

#[test]
fn parse_fixture_codegen_function_calls() {
    parse_file(Path::new("../../tests/fixtures/codegen/function_calls.st"));
}
