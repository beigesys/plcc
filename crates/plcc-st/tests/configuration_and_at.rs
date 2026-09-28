// SPDX-License-Identifier: MPL-2.0

//! CONFIGURATION details and AT syntax that codegen now depends on.

use plcc_st::ast::{Declaration, ExpressionKind};

fn config(src: &str) -> plcc_st::ast::ConfigurationDecl {
    let (unit, errors) = plcc_st::parse(src);
    assert!(errors.is_empty(), "{errors:?}");
    unit.declarations
        .into_iter()
        .find_map(|d| match d {
            Declaration::Configuration(c) => Some(c),
            _ => None,
        })
        .expect("configuration")
}

#[test]
fn program_connections_are_parsed() {
    let c = config(
        "CONFIGURATION C RESOURCE R ON X
            TASK T (INTERVAL := T#10ms, PRIORITY := 1);
            PROGRAM p WITH T : Main (a := g1, b => g2);
         END_RESOURCE END_CONFIGURATION",
    );
    let pc = &c.resources[0].program_configs[0];
    assert_eq!(pc.program_type.name, "Main");
    assert_eq!(pc.connections.len(), 2);
    assert!(!pc.connections[0].is_output);
    assert!(pc.connections[1].is_output);
    assert_eq!(pc.connections[1].name.as_ref().unwrap().name, "b");
}

#[test]
fn tasks_and_programs_without_a_resource() {
    let c = config(
        "CONFIGURATION Cell
            TASK Fast (INTERVAL := T#1ms);
            PROGRAM p1 WITH Fast : Main;
         END_CONFIGURATION",
    );
    assert_eq!(c.resources.len(), 1, "an implicit resource holds them");
    assert_eq!(c.resources[0].name.name, "Cell");
    assert_eq!(c.resources[0].tasks[0].name.name, "Fast");
    assert_eq!(c.resources[0].program_configs[0].name.name, "p1");
}

#[test]
fn var_config_is_diagnosed_not_skipped() {
    let (_, errors) = plcc_st::parse(
        "CONFIGURATION C
            VAR_CONFIG R.p.x AT %QX0.0 : BOOL; END_VAR
         END_CONFIGURATION",
    );
    assert!(
        errors.iter().any(|e| e.to_string().contains("not yet supported")),
        "{errors:?}"
    );
}

#[test]
fn partial_addresses_lex() {
    let (unit, errors) =
        plcc_st::parse("PROGRAM P VAR a AT %I* : BOOL; b AT %QW* : WORD; END_VAR END_PROGRAM");
    assert!(errors.is_empty(), "{errors:?}");
    let Declaration::Program(p) = &unit.declarations[0] else {
        panic!()
    };
    let reprs: Vec<_> = p.var_blocks[0]
        .declarations
        .iter()
        .map(|d| d.at_address.as_ref().unwrap().repr.clone())
        .collect();
    assert_eq!(reprs, ["%I*", "%QW*"]);
}

#[test]
fn at_without_an_address_is_an_error() {
    let (_, errors) = plcc_st::parse("PROGRAM P VAR a AT : BOOL; END_VAR END_PROGRAM");
    assert!(
        errors.iter().any(|e| e.to_string().contains("after AT")),
        "{errors:?}"
    );
}

#[test]
fn direct_variables_in_expressions() {
    let (unit, errors) = plcc_st::parse("PROGRAM P %QX0.1 := %IX0.0 AND %mx3.7; END_PROGRAM");
    assert!(errors.is_empty(), "{errors:?}");
    let Declaration::Program(p) = &unit.declarations[0] else {
        panic!()
    };
    let plcc_st::ast::StatementKind::Assignment { target, .. } = &p.body[0].kind else {
        panic!()
    };
    assert!(matches!(&target.kind, ExpressionKind::DirectVariable(r) if r == "%QX0.1"));
}
