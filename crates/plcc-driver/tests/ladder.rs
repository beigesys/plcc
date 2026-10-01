// SPDX-License-Identifier: MPL-2.0

//! Ladder model inputs (plcc studio projects): checked as the L5X they are
//! written as, with diagnostics traced back to rungs and elements.

use plcc_driver::{Project, Severity, check, ladder};

fn model(rungs: &str, extra_tags: &str) -> String {
    format!(
        r#"{{"dialect":"logix","name":"Demo","globals":[
 {{"name":"StartPB","data_type":"BOOL","section":"global","address":"%MX0.0"}},
 {{"name":"StopPB","data_type":"BOOL","section":"global","address":"%MX2.0"}},
 {{"name":"Motor","data_type":"BOOL","section":"global","address":"%QX0.0"}},
 {{"name":"Level","data_type":"INT","section":"global","address":"%IW2"}},
 {{"name":"High","data_type":"BOOL","section":"global","address":"%QX0.4"}},
 {{"name":"RunTimer","data_type":"TIMER","section":"global"}}{extra_tags}],
"pous":[{{"id":1,"name":"MainProgram","kind":"program","routines":[{{"id":2,"name":"MainRoutine","rungs":[{rungs}]}}]}}],
"tasks":[{{"name":"MainTask","interval_ms":10,"programs":["MainProgram"]}}]}}"#
    )
}

const SEAL: &str = r#"{"id":3,"elements":[{"type":"branch","id":4,"legs":[[{"type":"contact","id":5,"operand":"StartPB","kind":"no"}],[{"type":"contact","id":6,"operand":"Motor","kind":"no"}]]},{"type":"contact","id":7,"operand":"StopPB","kind":"nc"},{"type":"coil","id":8,"operand":"Motor","kind":"normal"}]}"#;

fn project(json: &str) -> Project {
    Project {
        files: [("project.json".to_string(), json.to_string())].into(),
        entry: Some(vec!["project.json".into()]),
        ..Default::default()
    }
}

#[test]
fn a_valid_model_checks_clean_with_its_io_and_tasks() {
    let ton = r#"{"id":9,"elements":[{"type":"contact","id":10,"operand":"Motor","kind":"no"},{"type":"block","id":11,"name":"TON","pins":[{"name":"Timer","value":"RunTimer"},{"name":"Preset","value":"5000"},{"name":"Accum","value":"0"}]}]}"#;
    let c = check(&project(&model(&format!("{SEAL},{ton}"), "")));
    assert!(c.ok, "{:#?}", c.diagnostics);
    let tags = plcc_driver::tags::tags(c.parsed.as_ref().unwrap());
    // The variables' addresses are bound to the process image.
    let addrs: Vec<&str> = tags.image.iter().map(|t| t.address.as_str()).collect();
    for a in ["%MX0.0", "%MX2.0", "%QX0.0", "%IW2", "%QX0.4"] {
        assert!(addrs.contains(&a), "{a} in {addrs:?}");
    }
    // The model's task is a periodic task.
    let t = tags.tasks.iter().find(|t| t.name.eq_ignore_ascii_case("MainTask")).unwrap();
    assert_eq!(t.interval_ns, Some(10_000_000));
}

#[test]
fn type_errors_point_at_the_element_and_operand() {
    // TON on a BOOL; GRT on an undeclared tag inside an expression.
    let bad = r#"{"id":9,"elements":[{"type":"contact","id":10,"operand":"Motor","kind":"no"},{"type":"block","id":11,"name":"TON","pins":[{"name":"Timer","value":"Motor"},{"name":"Preset","value":"5000"},{"name":"Accum","value":"0"}]}]},
      {"id":12,"elements":[{"type":"block","id":13,"name":"GRT","pins":[{"name":"Source A","value":"Level + Foo"},{"name":"Source B","value":"2000"}]},{"type":"coil","id":14,"operand":"High","kind":"normal"}]}"#;
    let c = check(&project(&model(&format!("{SEAL},{bad}"), "")));
    assert!(!c.ok);
    let errs: Vec<_> = c.diagnostics.iter().filter(|d| d.severity == Severity::Error).collect();
    let ton = errs.iter().find(|d| d.message.contains("TIMER")).expect("TON error");
    let at = ton.ladder.as_ref().unwrap();
    assert_eq!((at.rung, at.element, at.operand), (Some(9), Some(11), Some(0)));
    assert_eq!(at.routine.as_deref(), Some("MainRoutine"));
    // The span is a range of the rung's text `XIC(Motor)TON(Motor,5000,0);`.
    let span = ton.span.as_ref().unwrap();
    assert_eq!((span.start.line, span.start.col), (1, 15));
    let foo = errs.iter().find(|d| d.message.contains("Foo")).expect("Foo error");
    let at = foo.ladder.as_ref().unwrap();
    assert_eq!((at.rung, at.element, at.operand), (Some(12), Some(13), Some(0)));
}

#[test]
fn undeclared_tags_are_errors_on_their_element() {
    let r = r#"{"id":9,"elements":[{"type":"contact","id":10,"operand":"Nope","kind":"no"},{"type":"coil","id":11,"operand":"Gone.DN","kind":"normal"}]}"#;
    let c = check(&project(&model(r, "")));
    assert!(!c.ok);
    let found: Vec<_> = c
        .diagnostics
        .iter()
        .filter_map(|d| d.ladder.as_ref().map(|l| (l.element, l.tag.clone())))
        .collect();
    assert!(found.contains(&(Some(10), Some("Nope".into()))), "{found:?}");
    assert!(found.contains(&(Some(11), Some("Gone".into()))), "{found:?}");
}

#[test]
fn st_box_errors_point_into_the_code() {
    let r = r#"{"id":9,"elements":[{"type":"contact","id":10,"operand":"Motor","kind":"no"},{"type":"st","id":11,"code":"Level := 1;\nLevel := Bogus + 1;"}]}"#;
    let c = check(&project(&model(r, "")));
    let d = c
        .diagnostics
        .iter()
        .find(|d| d.message.contains("Bogus"))
        .unwrap_or_else(|| panic!("{:#?}", c.diagnostics));
    let at = d.ladder.as_ref().unwrap();
    assert_eq!((at.rung, at.element), (Some(9), Some(11)));
    let span = d.span.as_ref().unwrap();
    assert_eq!((span.start.line, span.start.col), (2, 10));
}

#[test]
fn st_routines_and_edge_contacts() {
    // A routine that is one ST box is an ST routine (Logix ST).
    let json = model(SEAL, "").replace(
        "]}]}],\n\"tasks\"",
        r#"]},{"id":20,"name":"Calc","rungs":[{"id":21,"elements":[{"type":"st","id":22,"code":"Level := Level + 1;\nMotor := Missing;"}]}]}]}],"tasks""#,
    );
    let c = check(&project(&json));
    let d = c.diagnostics.iter().find(|d| d.message.contains("Missing")).unwrap();
    let at = d.ladder.as_ref().unwrap();
    assert_eq!((at.routine.as_deref(), at.element), (Some("Calc"), Some(22)));
    assert_eq!(d.span.as_ref().unwrap().start.line, 2);
    // Logix has no edge contact: an error on that element.
    let r = r#"{"id":9,"elements":[{"type":"contact","id":10,"operand":"Motor","kind":"rising"}]}"#;
    let c = check(&project(&model(r, "")));
    let d = c.diagnostics.iter().find(|d| d.severity == Severity::Error).unwrap();
    assert_eq!(d.ladder.as_ref().unwrap().element, Some(10));
}

#[test]
fn bad_addresses_name_the_tag() {
    let c = check(&project(&model(
        SEAL,
        r#",{"name":"Wrong","data_type":"BOOL","section":"global","address":"%IW4"}"#,
    )));
    assert!(!c.ok);
    let d = c
        .diagnostics
        .iter()
        .find(|d| d.ladder.as_ref().and_then(|l| l.tag.as_deref()) == Some("Wrong"))
        .unwrap_or_else(|| panic!("{:#?}", c.diagnostics));
    assert_eq!(d.severity, Severity::Error);
}

#[test]
fn fault_sites_locate_rungs() {
    let json = model(SEAL, "");
    let (lowered, _) = ladder::lower("p.json", &json).ok().unwrap();
    let l5x = lowered.l5x.as_deref().unwrap();
    let at = l5x.find("XIO(StopPB)").unwrap();
    let line = l5x[..at].matches('\n').count() as u32 + 1;
    let col = (at - l5x[..at].rfind('\n').map_or(0, |i| i + 1)) as u32 + 1;
    let r = ladder::locate_site(&json, line, col).unwrap();
    assert_eq!((r.rung, r.element), (Some(3), Some(7)));
}
