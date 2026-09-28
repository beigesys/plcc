// SPDX-License-Identifier: MPL-2.0

//! An expression that produces no value must be blamed, not its container.
//!
//! `MIN(x, LANGUAGE.LMAX)` used to fail with `failed to compile argument for MIN`,
//! which reads as "MIN is unimplemented". MIN was fine; `LANGUAGE` was a VAR_GLOBAL in
//! another file. The same shape hid undefined nested functions (`T_PLC_MS()`) and
//! genuinely missing builtins (`TIME_TO_DWORD`) behind whichever builtin wrapped them,
//! and turned IF/FOR/WHILE failures into `failed to compile condition`.

use inkwell::context::Context;
use plcc_codegen::Compiler;

fn expect_error(source: &str) -> String {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let context = Context::create();
    let mut compiler = Compiler::new(&context, "no_value_test");
    match compiler.compile(&unit) {
        Ok(()) => panic!("expected a codegen error, but compilation succeeded"),
        Err(e) => e.to_string(),
    }
}

fn program(body: &str) -> String {
    format!(
        "PROGRAM Main\nVAR\n    x : INT;\n    y : INT;\n    d : DINT;\n    b : BOOL;\nEND_VAR\n{body}\nEND_PROGRAM\n"
    )
}

#[test]
fn builtin_argument_names_unknown_identifier() {
    let msg = expect_error(&program("    y := MIN(x, UNDEFINED_GLOBAL);"));
    assert!(
        msg.contains("UNDEFINED_GLOBAL"),
        "must name the culprit; got: {msg}"
    );
    assert!(
        msg.contains("unknown identifier"),
        "must say why; got: {msg}"
    );
    assert!(
        msg.contains("argument 2"),
        "must name the argument slot; got: {msg}"
    );
    assert!(
        !msg.contains("failed to compile argument"),
        "old misleading wording is gone; got: {msg}"
    );
}

#[test]
fn builtin_argument_names_unknown_nested_function() {
    let msg = expect_error(&program("    d := DWORD_TO_DINT(NOT_A_FUNC());"));
    assert!(
        msg.contains("NOT_A_FUNC"),
        "must name the culprit; got: {msg}"
    );
    assert!(msg.contains("unknown function"), "must say why; got: {msg}");
    assert!(
        msg.contains("DWORD_TO_DINT"),
        "must name the enclosing call; got: {msg}"
    );
}

#[test]
fn builtin_argument_names_unknown_struct_root() {
    // The OSCAT shape: a member of a global declared in another file.
    let msg = expect_error(&program("    y := MIN(x, LANGUAGE.LMAX);"));
    assert!(msg.contains("unknown identifier `LANGUAGE`"), "got: {msg}");
    assert!(
        msg.contains("LANGUAGE.LMAX"),
        "must render the argument; got: {msg}"
    );
}

#[test]
fn culprit_found_inside_arithmetic_argument() {
    let msg = expect_error(&program("    y := ABS(x + 2 * MISSING_VAR);"));
    assert!(
        msg.contains("unknown identifier `MISSING_VAR`"),
        "got: {msg}"
    );
}

#[test]
fn builtin_operand_that_compiled_is_not_blamed() {
    // `SHL(..)` compiles; `NOT_A_FUNC` is the operand that produced nothing. A builtin
    // has no module declaration, so a shape-only guess would wrongly blame SHL (OSCAT
    // `_RMP_W`: `DWORD_TO_DINT(SHL(tx-tl, 16) / TIME_TO_DWORD(TR))`).
    let msg = expect_error(&program(
        "    d := DWORD_TO_DINT(SHL(d, 2) / NOT_A_FUNC(x));",
    ));
    assert!(msg.contains("unknown function `NOT_A_FUNC`"), "got: {msg}");
    assert!(!msg.contains("unknown function `SHL`"), "got: {msg}");
}

#[test]
fn if_condition_names_culprit() {
    let msg = expect_error(&program("    IF NOPE THEN x := 1; END_IF;"));
    assert!(msg.contains("IF condition"), "got: {msg}");
    assert!(msg.contains("unknown identifier `NOPE`"), "got: {msg}");
}

#[test]
fn for_bound_names_culprit() {
    let msg = expect_error(&program("    FOR x := 1 TO LIMIT_FN() DO y := x; END_FOR;"));
    assert!(msg.contains("FOR end value"), "got: {msg}");
    assert!(msg.contains("unknown function `LIMIT_FN`"), "got: {msg}");
}

#[test]
fn while_condition_names_culprit() {
    let msg = expect_error(&program("    WHILE x < GONE DO x := x + 1; END_WHILE;"));
    assert!(msg.contains("WHILE condition"), "got: {msg}");
    assert!(msg.contains("unknown identifier `GONE`"), "got: {msg}");
}
