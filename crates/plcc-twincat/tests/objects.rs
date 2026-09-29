// SPDX-License-Identifier: MPL-2.0

//! TwinCAT object files lower to the plcc-st AST, with spans in the XML file.

use plcc_st::{Declaration, TypeSpecKind};
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/twincat")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(fixtures().join(rel)).unwrap()
}

/// 1-based (line, column) of a byte offset.
fn line_col(src: &str, offset: usize) -> (usize, usize) {
    let before = &src[..offset];
    let line = before.matches('\n').count() + 1;
    let col = before.len() - before.rfind('\n').map_or(0, |p| p + 1) + 1;
    (line, col)
}

#[test]
fn function_block_with_method_property_and_action() {
    let src = read("Demo/POUs/FB_Counter.TcPOU");
    let (unit, errors) = plcc_twincat::parse(&src);
    assert!(errors.is_empty(), "{errors:?}");
    let [Declaration::FunctionBlock(fb)] = unit.declarations.as_slice() else {
        panic!("expected one FB, got {:?}", unit.declarations);
    };
    assert_eq!(fb.name.name, "FB_Counter");
    assert_eq!(fb.implements[0].name, "I_Counter");
    assert_eq!(fb.methods.len(), 1);
    assert_eq!(fb.methods[0].name.name, "Increment");
    assert_eq!(fb.properties.len(), 2);
    let step = &fb.properties[0];
    assert_eq!(step.name.name, "Step");
    assert!(step.get.is_some() && step.set.is_some());
    assert!(fb.properties[1].set.is_none());
    assert_eq!(fb.actions.len(), 1);
    assert_eq!(fb.actions[0].name.name, "Reset");
    assert_eq!(fb.actions[0].body.len(), 2);
    // Spans point into the .TcPOU: the method name is where the file has it.
    let m = &fb.methods[0].name.span;
    assert_eq!(&src[m.start..m.end], "Increment");
    assert_eq!(line_col(&src, m.start), (24, 43));
    let a = &fb.actions[0].body[0].span;
    assert!(src[a.start..a.end].starts_with("nCount := 0;"));
}

#[test]
fn interface_with_property_prototypes() {
    let (unit, errors) = plcc_twincat::parse(&read("Demo/POUs/I_Counter.TcIO"));
    assert!(errors.is_empty(), "{errors:?}");
    let [Declaration::Interface(itf)] = unit.declarations.as_slice() else {
        panic!("expected an interface");
    };
    assert_eq!(itf.methods.len(), 1);
    assert_eq!(itf.properties.len(), 1);
    assert!(itf.properties[0].get.is_some() && itf.properties[0].set.is_some());
}

#[test]
fn dut_and_gvl() {
    let (unit, errors) = plcc_twincat::parse(&read("Demo/DUTs/E_Mode.TcDUT"));
    assert!(errors.is_empty(), "{errors:?}");
    let [Declaration::TypeDecl(t)] = unit.declarations.as_slice() else {
        panic!("expected a type");
    };
    let TypeSpecKind::Enum(e) = &t.type_spec.kind else {
        panic!("expected an enum");
    };
    assert_eq!(e.base_type.as_ref().unwrap().name, "INT");
    assert_eq!(e.values.len(), 3);

    let src = read("Demo/GVLs/GVL_Main.TcGVL");
    let (unit, errors) = plcc_twincat::parse(&src);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(unit.declarations.len(), 2);
    for d in &unit.declarations {
        let Declaration::GlobalVarDecl(b) = d else {
            panic!("expected VAR_GLOBAL");
        };
        let name = b.list_name.as_ref().unwrap();
        assert_eq!(name.name, "GVL_Main");
        assert_eq!(&src[name.span.start..name.span.end], "GVL_Main");
    }
}

#[test]
fn parse_errors_point_into_the_cdata() {
    let src = read("errors/FB_Syntax.TcPOU");
    let (_, errors) = plcc_twincat::parse(&src);
    assert_eq!(errors.len(), 1, "{errors:?}");
    let at = errors[0].span.offset();
    assert_eq!(line_col(&src, at), (16, 35));
}

#[test]
fn graphical_bodies_and_transitions_are_reported() {
    let src = read("errors/FB_Ladder.TcPOU");
    let (_, errors) = plcc_twincat::parse(&src);
    let (warnings, errs): (Vec<_>, Vec<_>) = errors.iter().partition(|e| e.is_warning());
    assert_eq!(errs.len(), 1, "{errors:?}");
    assert!(errs[0].message.contains("FB_Ladder"));
    assert!(errs[0].message.contains("not yet supported"));
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].message.contains("T_Done"));
}

#[test]
fn project_lists_its_object_files_in_order() {
    let path = fixtures().join("Demo/Demo.plcproj");
    let src = std::fs::read_to_string(&path).unwrap();
    let files = plcc_twincat::project_files(&path, &src).unwrap();
    let names: Vec<String> = files
        .iter()
        .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    // The visualization is not code and is skipped.
    assert_eq!(
        names,
        [
            "E_Mode.TcDUT",
            "ST_Sample.TcDUT",
            "GVL_Main.TcGVL",
            "I_Counter.TcIO",
            "FB_Counter.TcPOU",
            "PRG_Stats.TcPOU",
            "MAIN.TcPOU",
            "PlcTask.TcTTO"
        ]
    );
    assert_eq!(plcc_twincat::project_libraries(&src), ["Tc2_Standard"]);
    match plcc_twincat::directory_input(&fixtures().join("Demo")).unwrap() {
        plcc_twincat::DirectoryInput::Project(p) => assert!(p.ends_with("Demo.plcproj")),
        plcc_twincat::DirectoryInput::Files(_) => panic!("expected the project"),
    }
}
