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
