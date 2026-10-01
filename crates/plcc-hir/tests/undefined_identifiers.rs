// SPDX-License-Identifier: MPL-2.0

//! An identifier that names nothing is a type-check error with a location, as in
//! CODESYS ("Identifier '<name>' not defined"). It used to pass `plcc check`, and
//! `plcc compile` then failed in codegen with no source location.

use plcc_hir::{CheckError, check};
use plcc_st::parse;

fn undefined(src: &str) -> Vec<String> {
    let (unit, parse_errors) = parse(src);
    assert!(parse_errors.is_empty(), "parse errors: {parse_errors:?}");
    check(&unit)
        .1
        .into_iter()
        .filter_map(|e| match e {
            CheckError::UndefinedVariable { name, .. } => Some(name),
            _ => None,
        })
        .collect()
}

#[test]
fn undeclared_target_and_operand_are_reported() {
    let src = "PROGRAM p VAR a : INT; END_VAR
undeclared := 5;
a := also_undeclared + 1;
END_PROGRAM";
    assert_eq!(undefined(src), ["undeclared", "also_undeclared"]);
}

#[test]
fn everything_that_is_defined_is_accepted() {
    let src = "
TYPE Mode : (Idle, Run); END_TYPE
TYPE Pt : STRUCT x : INT; END_STRUCT END_TYPE
VAR_GLOBAL g : INT; gp : Pt; END_VAR
CONFIGURATION c
  VAR_GLOBAL cg : INT; END_VAR
  RESOURCE r ON PLC
    VAR_GLOBAL rg : INT; END_VAR
    TASK t (INTERVAL := T#10ms);
    PROGRAM i WITH t : p;
  END_RESOURCE
END_CONFIGURATION
FUNCTION f : INT VAR_INPUT v : INT; END_VAR f := v; END_FUNCTION
FUNCTION_BLOCK Base VAR_INPUT bin : INT; END_VAR VAR bl : INT; END_VAR END_FUNCTION_BLOCK
FUNCTION_BLOCK Derived EXTENDS Base
  VAR_OUTPUT o : INT; END_VAR
  o := bin + bl;
END_FUNCTION_BLOCK
PROGRAM p
VAR a : INT; m : Mode; inl : (Red, Green); d : Derived; s : UDINT; END_VAR
a := g + cg + rg + gp.x + f(1) + d.o;
m := Idle;
m := Mode#Run;
inl := Green;
s := SIZEOF(Pt);
PRINT('x');
END_PROGRAM";
    assert_eq!(undefined(src), Vec::<String>::new());
}

#[test]
fn method_bodies_are_checked() {
    // Methods see the block's variables, inherited ones, their own and their
    // return value; anything else is undefined (Logix ST routines are methods).
    let src = "
FUNCTION_BLOCK Base VAR bl : INT; END_VAR END_FUNCTION_BLOCK
FUNCTION_BLOCK FB EXTENDS Base
VAR x : INT; END_VAR
METHOD M : INT
VAR_INPUT k : INT; END_VAR
VAR t : INT; END_VAR
    t := k + x + bl;
    M := t;
    x := Bogus + 1;
END_METHOD
M(k := 1);
END_FUNCTION_BLOCK";
    assert_eq!(undefined(src), ["Bogus"]);
}
