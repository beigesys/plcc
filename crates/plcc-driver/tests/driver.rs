// SPDX-License-Identifier: MPL-2.0

//! The in-memory pipeline against every input format.

use plcc_driver::{Project, Severity, Stage, check, tags};
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

const SEAL_IN: &str = "PROGRAM SealIn
VAR
    start AT %IX0.0 : BOOL;
    stop  AT %IX0.1 : BOOL;
    motor AT %QX0.0 : BOOL;
    speed AT %QW1 : INT;
    t : TON;
END_VAR
motor := (start OR motor) AND NOT stop;
t(IN := motor, PT := T#1s);
END_PROGRAM
";

#[test]
fn valid_st_checks_clean() {
    let c = check(&project(&[("main.st", SEAL_IN)]));
    assert!(c.ok, "{:#?}", c.diagnostics);
    assert!(c.diagnostics.is_empty());
}

#[test]
fn type_error_is_located() {
    let src = "PROGRAM P\nVAR x : BOOL; y : INT; z : INT; END_VAR\n  z := x + y;\nEND_PROGRAM\n";
    let c = check(&project(&[("src/p.st", src)]));
    assert!(!c.ok);
    let d = c
        .diagnostics
        .iter()
        .find(|d| d.severity == Severity::Error)
        .expect("an error");
    assert_eq!(d.file.as_deref(), Some("src/p.st"));
    assert_eq!(d.stage, Stage::Typecheck);
    let span = d.span.as_ref().expect("a span");
    assert_eq!(span.start.line, 3, "{d:#?}");
    assert!(span.start.col >= 3);
    let json = serde_json::to_value(d).unwrap();
    for key in ["file", "severity", "stage", "code", "message", "span", "labels"] {
        assert!(json.get(key).is_some(), "missing {key}: {json}");
    }
    assert_eq!(json["severity"], "error");
}

#[test]
fn parse_errors_stop_before_the_type_check() {
    let c = check(&project(&[("bad.st", "PROGRAM P\nVAR x : INT END_VAR\nx := ;\nEND_PROGRAM\n")]));
    assert!(!c.ok);
    assert!(c.parsed.is_none());
    assert!(c.diagnostics.iter().all(|d| d.stage == Stage::Parse));
    assert!(c.diagnostics[0].span.is_some());
}

#[test]
fn bad_paths_are_rejected() {
    for bad in ["../x.st", "/etc/passwd", "a/../../b.st", "a//b.st", "C:\\x.st", "./a.st"] {
        let c = check(&project(&[(bad, SEAL_IN)]));
        assert!(!c.ok, "{bad}");
        assert_eq!(c.diagnostics[0].stage, Stage::Input, "{bad}");
    }
}

#[test]
fn missing_entry_is_reported() {
    let mut p = project(&[("main.st", SEAL_IN)]);
    p.entry = Some(vec!["other.st".into()]);
    let c = check(&p);
    assert!(!c.ok);
    assert!(c.diagnostics[0].message.contains("other.st"));
}

#[test]
fn plcopen_xml() {
    let c = check(&project(&[("ld_seal_in.xml", &fixture("plcopen/ld_seal_in.xml"))]));
    assert!(c.ok, "{:#?}", c.diagnostics);
}

#[test]
fn l5x_with_io_map() {
    let mut p = project(&[("opta_io.L5X", &fixture("l5x/opta_io.L5X"))]);
    p.io_map = Some(fixture("l5x/opta_io.toml"));
    let c = check(&p);
    assert!(c.ok, "{:#?}", c.diagnostics);
    let t = tags::tags(c.parsed.as_ref().unwrap());
    assert!(
        t.image.iter().any(|i| i.address == "%IX0.0"),
        "{:#?}",
        t.image
    );
}

#[test]
fn bad_io_map_is_a_diagnostic() {
    let mut p = project(&[("opta_io.L5X", &fixture("l5x/opta_io.L5X"))]);
    p.io_map = Some("this is = = not toml".into());
    let c = check(&p);
    assert!(!c.ok);
    assert_eq!(c.diagnostics[0].stage, Stage::IoMap);
}

#[test]
fn twincat_project_in_memory() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/twincat/Demo");
    let mut files = Vec::new();
    for rel in [
        "Demo.plcproj",
        "PlcTask.TcTTO",
        "DUTs/E_Mode.TcDUT",
        "DUTs/ST_Sample.TcDUT",
        "POUs/MAIN.TcPOU",
        "POUs/PRG_Stats.TcPOU",
        "POUs/I_Counter.TcIO",
        "POUs/FB_Counter.TcPOU",
        "GVLs/GVL_Main.TcGVL",
        "POUs/Drafts/FB_Draft.TcPOU",
    ] {
        files.push((
            format!("Demo/{rel}"),
            std::fs::read_to_string(root.join(rel)).unwrap(),
        ));
    }
    let refs: Vec<(&str, &str)> = files.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let c = check(&project(&refs));
    assert!(c.ok, "{:#?}", c.diagnostics);
    let t = tags::tags(c.parsed.as_ref().unwrap());
    assert!(!t.tasks.is_empty());
    assert!(!t.tasks[0].implicit, "{:#?}", t.tasks);

    // Drop an included file: the project names it.
    let without: Vec<(&str, &str)> = refs
        .iter()
        .copied()
        .filter(|(p, _)| !p.ends_with("FB_Counter.TcPOU"))
        .collect();
    let c = check(&project(&without));
    assert!(!c.ok);
    let d = &c.diagnostics[0];
    assert_eq!(d.file.as_deref(), Some("Demo/Demo.plcproj"));
    assert!(d.message.contains("FB_Counter"), "{d:#?}");
}

#[test]
fn tag_outline() {
    let src = "CONFIGURATION C
  RESOURCE R ON Opta
    TASK Fast (INTERVAL := T#10ms, PRIORITY := 1);
    PROGRAM Main WITH Fast : SealIn;
  END_RESOURCE
END_CONFIGURATION
";
    let c = check(&project(&[("main.st", SEAL_IN), ("config.st", src)]));
    assert!(c.ok, "{:#?}", c.diagnostics);
    let t = tags::tags(c.parsed.as_ref().unwrap());
    let speed = t.image.iter().find(|i| i.name == "speed").unwrap();
    assert_eq!((speed.area, speed.byte_offset, speed.bits), ('Q', 2, 16));
    let start = t.image.iter().find(|i| i.name == "start").unwrap();
    assert_eq!((start.byte_offset, start.bit), (0, Some(0)));
    assert_eq!(t.programs[0].variables.len(), 5);
    assert_eq!(t.tasks.len(), 1);
    assert_eq!(t.tasks[0].interval_ns, Some(10_000_000));
    assert_eq!(t.tasks[0].priority, Some(1));
    assert_eq!(t.tasks[0].instances[0].name, "R.Main");
}
