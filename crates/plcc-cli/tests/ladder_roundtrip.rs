// SPDX-License-Identifier: MPL-2.0

//! Round-trip guarantees of the ladder model (docs/ladder-translation.md):
//!
//! * LD → ST → LD is the identity for rungs in canonical form (contacts,
//!   branches, one coil / set / reset coils, standard FB boxes with a coil on
//!   their power output);
//! * LD → ST → LD → ST runs like LD → ST for every LD fixture;
//! * the PLCopen XML the writer makes follows the TC6 v2.01 element order,
//!   every connection names an element of its body, and localIds are unique.

mod jit;

use plcc_ladder::model::*;
use std::collections::HashSet;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).expect("fixture")
}

/// Canonical rungs: what the IEC lowering prints as one ST statement (or one
/// FB call with its output copies) and `from_st` draws back the same.
const CANONICAL: &str = r#"{
  "dialect": "iec", "name": "Canon",
  "pous": [ { "id": 1, "name": "Canon", "kind": "program",
    "variables": [
      {"name": "a", "data_type": "BOOL"}, {"name": "b", "data_type": "BOOL"},
      {"name": "c", "data_type": "BOOL"}, {"name": "d", "data_type": "BOOL"},
      {"name": "x", "data_type": "BOOL"}, {"name": "y", "data_type": "BOOL"},
      {"name": "z", "data_type": "BOOL"}, {"name": "Done", "data_type": "BOOL"},
      {"name": "Reached", "data_type": "BOOL"}, {"name": "Elapsed", "data_type": "TIME"},
      {"name": "Count", "data_type": "INT"},
      {"name": "T1", "data_type": "TON"}, {"name": "C1", "data_type": "CTU"}
    ],
    "routines": [ { "id": 2, "name": "Canon", "rungs": [
      { "id": 3, "elements": [
        { "type": "contact", "id": 4, "operand": "a" },
        { "type": "contact", "id": 5, "operand": "b", "kind": "nc" },
        { "type": "coil", "id": 6, "operand": "x" } ] },
      { "id": 7, "elements": [
        { "type": "branch", "id": 8, "legs": [
          [ { "type": "contact", "id": 9, "operand": "a" } ],
          [ { "type": "contact", "id": 10, "operand": "b" },
            { "type": "contact", "id": 11, "operand": "c", "kind": "nc" } ] ] },
        { "type": "contact", "id": 12, "operand": "d" },
        { "type": "coil", "id": 13, "operand": "y" } ] },
      { "id": 14, "elements": [
        { "type": "contact", "id": 15, "operand": "a" },
        { "type": "branch", "id": 16, "legs": [
          [ { "type": "contact", "id": 17, "operand": "b" } ],
          [ { "type": "contact", "id": 18, "operand": "c" } ] ] },
        { "type": "coil", "id": 19, "operand": "z" } ] },
      { "id": 20, "elements": [
        { "type": "contact", "id": 21, "operand": "c" },
        { "type": "coil", "id": 22, "operand": "x", "kind": "set" } ] },
      { "id": 23, "elements": [
        { "type": "contact", "id": 24, "operand": "d" },
        { "type": "coil", "id": 25, "operand": "x", "kind": "reset" } ] },
      { "id": 26, "elements": [
        { "type": "contact", "id": 27, "operand": "a" },
        { "type": "block", "id": 28, "name": "TON", "instance": "T1",
          "pins": [ {"name": "IN"}, {"name": "PT", "value": "T#100ms"},
                    {"name": "Q", "dir": "output"}, {"name": "ET", "dir": "output", "value": "Elapsed"} ],
          "power_in": "IN", "power_out": "Q" },
        { "type": "coil", "id": 29, "operand": "Done" } ] },
      { "id": 30, "elements": [
        { "type": "contact", "id": 31, "operand": "b" },
        { "type": "block", "id": 32, "name": "CTU", "instance": "C1",
          "pins": [ {"name": "CU"}, {"name": "R", "value": "d"}, {"name": "PV", "value": "3"},
                    {"name": "Q", "dir": "output"}, {"name": "CV", "dir": "output", "value": "Count"} ],
          "power_in": "CU", "power_out": "Q" },
        { "type": "coil", "id": 33, "operand": "Reached" } ] }
    ] } ] } ]
}"#;

/// The model lowered to canonical ST (printed and parsed again), drawn back.
fn ld_st_ld(model: &Project) -> (String, Project) {
    let (unit, errs) = plcc_ladder::to_unit(model, false);
    assert!(errs.is_empty(), "{errs:?}");
    let st = plcc_st::print_unit(&unit);
    let (again, errs) = plcc_st::parse(&st);
    assert!(errs.is_empty(), "{errs:?}\n{st}");
    let (back, _) = plcc_ladder::from_st::from_unit(&again);
    (st, back)
}

fn rungs_shape(p: &Project) -> serde_json::Value {
    let mut p = p.clone();
    p.renumber();
    let rungs: Vec<&Vec<Rung>> = p
        .pous
        .iter()
        .flat_map(|q| q.routines.iter().map(|r| &r.rungs))
        .collect();
    serde_json::to_value(rungs).unwrap()
}

#[test]
fn ld_to_st_to_ld_is_identity_for_canonical_rungs() {
    let canon = Project::from_json(CANONICAL).unwrap();
    let (st, back) = ld_st_ld(&canon);
    assert_eq!(
        rungs_shape(&canon),
        rungs_shape(&back),
        "canonical rungs changed:\n{st}\n{}",
        back.to_json()
    );
    assert_eq!(canon.pous[0].variables, back.pous[0].variables);
    // The seal-in fixture is canonical too.
    let (seal, _) = plcc_plcopen::ladder::read(&read("tests/fixtures/plcopen/ld_seal_in.xml"));
    let seal = seal.unwrap();
    let (st, back) = ld_st_ld(&seal);
    let ladder_pou = |p: &Project| {
        let mut q = p.clone();
        q.pous.retain(|x| x.name == "SealIn");
        q.pous.iter_mut().for_each(|x| {
            x.routines
                .iter_mut()
                .for_each(|r| r.rungs.iter_mut().for_each(|g| g.comment = None))
        });
        rungs_shape(&q)
    };
    assert_eq!(
        ladder_pou(&seal),
        ladder_pou(&back),
        "seal-in changed:\n{st}"
    );
}

#[test]
fn ld_to_st_to_ld_runs_like_ld() {
    let _clock = jit::clock();
    for f in [
        "tests/fixtures/plcopen/ld_seal_in.xml",
        "tests/fixtures/plcopen/ld_branches.xml",
        "tests/fixtures/plcopen/ld_coils_edges.xml",
        "tests/fixtures/plcopen/ld_jumps.xml",
        "tests/fixtures/plcopen/ld_timer_counter.xml",
    ] {
        let src = read(f);
        let (m, _) = plcc_plcopen::ladder::read(&src);
        let m = m.unwrap();
        let (u1, _) = plcc_ladder::to_unit(&m, false);
        let (_, back) = ld_st_ld(&m);
        let (u2, errs) = plcc_ladder::to_unit(&back, false);
        assert!(errs.is_empty(), "{f}: {errs:?}");
        let (direct, _) = plcc_plcopen::parse(&src);
        let mut u1 = u1;
        let mut u2 = u2;
        for d in &direct.declarations {
            if matches!(d, plcc_st::Declaration::Configuration(_)) {
                u1.declarations.push(d.clone());
                u2.declarations.push(d.clone());
            }
        }
        let bools: Vec<String> = m
            .pous
            .iter()
            .flat_map(|p| &p.variables)
            .filter(|v| v.data_type == "BOOL")
            .map(|v| v.name.clone())
            .collect();
        let ctx_a = inkwell::context::Context::create();
        let ctx_b = inkwell::context::Context::create();
        let a = jit::load(&ctx_a, "ld", &jit::with_libs(u1, false)).unwrap();
        let b = jit::load(&ctx_b, "ld-st-ld", &jit::with_libs(u2, false))
            .unwrap_or_else(|e| panic!("{f}: {e}"));
        // Randomize everything BOOL the rungs read: inputs and outputs alike
        // are driven, so the two must agree on every scan.
        jit::differential(&a, &b, &bools, &[], 200, 23, 25).unwrap_or_else(|e| panic!("{f}: {e}"));
    }
}

// ── The written PLCopen XML against the TC6 v2.01 element order ──

/// Child elements each element may have, in schema order (TC6 XML v2.01,
/// `tc6_xml_v201.xsd`), for the elements the writer emits.
fn schema_order(tag: &str) -> Option<&'static [&'static str]> {
    Some(match tag {
        "project" => &[
            "fileHeader",
            "contentHeader",
            "types",
            "instances",
            "addData",
            "documentation",
        ],
        "contentHeader" => &["Comment", "coordinateInfo", "addDataInfo", "addData"],
        "types" => &["dataTypes", "pous"],
        "pou" => &[
            "interface",
            "actions",
            "transitions",
            "body",
            "addData",
            "documentation",
        ],
        "interface" => &[
            "returnType",
            "localVars",
            "tempVars",
            "inputVars",
            "outputVars",
            "inOutVars",
            "externalVars",
            "globalVars",
            "accessVars",
            "addData",
            "documentation",
        ],
        "variable" => &[
            "type",
            "initialValue",
            "addData",
            "documentation",
            "connectionPointIn",
            "connectionPointOut",
        ],
        "action" => &["body", "addData", "documentation"],
        "body" => &["IL", "ST", "FBD", "LD", "SFC", "addData", "documentation"],
        "leftPowerRail" => &["position", "connectionPointOut", "addData", "documentation"],
        "rightPowerRail" => &["position", "connectionPointIn", "addData", "documentation"],
        "contact" | "coil" => &[
            "position",
            "connectionPointIn",
            "connectionPointOut",
            "variable",
            "addData",
            "documentation",
        ],
        "block" => &[
            "position",
            "inputVariables",
            "inOutVariables",
            "outputVariables",
            "addData",
            "documentation",
        ],
        "inVariable" => &[
            "position",
            "connectionPointOut",
            "expression",
            "addData",
            "documentation",
        ],
        "outVariable" => &[
            "position",
            "connectionPointIn",
            "expression",
            "addData",
            "documentation",
        ],
        "jump" | "return" => &["position", "connectionPointIn", "addData", "documentation"],
        "label" => &["position", "addData", "documentation"],
        "comment" => &["position", "content", "addData", "documentation"],
        "connectionPointIn" => &["relPosition", "connection", "expression", "addData"],
        "connectionPointOut" => &["relPosition", "expression", "addData"],
        "connection" => &["position", "addData"],
        _ => return None,
    })
}

fn check_xml(xml: &str) -> Result<(), String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| e.to_string())?;
    // Note: the interface allows its sections in any order in practice; the
    // XSD's choice group repeats, so only elements with a fixed sequence are
    // checked strictly.
    for n in doc.descendants().filter(|n| n.is_element()) {
        let tag = n.tag_name().name();
        if tag == "interface"
            || tag == "variable"
                && n.parent().is_some_and(|p| {
                    p.tag_name().name() == "inputVariables"
                        || p.tag_name().name() == "outputVariables"
                        || p.tag_name().name() == "inOutVariables"
                })
        {
            continue;
        }
        let Some(order) = schema_order(tag) else {
            continue;
        };
        let mut last = 0;
        for c in n.children().filter(|c| c.is_element()) {
            let ct = c.tag_name().name();
            let Some(k) = order.iter().position(|o| *o == ct) else {
                return Err(format!("<{ct}> is not allowed in <{tag}>"));
            };
            if k < last {
                return Err(format!("<{ct}> comes too late in <{tag}>"));
            }
            last = k;
        }
    }
    // Bodies: unique localIds, connections to existing elements.
    for body in doc.descendants().filter(|n| n.has_tag_name("LD")) {
        let mut ids = HashSet::new();
        for e in body.children().filter(|c| c.is_element()) {
            let id = e
                .attribute("localId")
                .ok_or(format!("<{}> has no localId", e.tag_name().name()))?;
            if !ids.insert(id.to_string()) {
                return Err(format!("localId {id} is used twice"));
            }
        }
        for c in body.descendants().filter(|n| n.has_tag_name("connection")) {
            let r = c
                .attribute("refLocalId")
                .ok_or("a connection without refLocalId")?;
            if !ids.contains(r) {
                return Err(format!("connection to unknown localId {r}"));
            }
            if c.children().filter(|p| p.has_tag_name("position")).count() < 2 {
                return Err("a connection without its two end points".into());
            }
        }
        for p in body
            .descendants()
            .filter(|n| n.has_tag_name("connectionPointIn") || n.has_tag_name("connectionPointOut"))
        {
            if !p.children().any(|c| c.has_tag_name("relPosition")) {
                return Err("a connection point without relPosition".into());
            }
        }
    }
    Ok(())
}

#[test]
fn written_plcopen_follows_the_schema() {
    let mut models = vec![Project::from_json(CANONICAL).unwrap()];
    for f in [
        "tests/fixtures/plcopen/ld_seal_in.xml",
        "tests/fixtures/plcopen/ld_branches.xml",
        "tests/fixtures/plcopen/ld_coils_edges.xml",
        "tests/fixtures/plcopen/ld_jumps.xml",
        "tests/fixtures/plcopen/ld_timer_counter.xml",
    ] {
        models.push(plcc_plcopen::ladder::read(&read(f)).0.unwrap());
    }
    // Logix fixtures translated to IEC, and ST drawn as ladder, too.
    for f in [
        "tests/fixtures/l5x/seal_in.L5X",
        "tests/fixtures/l5x/bits_branches.L5X",
    ] {
        let (m, _) = plcc_l5x::ladder::read(&read(f));
        models.push(plcc_l5x::ladder::translate(&m.unwrap(), Dialect::Iec).0);
    }
    let (u, _) = plcc_st::parse(&read("tests/fixtures/programs/stdlib_blinker.st"));
    models.push(plcc_ladder::from_st::from_unit(&u).0);
    for m in models {
        let (xml, _) = plcc_plcopen::ladder::write(&m).unwrap();
        check_xml(&xml).unwrap_or_else(|e| panic!("{}: {e}\n{xml}", m.name));
        // And it reads back.
        let (_, errs) = plcc_plcopen::parse(&xml);
        assert!(errs.is_empty(), "{}: {errs:?}", m.name);
    }
}
