// SPDX-License-Identifier: MPL-2.0

//! Two `TYPE` declarations that share a name are a diagnostic, not last-wins.
//!
//! `TypeRegistry` case-folds its keys, because ST identifiers are case-insensitive.
//! So `TYPE Foo : STRUCT a : DINT; ...` followed by `TYPE foo : STRUCT b : DINT; ...`
//! silently replaced the first, and the program then failed somewhere else entirely:
//!
//! ```text
//! $ plcc compile dup.st -o dup.ll
//! Codegen error: undefined variable: a
//! ```
//!
//! — `a` being a field only the shadowed declaration had. Nothing in that message
//! points at the two TYPE lines.

use inkwell::context::Context;
use plcc_codegen::Compiler;

fn compile(source: &str) -> Result<(), String> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let ctx = Context::create();
    let mut compiler = Compiler::new(&ctx, "duptype");
    compiler.compile(&unit).map_err(|e| e.to_string())
}

#[test]
fn two_types_differing_only_in_case_are_rejected() {
    let err = compile(
        r#"
TYPE
    Foo : STRUCT
        a : DINT;
    END_STRUCT;
    foo : STRUCT
        b : DINT;
    END_STRUCT;
END_TYPE

PROGRAM P
VAR
    n : DINT;
    s : Foo;
END_VAR
    s.a := 1;
    n := s.a;
END_PROGRAM
"#,
    )
    .expect_err("a duplicate TYPE name must not compile");
    assert!(
        err.contains("Foo") && err.contains("foo"),
        "the diagnostic must name both spellings: {err}"
    );
    assert!(
        !err.contains("undefined variable"),
        "the old symptom must not be what the user sees: {err}"
    );
}

#[test]
fn two_types_with_the_same_spelling_are_rejected() {
    let err = compile(
        r#"
TYPE
    Thing : STRUCT
        a : DINT;
    END_STRUCT;
    Thing : STRUCT
        a : DINT;
    END_STRUCT;
END_TYPE

PROGRAM P
VAR
    t : Thing;
END_VAR
    t.a := 1;
END_PROGRAM
"#,
    )
    .expect_err("a duplicate TYPE name must not compile");
    assert!(err.contains("Thing"), "must name the type: {err}");
}

#[test]
fn one_type_referenced_in_several_casings_still_compiles() {
    // The fold itself must stay: OSCAT declares `TYPE COMPLEX` and then writes
    // `complex`. Rejecting duplicates must not start rejecting that.
    compile(
        r#"
TYPE
    Complex : STRUCT
        re : REAL;
        im : REAL;
    END_STRUCT;
END_TYPE

PROGRAM P
VAR
    a : COMPLEX;
    b : complex;
    c : Complex;
END_VAR
    a.re := 1.0;
    b.re := 2.0;
    c.re := 3.0;
END_PROGRAM
"#,
    )
    .expect("one TYPE named in three casings is one type");
}
