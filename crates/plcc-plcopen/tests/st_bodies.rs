// SPDX-License-Identifier: MPL-2.0

//! PLCopen XML with ST bodies: data types, interfaces, AT addresses, initial
//! values, a configuration with a task — lowered, compiled, executed.

mod common;
use common::*;
use plcc_st::ast::{Declaration, TypeSpecKind, VarBlockKind};

#[test]
fn lowers_types_pous_and_configuration() {
    let unit = lower("st_project.xml");
    let names: Vec<String> = unit
        .declarations
        .iter()
        .map(|d| match d {
            Declaration::TypeDecl(t) => format!("TYPE {}", t.name.name),
            Declaration::Function(f) => format!("FUNCTION {}", f.name.name),
            Declaration::FunctionBlock(f) => format!("FB {}", f.name.name),
            Declaration::Program(p) => format!("PROGRAM {}", p.name.name),
            Declaration::Configuration(c) => format!("CONFIGURATION {}", c.name.name),
            _ => "?".into(),
        })
        .collect();
    assert_eq!(
        names,
        [
            "TYPE Mode",
            "TYPE Percent",
            "TYPE Counts",
            "TYPE Speed",
            "TYPE Motor",
            "FUNCTION Clamp",
            "FB Accumulator",
            "PROGRAM Main",
            "CONFIGURATION Plant"
        ]
    );
    let Declaration::TypeDecl(t) = &unit.declarations[0] else {
        unreachable!()
    };
    assert!(matches!(&t.type_spec.kind, TypeSpecKind::Enum(e) if e.values.len() == 3));

    let main = unit
        .declarations
        .iter()
        .find_map(|d| match d {
            Declaration::Program(p) => Some(p),
            _ => None,
        })
        .unwrap();
    let kinds: Vec<_> = main
        .var_blocks
        .iter()
        .map(|b| (b.kind, b.is_constant, b.is_retain))
        .collect();
    assert_eq!(
        kinds,
        [
            (VarBlockKind::Var, false, false),
            (VarBlockKind::Var, true, false),
            (VarBlockKind::Var, false, true),
        ]
    );
    let start = &main.var_blocks[0].declarations[0];
    assert_eq!(start.at_address.as_ref().unwrap().repr, "%IX0.0");

    // Spans point into the XML: the body's first statement is `scans := scans + 1;`.
    let src = read_fixture("st_project.xml");
    let s = &main.body[0].span;
    assert_eq!(&src[s.start..s.end], "scans := scans + 1;");
}

#[test]
fn type_checks_clean() {
    let errs = check_messages("st_project.xml");
    assert!(errs.is_empty(), "{errs:#?}");
}

#[test]
fn st_project_runs() {
    with_plc("st_project.xml", |plc| {
        assert_eq!(plc.tasks().len(), 1);
        assert_eq!(plc.task_name(0), "Fast");
        assert_eq!(plc.tasks()[0].interval_ns, 10 * MS);
        assert_eq!(plc.tasks()[0].priority, 1);

        // %IX0.0 = start, %IW1 = raw (bytes 2..3)
        *plc.input_byte(0) = 1;
        let raw = 300i16.to_ne_bytes();
        *plc.input_byte(2) = raw[0];
        *plc.input_byte(3) = raw[1];
        plc.scan();
        assert!(plc.get_bool("Main.running"), "%QX0.1 follows start");
        assert_eq!(plc.output_byte(0) & 0b10, 0b10);
        assert_eq!(plc.get("Main.level"), 600);
        assert_eq!(plc.get("main1.histsum"), 10 + 10 + 20 + 30 + 42);
        assert_eq!(plc.get("main1.m.starts"), 8, "struct field default 7, +1");
        assert_eq!(plc.get("main1.acc.total"), 1);

        let raw = 900i16.to_ne_bytes();
        *plc.input_byte(2) = raw[0];
        *plc.input_byte(3) = raw[1];
        *plc.input_byte(0) = 0;
        plc.scan();
        assert!(!plc.get_bool("Main.running"));
        assert_eq!(plc.get("Main.level"), 1000, "Clamp() limits to LIMIT_HI");
        assert_eq!(plc.get("main1.scans"), 2);
        assert_eq!(plc.get("main1.acc.total"), 2);
    });
}

#[test]
fn sniffing() {
    assert!(plcc_plcopen::is_plcopen(&read_fixture("st_project.xml")));
    assert!(plcc_plcopen::is_plcopen("<project/>"));
    assert!(plcc_plcopen::is_plcopen(
        "\u{feff}<?xml version=\"1.0\"?>\n<!-- c --><ns:project>"
    ));
    assert!(!plcc_plcopen::is_plcopen("PROGRAM p END_PROGRAM"));
    assert!(!plcc_plcopen::is_plcopen("<?xml version=\"1.0\"?><svg/>"));
}
