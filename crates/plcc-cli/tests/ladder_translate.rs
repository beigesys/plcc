// SPDX-License-Identifier: MPL-2.0

//! IEC ladder ↔ Logix ladder (`plcc_ladder::translate`): the translated
//! program, compiled with the other dialect's semantics, runs like the
//! original over randomized input sequences (JIT, differential). Variables
//! are paired by name; the first scan is skipped (Logix prescan).

mod jit;

use plcc_ladder::model::*;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).expect("fixture")
}

fn inputs(p: &Project) -> Vec<String> {
    let mut read = Vec::new();
    let mut written = Vec::new();
    for pou in &p.pous {
        for r in &pou.routines {
            for g in &r.rungs {
                walk(&g.elements, &mut |e| match e {
                    Element::Contact(c) => read.push(c.operand.clone()),
                    Element::Coil(c) => written.push(c.operand.clone()),
                    Element::Block(b) => {
                        for (k, pin) in b.pins.iter().enumerate() {
                            if let Some(v) = &pin.value {
                                if pin.dir == PinDir::Input && b.instance.is_some() {
                                    read.push(v.clone());
                                } else if pin.dir == PinDir::Output
                                    || k > 0 && p.dialect == Dialect::Logix
                                {
                                    written.push(v.clone());
                                }
                            }
                        }
                    }
                    _ => {}
                });
            }
        }
    }
    read.retain(|r| {
        r.chars().all(|c| c.is_alphanumeric() || c == '_')
            && !written.iter().any(|w| w.eq_ignore_ascii_case(r))
    });
    read.sort();
    read.dedup();
    read
}

/// An IEC model with the mapped elements: seal-in, set/reset coils, edge
/// contact and coils, a negated coil, TON / TOF / RTO / CTU with Q driving
/// the rung, and ADD / MOVE / compare boxes.
const IEC_MIX: &str = r#"{
  "dialect": "iec", "name": "Mix",
  "pous": [ { "id": 1, "name": "Mix", "kind": "program",
    "variables": [
      {"name": "Start", "data_type": "BOOL"}, {"name": "Stop", "data_type": "BOOL"},
      {"name": "Motor", "data_type": "BOOL"}, {"name": "Latch", "data_type": "BOOL"},
      {"name": "En", "data_type": "BOOL"}, {"name": "Done", "data_type": "BOOL"},
      {"name": "OffDone", "data_type": "BOOL"}, {"name": "Pulse", "data_type": "BOOL"},
      {"name": "Rst", "data_type": "BOOL"}, {"name": "Reached", "data_type": "BOOL"},
      {"name": "Hold", "data_type": "BOOL"}, {"name": "RDone", "data_type": "BOOL"},
      {"name": "Rise", "data_type": "BOOL"}, {"name": "Fall", "data_type": "BOOL"},
      {"name": "NotEn", "data_type": "BOOL"}, {"name": "Edge", "data_type": "BOOL"},
      {"name": "N", "data_type": "DINT"}, {"name": "M", "data_type": "DINT"},
      {"name": "Big", "data_type": "BOOL"},
      {"name": "T1", "data_type": "TON"}, {"name": "T2", "data_type": "TOF"},
      {"name": "R1", "data_type": "RTO"}, {"name": "C1", "data_type": "CTU"}
    ],
    "routines": [ { "id": 2, "name": "Main", "rungs": [
      { "id": 10, "elements": [
        { "type": "branch", "id": 11, "legs": [
          [ { "type": "contact", "id": 12, "operand": "Start" } ],
          [ { "type": "contact", "id": 13, "operand": "Motor" } ] ] },
        { "type": "contact", "id": 14, "operand": "Stop", "kind": "nc" },
        { "type": "coil", "id": 15, "operand": "Motor" } ] },
      { "id": 20, "elements": [
        { "type": "contact", "id": 21, "operand": "Start" },
        { "type": "coil", "id": 22, "operand": "Latch", "kind": "set" } ] },
      { "id": 23, "elements": [
        { "type": "contact", "id": 24, "operand": "Stop" },
        { "type": "coil", "id": 25, "operand": "Latch", "kind": "reset" } ] },
      { "id": 30, "elements": [
        { "type": "contact", "id": 31, "operand": "En" },
        { "type": "block", "id": 32, "name": "TON", "instance": "T1",
          "pins": [ {"name": "IN"}, {"name": "PT", "value": "T#50ms"},
                    {"name": "Q", "dir": "output"}, {"name": "ET", "dir": "output"} ],
          "power_in": "IN", "power_out": "Q" },
        { "type": "coil", "id": 33, "operand": "Done" } ] },
      { "id": 34, "elements": [
        { "type": "contact", "id": 35, "operand": "En" },
        { "type": "block", "id": 36, "name": "TOF", "instance": "T2",
          "pins": [ {"name": "IN"}, {"name": "PT", "value": "T#30ms"},
                    {"name": "Q", "dir": "output"}, {"name": "ET", "dir": "output"} ],
          "power_in": "IN", "power_out": "Q" },
        { "type": "coil", "id": 37, "operand": "OffDone" } ] },
      { "id": 40, "elements": [
        { "type": "contact", "id": 41, "operand": "Pulse" },
        { "type": "block", "id": 42, "name": "CTU", "instance": "C1",
          "pins": [ {"name": "CU"}, {"name": "R", "value": "Rst"}, {"name": "PV", "value": "3"},
                    {"name": "Q", "dir": "output"}, {"name": "CV", "dir": "output"} ],
          "power_in": "CU", "power_out": "Q" },
        { "type": "coil", "id": 43, "operand": "Reached" } ] },
      { "id": 50, "elements": [
        { "type": "contact", "id": 51, "operand": "Hold" },
        { "type": "block", "id": 52, "name": "RTO", "instance": "R1",
          "pins": [ {"name": "IN"}, {"name": "R", "value": "Rst"}, {"name": "PT", "value": "T#40ms"},
                    {"name": "Q", "dir": "output"}, {"name": "ET", "dir": "output"} ],
          "power_in": "IN", "power_out": "Q" },
        { "type": "coil", "id": 53, "operand": "RDone" } ] },
      { "id": 60, "elements": [
        { "type": "contact", "id": 61, "operand": "Pulse" },
        { "type": "coil", "id": 62, "operand": "Rise", "kind": "rising" } ] },
      { "id": 63, "elements": [
        { "type": "contact", "id": 64, "operand": "Pulse" },
        { "type": "coil", "id": 65, "operand": "Fall", "kind": "falling" } ] },
      { "id": 66, "elements": [
        { "type": "contact", "id": 67, "operand": "En" },
        { "type": "coil", "id": 68, "operand": "NotEn", "kind": "negated" } ] },
      { "id": 70, "elements": [
        { "type": "contact", "id": 71, "operand": "Start", "kind": "rising" },
        { "type": "contact", "id": 72, "operand": "En" },
        { "type": "coil", "id": 73, "operand": "Edge" } ] },
      { "id": 80, "elements": [
        { "type": "contact", "id": 81, "operand": "Rise" },
        { "type": "block", "id": 82, "name": "ADD",
          "pins": [ {"name": "EN"}, {"name": "IN1", "value": "N"}, {"name": "IN2", "value": "1"},
                    {"name": "ENO", "dir": "output"}, {"name": "OUT", "dir": "output", "value": "N"} ],
          "power_in": "EN", "power_out": "ENO" } ] },
      { "id": 83, "elements": [
        { "type": "contact", "id": 84, "operand": "Stop" },
        { "type": "block", "id": 85, "name": "MOVE",
          "pins": [ {"name": "EN"}, {"name": "IN", "value": "N"},
                    {"name": "ENO", "dir": "output"}, {"name": "OUT", "dir": "output", "value": "M"} ],
          "power_in": "EN", "power_out": "ENO" } ] },
      { "id": 86, "elements": [
        { "type": "block", "id": 87, "name": "GT",
          "pins": [ {"name": "IN1", "value": "N"}, {"name": "IN2", "value": "4"},
                    {"name": "OUT", "dir": "output"} ],
          "power_out": "OUT" },
        { "type": "coil", "id": 88, "operand": "Big" } ] }
    ] } ] } ]
}"#;

fn iec_unit(model: &Project) -> plcc_st::CompilationUnit {
    let (unit, errs) = plcc_ladder::to_unit(model, false);
    assert!(errs.is_empty(), "{errs:?}");
    unit
}

fn logix_unit(model: &Project) -> (String, plcc_st::CompilationUnit) {
    let (l5x, _warnings) = plcc_l5x::ladder::write(model).unwrap_or_else(|e| panic!("{e:?}"));
    let (unit, errs) = plcc_l5x::parse(&l5x);
    assert!(errs.iter().all(|e| e.is_warning()), "{errs:?}\n{l5x}");
    (l5x, unit)
}

/// Compare the elementary variables the model declares (not the members of
/// FB instances / structure tags, which mean different things).
fn opts(p: &Project) -> jit::Diff {
    let only = p
        .globals
        .iter()
        .chain(p.pous.iter().flat_map(|q| &q.variables))
        .filter(|v| {
            matches!(
                v.data_type.to_ascii_uppercase().as_str(),
                "BOOL" | "SINT" | "INT" | "DINT" | "LINT" | "REAL" | "LREAL"
            )
        })
        .map(|v| v.name.clone())
        .collect();
    jit::Diff {
        by_name: true,
        skip_first: 1,
        only: Some(only),
        ..Default::default()
    }
}

#[test]
fn iec_to_logix_runs_alike() {
    let _clock = jit::clock();
    let iec = Project::from_json(IEC_MIX).unwrap();
    let (logix, warnings) = plcc_ladder::translate::translate(&iec, Dialect::Logix);
    assert!(!warnings.is_empty());
    for w in &warnings {
        eprintln!("warning: {w}");
    }
    let (l5x, lu) = logix_unit(&logix);
    let ctx_a = inkwell::context::Context::create();
    let ctx_b = inkwell::context::Context::create();
    let a = jit::load(&ctx_a, "iec", &jit::with_libs(iec_unit(&iec), false)).unwrap();
    let b = jit::load(&ctx_b, "logix", &jit::with_libs(lu, true))
        .unwrap_or_else(|e| panic!("{e}\n{l5x}"));
    let n = jit::differential_with(&a, &b, &inputs(&iec), 400, 3, 25, &opts(&iec))
        .unwrap_or_else(|e| panic!("{e}\n{l5x}"));
    eprintln!("IEC -> Logix: {n} comparisons");
}

#[test]
fn iec_fixtures_to_logix_run_alike() {
    let _clock = jit::clock();
    for f in [
        "tests/fixtures/plcopen/ld_seal_in.xml",
        "tests/fixtures/plcopen/ld_coils_edges.xml",
    ] {
        let src = read(f);
        let (iec, errs) = plcc_plcopen::ladder::read(&src);
        assert!(errs.is_empty());
        let iec = iec.unwrap();
        let (logix, _) = plcc_ladder::translate::translate(&iec, Dialect::Logix);
        let (l5x, lu) = logix_unit(&logix);
        let (direct, _) = plcc_plcopen::parse(&src);
        let ctx_a = inkwell::context::Context::create();
        let ctx_b = inkwell::context::Context::create();
        let a = jit::load(&ctx_a, "iec", &jit::with_libs(direct, false)).unwrap();
        let b = jit::load(&ctx_b, "logix", &jit::with_libs(lu, true))
            .unwrap_or_else(|e| panic!("{f}: {e}\n{l5x}"));
        jit::differential_with(&a, &b, &inputs(&iec), 300, 9, 25, &opts(&iec))
            .unwrap_or_else(|e| panic!("{f}: {e}\n{l5x}"));
    }
}

#[test]
fn logix_to_iec_runs_alike() {
    let _clock = jit::clock();
    for (f, ignore) in [
        ("tests/fixtures/l5x/seal_in.L5X", vec![]),
        // Bits32 is written through an indirect bit `Bits32.[Idx]`, which
        // has no IEC ladder form (reported NOT TRANSLATED).
        (
            "tests/fixtures/l5x/bits_branches.L5X",
            vec!["Bits32".to_string()],
        ),
        ("tests/fixtures/l5x/timers_counters.L5X", vec![]),
    ] {
        let src = read(f);
        let (logix, errs) = plcc_l5x::ladder::read(&src);
        assert!(errs.iter().all(|e| e.is_warning()));
        let logix = logix.unwrap();
        let (iec, warnings) = plcc_l5x::ladder::translate(&logix, Dialect::Iec);
        let untranslated: Vec<&String> = warnings
            .iter()
            .filter(|w| w.contains("NOT TRANSLATED"))
            .collect();
        let (original, _) = plcc_l5x::parse(&src);
        let unit = iec_unit(&iec);
        let st = plcc_st::print_unit(&unit);
        let ctx_a = inkwell::context::Context::create();
        let ctx_b = inkwell::context::Context::create();
        let a = jit::load(&ctx_a, "logix", &jit::with_libs(original, true)).unwrap();
        let b = match jit::load(&ctx_b, "iec", &jit::with_libs(unit, false)) {
            Ok(b) => b,
            Err(e) => panic!("{f}: {e}\n{st}\n{untranslated:#?}"),
        };
        let o = jit::Diff {
            ignore,
            ..opts(&logix)
        };
        match jit::differential_with(&a, &b, &inputs(&logix), 300, 13, 25, &o) {
            Ok(n) => eprintln!(
                "{f}: {n} comparisons, {} not translated",
                untranslated.len()
            ),
            Err(e) => panic!("{f}: {e}\n{st}\nnot translated: {untranslated:#?}"),
        }
    }
}

/// The math fixture is about Logix-only behaviour (status flags, division by
/// zero, round-half-even stores): its translation says so, element by element.
#[test]
fn logix_math_differences_are_reported() {
    let src = read("tests/fixtures/l5x/math.L5X");
    let (logix, _) = plcc_l5x::ladder::read(&src);
    let (iec, warnings) = plcc_l5x::ladder::translate(&logix.unwrap(), Dialect::Iec);
    let has = |s: &str| warnings.iter().any(|w| w.contains(s));
    assert!(
        has("S:V: S:V is a Logix module or system tag"),
        "{warnings:#?}"
    );
    assert!(
        has("DIV(Num,Zero,DivZ)") && has("division by zero"),
        "{warnings:#?}"
    );
    assert!(has("NOT TRANSLATED") && has("BTD"), "{warnings:#?}");
    // Everything else still lowers.
    let (_, errs) = plcc_ladder::to_unit(&iec, false);
    assert!(errs.is_empty(), "{errs:?}");
}
