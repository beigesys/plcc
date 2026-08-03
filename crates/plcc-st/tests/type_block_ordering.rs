// SPDX-License-Identifier: MPL-2.0

//! A `TYPE ... END_TYPE` block yields one declaration per entry, in source order,
//! interleaved correctly with the declarations around it.
//!
//! The parser returns the block's first entry from `parse_declaration` and queues the
//! rest. The queue is drained unconditionally after each attempt, not only after a
//! successful one — draining it only on success means any construct that queues
//! extras while failing to produce a first declaration would leak them past the next
//! declaration boundary and reorder the unit around a parse error.

use plcc_st::ast::Declaration;

fn names(source: &str) -> Vec<String> {
    let (unit, _errors) = plcc_st::parse(source);
    unit.declarations
        .iter()
        .map(|d| match d {
            Declaration::Program(p) => format!("PROGRAM {}", p.name.name),
            Declaration::Function(f) => format!("FUNCTION {}", f.name.name),
            Declaration::FunctionBlock(fb) => format!("FUNCTION_BLOCK {}", fb.name.name),
            Declaration::Class(c) => format!("CLASS {}", c.name.name),
            Declaration::Interface(i) => format!("INTERFACE {}", i.name.name),
            Declaration::TypeDecl(t) => format!("TYPE {}", t.name.name),
            Declaration::GlobalVarDecl(_) => "VAR_GLOBAL".to_string(),
            Declaration::Configuration(c) => format!("CONFIGURATION {}", c.name.name),
        })
        .collect()
}

#[test]
fn every_entry_of_a_type_block_appears_in_source_order() {
    let source = r#"
TYPE
    T1 : INT;
    T2 : DINT;
    T3 : STRUCT a : INT; END_STRUCT;
END_TYPE

PROGRAM After
VAR x : INT; END_VAR
    x := 1;
END_PROGRAM
"#;
    assert_eq!(
        names(source),
        vec!["TYPE T1", "TYPE T2", "TYPE T3", "PROGRAM After"]
    );
}

#[test]
fn two_type_blocks_around_a_pou_keep_their_positions() {
    let source = r#"
TYPE
    A1 : INT;
    A2 : INT;
END_TYPE

PROGRAM Middle
VAR x : INT; END_VAR
    x := 1;
END_PROGRAM

TYPE
    B1 : INT;
    B2 : INT;
END_TYPE

PROGRAM Last
VAR y : INT; END_VAR
    y := 2;
END_PROGRAM
"#;
    assert_eq!(
        names(source),
        vec![
            "TYPE A1",
            "TYPE A2",
            "PROGRAM Middle",
            "TYPE B1",
            "TYPE B2",
            "PROGRAM Last"
        ]
    );
}

#[test]
fn a_parse_error_after_a_type_block_does_not_reorder_it() {
    // `x := ;` is a broken statement. The TYPE entries must still sit before the POU
    // they were written before — not after whatever declaration parses next.
    let source = r#"
TYPE
    T1 : INT;
    T2 : DINT;
END_TYPE

PROGRAM Broken
VAR x : INT; END_VAR
    x := ;
END_PROGRAM

PROGRAM Good
VAR y : INT; END_VAR
    y := 1;
END_PROGRAM
"#;
    assert_eq!(
        names(source),
        vec!["TYPE T1", "TYPE T2", "PROGRAM Broken", "PROGRAM Good"]
    );
}

#[test]
fn a_malformed_entry_does_not_lose_the_entries_after_it() {
    // The first entry has no name. The rest of the block must survive, in order.
    let source = r#"
TYPE
    : INT;
    T2 : DINT;
    T3 : INT;
END_TYPE

PROGRAM After
VAR x : INT; END_VAR
    x := 1;
END_PROGRAM
"#;
    let got = names(source);
    assert_eq!(
        got.len(),
        4,
        "one declaration per entry, plus the POU: {got:?}"
    );
    assert_eq!(got[1], "TYPE T2");
    assert_eq!(got[2], "TYPE T3");
    assert_eq!(got[3], "PROGRAM After");
}
