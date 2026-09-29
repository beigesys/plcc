// SPDX-License-Identifier: MPL-2.0

//! CODESYS / TwinCAT syntax: PROPERTY, ACTION, modifiers after the keyword,
//! namespace-qualified names, pragmas, AND_THEN / OR_ELSE, FB_init arguments.

use plcc_st::{BinaryOp, Declaration, ExpressionKind, StatementKind, TypeSpecKind, VarBlockKind};

fn parse_ok(src: &str) -> plcc_st::CompilationUnit {
    let (unit, errors) = plcc_st::parse(src);
    assert!(errors.is_empty(), "{errors:?}");
    unit
}

#[test]
fn property_and_action_in_a_function_block() {
    let unit = parse_ok(
        "{attribute 'reflection'}
        FUNCTION_BLOCK PUBLIC FB_X EXTENDS Lib.FB_Base IMPLEMENTS I_A, Lib.I_B
        VAR n : INT; END_VAR
            n := n + 1;
        METHOD PRIVATE FINAL M : BOOL;
        VAR_INPUT k : INT; END_VAR
            M := k > 0;
        END_METHOD
        PROPERTY PUBLIC P : INT
        GET
            VAR tmp : INT; END_VAR
            tmp := n; P := tmp;
        END_GET
        SET PRIVATE
            n := P;
        END_SET
        END_PROPERTY
        ACTION Reset:
            n := 0;
        END_ACTION
        END_FUNCTION_BLOCK",
    );
    let Declaration::FunctionBlock(fb) = &unit.declarations[0] else {
        panic!()
    };
    assert_eq!(fb.name.name, "FB_X");
    assert_eq!(fb.extends.as_ref().unwrap().name, "Lib.FB_Base");
    assert_eq!(fb.implements[1].name, "Lib.I_B");
    assert_eq!(fb.body.len(), 1);
    assert_eq!(fb.methods[0].name.name, "M");
    assert!(fb.methods[0].is_final);
    let p = &fb.properties[0];
    assert_eq!(p.name.name, "P");
    assert_eq!(p.get.as_ref().unwrap().var_blocks.len(), 1);
    assert_eq!(p.get.as_ref().unwrap().body.len(), 2);
    assert_eq!(p.set.as_ref().unwrap().body.len(), 1);
    assert_eq!(fb.actions[0].name.name, "Reset");
}

#[test]
fn a_variable_named_action_is_still_a_variable() {
    let unit = parse_ok(
        "FUNCTION_BLOCK F VAR action : INT; property : BOOL; END_VAR
            action := action + 1;
            property := TRUE;
        END_FUNCTION_BLOCK",
    );
    let Declaration::FunctionBlock(fb) = &unit.declarations[0] else {
        panic!()
    };
    assert_eq!(fb.body.len(), 2);
    assert!(fb.actions.is_empty());
}

#[test]
fn short_circuit_operators() {
    let unit = parse_ok(
        "PROGRAM P VAR a, b, c : BOOL; END_VAR
            c := a AND_THEN b OR_ELSE c;
        END_PROGRAM",
    );
    let Declaration::Program(p) = &unit.declarations[0] else {
        panic!()
    };
    let StatementKind::Assignment { value, .. } = &p.body[0].kind else {
        panic!()
    };
    let ExpressionKind::BinaryOp { op, left, .. } = &value.kind else {
        panic!()
    };
    assert_eq!(*op, BinaryOp::OrElse);
    assert!(matches!(
        left.kind,
        ExpressionKind::BinaryOp {
            op: BinaryOp::AndThen,
            ..
        }
    ));
}

#[test]
fn declarations_twincat_writes() {
    let unit = parse_ok(
        "TYPE INTERNAL E : (A := 1, B) DINT; END_TYPE
        TYPE ST_D EXTENDS ST_Base : STRUCT x, y : INT; io AT %I* : BOOL; END_STRUCT END_TYPE
        FUNCTION_BLOCK F
        VAR
            t : Tc2_Standard.TON;
            fb : FB_Init(1, THIS^);
            s : ST_D := ();
        END_VAR
        VAR PERSISTENT p : INT; END_VAR
        VAR_STAT shared : INT; END_VAR
        END_FUNCTION_BLOCK",
    );
    let Declaration::TypeDecl(e) = &unit.declarations[0] else {
        panic!()
    };
    let TypeSpecKind::Enum(spec) = &e.type_spec.kind else {
        panic!()
    };
    assert_eq!(spec.base_type.as_ref().unwrap().name, "DINT");
    let Declaration::TypeDecl(d) = &unit.declarations[1] else {
        panic!()
    };
    assert_eq!(d.extends.as_ref().unwrap().name, "ST_Base");
    let TypeSpecKind::Struct(fields) = &d.type_spec.kind else {
        panic!()
    };
    let names: Vec<&str> = fields.iter().map(|f| f.name.name.as_str()).collect();
    assert_eq!(names, ["x", "y", "io"]);
    let Declaration::FunctionBlock(fb) = &unit.declarations[2] else {
        panic!()
    };
    let vars = &fb.var_blocks[0].declarations;
    let TypeSpecKind::Named(t) = &vars[0].type_spec.kind else {
        panic!()
    };
    assert_eq!(t.name, "Tc2_Standard.TON");
    assert_eq!(vars[1].init_args.len(), 2);
    assert!(matches!(
        vars[2].initializer.as_ref().unwrap().kind,
        ExpressionKind::StructInitializer(ref f) if f.is_empty()
    ));
    assert!(fb.var_blocks[1].is_retain);
    assert_eq!(fb.var_blocks[2].kind, VarBlockKind::VarStat);
}

#[test]
fn call_forms_twincat_accepts() {
    let unit = parse_ok(
        "PROGRAM P VAR t : TON; a, b : INT; q : BOOL; END_VAR
            t(IN := TRUE, PT := , Q => , ET => );
            t(IN := TRUE, Q => q, );
            a := b := 3;
        END_PROGRAM",
    );
    let Declaration::Program(p) = &unit.declarations[0] else {
        panic!()
    };
    let StatementKind::FunctionCall { args, .. } = &p.body[0].kind else {
        panic!()
    };
    assert_eq!(args.len(), 1);
    let StatementKind::FunctionCall { args, .. } = &p.body[1].kind else {
        panic!()
    };
    assert_eq!(args.len(), 2);
    // `a := b := 3` is `b := 3; a := b;`.
    let StatementKind::If { then_body, .. } = &p.body[2].kind else {
        panic!()
    };
    assert_eq!(then_body.len(), 2);
}
