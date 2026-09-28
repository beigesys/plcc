// SPDX-License-Identifier: MPL-2.0

//! The type checker accepts what CODESYS accepts.
//!
//! `plcc compile` runs the checker and refuses to build on an error, so a false
//! positive here is a program that will not compile. Each snippet below is valid
//! CODESYS (most are lifted from OSCAT or the repo's own fixtures) and used to be
//! rejected. Narrowing conversions are reported, as CODESYS does, but as warnings.

use plcc_hir::{CheckError, check};
use plcc_st::parse;

fn diagnostics(src: &str) -> Vec<CheckError> {
    let (unit, parse_errors) = parse(src);
    assert!(parse_errors.is_empty(), "parse errors: {parse_errors:?}");
    check(&unit).1
}

fn errors(src: &str) -> Vec<CheckError> {
    diagnostics(src).into_iter().filter(|e| !e.is_warning()).collect()
}

fn program(decls: &str, body: &str) -> String {
    format!("PROGRAM p\nVAR\n{decls}\nEND_VAR\n{body}\nEND_PROGRAM\n")
}

#[track_caller]
fn assert_clean(decls: &str, body: &str) {
    let d = diagnostics(&program(decls, body));
    assert!(d.is_empty(), "`{}` should be accepted silently, got {d:?}", body.trim());
}

#[track_caller]
fn assert_warns(decls: &str, body: &str) {
    let d = diagnostics(&program(decls, body));
    assert!(
        !d.is_empty() && d.iter().all(CheckError::is_warning),
        "`{}` should compile with a warning, got {d:?}",
        body.trim()
    );
}

#[track_caller]
fn assert_error(decls: &str, body: &str) {
    let e = errors(&program(decls, body));
    assert!(!e.is_empty(), "`{}` should be rejected", body.trim());
}

#[test]
fn real_literal_takes_the_type_of_its_context() {
    assert_clean("x : REAL;", "x := 2.0 * x;");
    assert_clean("x : REAL;", "x := x / 3.0 + 0.5;");
    assert_clean("x : REAL;", "x := 1.5;");
    assert_clean("x : REAL;", "x := -1.5;");
    assert_clean("x : REAL;", "x := 2.0 ** 3.0;");
    assert_clean("x : LREAL;", "x := 2.0 * x;");
    assert_clean("x : REAL; b : BOOL;", "b := x > 0.5;");
}

#[test]
fn integer_literal_takes_the_type_of_its_context() {
    assert_clean("b : BYTE;", "b := 5;");
    assert_clean("b : BYTE;", "b := 255;");
    assert_clean("w : WORD;", "w := 16#FFFF;");
    assert_clean("u : UINT;", "u := 1000;");
    assert_clean("b : BYTE;", "b := b AND 16#0F;");
    assert_clean("b : BYTE;", "b := b + 1;");
    assert_clean("d : DWORD;", "d := SHL(d, 1) OR 1;");
    assert_clean("r : REAL;", "r := 3;");
    // OSCAT TOGGLE: `q := 0;` on a BOOL output.
    assert_clean("q : BOOL;", "q := 0;");
}

#[test]
fn out_of_range_literal_is_still_diagnosed() {
    assert_warns("b : BYTE;", "b := 300;");
    assert_warns("s : SINT;", "s := 200;");
}

#[test]
fn bit_strings_do_arithmetic_as_in_codesys() {
    // OSCAT DEC_TO_INT, BIN_TO_BYTE, COUNT_BR, FILTER_MAV_W.
    assert_clean("i : INT; b : BYTE;", "i := i * 10 + b - 48;");
    assert_clean("d : DWORD;", "d := d * d;");
    assert_clean("r : REAL; b : BYTE;", "r := r + b;");
    assert_clean("b : BYTE; s : BYTE;", "b := -s;");
    assert_clean("w : WORD; d : DWORD;", "d := w;");
    assert_clean("b : BYTE; i : INT;", "i := b;");
}

#[test]
fn pointer_arithmetic() {
    assert_clean("p : POINTER TO BYTE; q : POINTER TO BYTE;", "p := p + 1;");
    assert_clean("p : POINTER TO BYTE; q : POINTER TO BYTE;", "q := 2 + p;");
    assert_clean("p : POINTER TO BYTE; q : POINTER TO BYTE; n : DINT;", "n := q - p;");
}

#[test]
fn for_over_a_byte() {
    assert_clean("b : BYTE; n : INT;", "FOR b := 0 TO 7 DO n := n + 1; END_FOR;");
}

#[test]
fn time_arithmetic() {
    assert_clean("t : TIME; u : TIME;", "t := t + u;");
    assert_clean("t : TIME; u : TIME;", "t := u * 2;");
    assert_clean("t : TIME; u : TIME;", "t := u / 2;");
    assert_clean("t : TIME; d : DT; e : DT;", "t := d - e;");
    assert_clean("d : DT; t : TIME;", "d := d + t;");
}

#[test]
fn lossy_numeric_conversions_warn() {
    // CODESYS C0197 "Implicit conversion from 'DINT' to 'REAL': possible loss of
    // information"; the build still succeeds.
    assert_warns("r : REAL; d : DINT;", "r := d;");
    assert_warns("i : INT; r : REAL;", "i := r;");
    assert_warns("i : INT; d : DINT;", "i := d;");
    assert_warns("r : REAL; l : LREAL;", "r := l;");
    assert_warns("u : UINT; i : INT;", "u := i;");
    assert_warns("t : TIME;", "t := 0;");
    // Lossless ones stay silent.
    assert_clean("r : REAL; i : INT;", "r := i;");
    assert_clean("l : LREAL; d : DINT;", "l := d;");
    assert_clean("d : DINT; u : UINT;", "d := u;");
}

#[test]
fn real_type_errors_are_still_errors() {
    assert_error("i : INT; b : BOOL;", "i := b + i;");
    assert_error("r : REAL;", "r := 'hello';");
    assert_error("i : INT; t : TIME;", "i := t;");
    assert_error("i : INT;", "IF i THEN i := 0; END_IF;");
    assert_error("r : REAL; w : WORD;", "w := NOT r;");
    assert_error("b : BOOL; i : INT;", "b := b AND i;");
}
