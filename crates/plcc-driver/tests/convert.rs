// SPDX-License-Identifier: MPL-2.0

//! `plcc convert` over in-memory files.

use plcc_driver::convert::{Format, convert};
use plcc_driver::{Dialect, Project, Severity, Stage};
use std::path::Path;

fn fixture(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    std::fs::read_to_string(root.join(rel)).unwrap()
}

fn project(files: &[(&str, &str)]) -> Project {
    Project {
        files: files
            .iter()
            .map(|(p, t)| (p.to_string(), t.to_string()))
            .collect(),
        ..Default::default()
    }
}

const SEAL_IN: &str = "PROGRAM P VAR a : BOOL; b : BOOL; q : BOOL; END_VAR q := (a OR q) AND NOT b; END_PROGRAM";

#[test]
fn plcopen_ladder_to_l5x() {
    let p = project(&[("ld.xml", &fixture("plcopen/ld_timer_counter.xml"))]);
    let c = convert(&p, Format::L5x, None, false);
    let out = c.output.unwrap_or_else(|| panic!("{:#?}", c.diagnostics));
    assert!(out.contains("RSLogix5000Content"));
    for d in &c.diagnostics {
        assert_ne!(d.severity, Severity::Error, "{d:#?}");
        assert_eq!(d.stage, Stage::Convert);
    }
}

#[test]
fn l5x_to_st_and_through_json_to_plcopen() {
    let p = project(&[("s.L5X", &fixture("l5x/seal_in.L5X"))]);
    let st = convert(&p, Format::St, None, true);
    let st = st.output.unwrap_or_else(|| panic!("{:#?}", st.diagnostics));
    assert!(st.contains("Logix prelude"));
    let json = convert(&p, Format::LadderJson, Some(Dialect::Iec), false);
    let json = json.output.unwrap_or_else(|| panic!("{:#?}", json.diagnostics));
    let back = convert(&project(&[("m.json", &json)]), Format::Plcopen, None, false);
    assert!(back.output.expect("xml").contains("<project"));
}

#[test]
fn st_to_ladder_to_st() {
    let c = convert(&project(&[("p.st", SEAL_IN)]), Format::LadderJson, None, false);
    let json = c.output.unwrap_or_else(|| panic!("{:#?}", c.diagnostics));
    let back = convert(&project(&[("p.json", &json)]), Format::St, None, false);
    assert!(back.output.expect("st").contains("PROGRAM P"));
}

#[test]
fn errors_are_diagnostics() {
    let c = convert(&project(&[("a.st", SEAL_IN), ("b.st", SEAL_IN)]), Format::L5x, None, false);
    assert!(c.output.is_none());
    assert!(c.diagnostics[0].message.contains("one input"));
    let c = convert(&project(&[("a.st", "PROGRAM P x := ; END_PROGRAM")]), Format::LadderJson, None, false);
    assert!(c.output.is_none());
    assert_eq!(c.diagnostics[0].stage, Stage::Parse);
    let c = convert(&project(&[("../a.st", SEAL_IN)]), Format::St, None, false);
    assert_eq!(c.diagnostics[0].stage, Stage::Input);
}

/// The studio's seeded demo project (`studio/src/model/demo.ts`), as the
/// Logix ladder model the studio's ST view converts.
const DEMO: &str = r#"{
  "dialect": "logix", "name": "Demo Opta",
  "globals": [
    {"name": "StartPB", "data_type": "BOOL", "section": "global"},
    {"name": "StopPB", "data_type": "BOOL", "section": "global"},
    {"name": "Motor", "data_type": "BOOL", "section": "global"},
    {"name": "Level", "data_type": "INT", "section": "global"},
    {"name": "High", "data_type": "BOOL", "section": "global"},
    {"name": "RunTimer", "data_type": "TIMER", "section": "global"}
  ],
  "pous": [ {"id": 1, "name": "MainProgram", "kind": "program", "routines": [
    {"id": 2, "name": "MainRoutine", "rungs": [
      {"id": 3, "comment": "Motor start/stop with seal-in", "elements": [
        {"type": "branch", "id": 4, "legs": [
          [{"type": "contact", "id": 5, "operand": "StartPB", "kind": "no"}],
          [{"type": "contact", "id": 6, "operand": "Motor", "kind": "no"}]]},
        {"type": "contact", "id": 7, "operand": "StopPB", "kind": "nc"},
        {"type": "coil", "id": 8, "operand": "Motor", "kind": "normal"}]},
      {"id": 9, "comment": "Run timer: done 5 s after the motor starts", "elements": [
        {"type": "contact", "id": 10, "operand": "Motor", "kind": "no"},
        {"type": "block", "id": 11, "name": "TON", "pins": [
          {"name": "Timer", "dir": "in_out", "value": "RunTimer"},
          {"name": "Preset", "dir": "input", "value": "5000"},
          {"name": "Accum", "dir": "input", "value": "0"}]}]},
      {"id": 12, "comment": "High level alarm on the USER LED", "elements": [
        {"type": "block", "id": 13, "name": "GRT", "pins": [
          {"name": "Source A", "dir": "input", "value": "Level"},
          {"name": "Source B", "dir": "input", "value": "2000"}]},
        {"type": "coil", "id": 14, "operand": "High", "kind": "normal"}]}
    ]}
  ]} ],
  "tasks": [{"name": "MainTask", "interval_ms": 10.0, "priority": 10, "programs": ["MainProgram"]}]
}"#;

/// The studio's ST view: the model converted to ST in one flavor, and the
/// conversion's notes.
fn st_view(model: &str, dialect: Dialect) -> (String, Vec<String>) {
    let mut p = project(&[("project.json", model)]);
    p.entry = Some(vec!["project.json".into()]);
    let c = convert(&p, Format::St, Some(dialect), false);
    let notes = c.diagnostics.iter().map(|d| d.message.clone()).collect();
    (c.output.unwrap_or_else(|| panic!("{:#?}", c.diagnostics)), notes)
}

/// Rungs are numbered from 0 in both flavors of the ST view and in the
/// translation's warnings, as in the studio and Studio 5000.
#[test]
fn st_view_numbers_rungs_from_zero() {
    for d in [Dialect::Logix, Dialect::Iec] {
        let (st, notes) = st_view(DEMO, d);
        for (n, doc) in [
            (0, "Motor start/stop with seal-in"),
            (1, "Run timer: done 5 s after the motor starts"),
            (2, "High level alarm on the USER LED"),
        ] {
            let c = format!("rung {n}: {doc}");
            assert_eq!(st.matches(&c).count(), 1, "{d:?}: `{c}`\n{st}");
        }
        assert!(!st.contains("rung 3"), "{d:?}\n{st}");
        if d == Dialect::Iec {
            assert!(
                notes
                    .iter()
                    .any(|m| m.contains("MainRoutine rung 1: TON(RunTimer,5000,0)")),
                "{notes:#?}"
            );
        }
    }
}

/// A rung the translation adds (a one-shot's storage-bit update) is numbered
/// as the rung it belongs to: the rungs after it keep their numbers.
#[test]
fn translation_helper_rungs_keep_the_numbering() {
    let l5x = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<RSLogix5000Content SchemaRevision="1.0" SoftwareRevision="33.01" TargetName="Ons" TargetType="Controller">
<Controller Use="Target" Name="Ons" ProcessorType="1769-L33ER" MajorRev="33" MinorRev="11">
<DataTypes/><Modules/><AddOnInstructionDefinitions/>
<Tags>
<Tag Name="A" TagType="Base" DataType="BOOL"/><Tag Name="B" TagType="Base" DataType="BOOL"/>
<Tag Name="Sb" TagType="Base" DataType="BOOL"/><Tag Name="P" TagType="Base" DataType="BOOL"/>
<Tag Name="Q" TagType="Base" DataType="BOOL"/>
</Tags>
<Programs><Program Name="Main" MainRoutineName="R"><Tags/><Routines><Routine Name="R" Type="RLL"><RLLContent>
<Rung Number="0" Type="N"><Text><![CDATA[XIC(A)ONS(Sb)OTE(P);]]></Text></Rung>
<Rung Number="1" Type="N"><Text><![CDATA[XIC(B)OTE(Q);]]></Text></Rung>
</RLLContent></Routine></Routines></Program></Programs>
<Tasks><Task Name="T" Type="CONTINUOUS" Priority="10"><ScheduledPrograms><ScheduledProgram Name="Main"/></ScheduledPrograms></Task></Tasks>
</Controller>
</RSLogix5000Content>
"#;
    let json = convert(&project(&[("o.L5X", l5x)]), Format::LadderJson, None, false);
    let json = json.output.unwrap_or_else(|| panic!("{:#?}", json.diagnostics));
    let (st, _) = st_view(&json, Dialect::Iec);
    let at = |s: &str| st.find(s).unwrap_or_else(|| panic!("no `{s}`\n{st}"));
    assert!(at("(* rung 0") < at("(* part of rung 0"), "{st}");
    assert!(at("(* part of rung 0") < at("(* rung 1"), "{st}");
    assert!(!st.contains("rung 2"), "{st}");
}
