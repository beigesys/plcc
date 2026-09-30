// SPDX-License-Identifier: MPL-2.0

//! The ladder model (`plcc-ladder`) against the existing ladder paths:
//!
//! * PLCopen LD: the model path (XML → model → ST AST) runs every fixture
//!   exactly like the direct LD lowering, over randomized input sequences
//!   (JIT, differential);
//! * PLCopen round trip: model → XML → model is the identity, and the written
//!   XML compiles and runs like the original;
//! * Rockwell rung text: text → model → text is the identity on canonical text.

mod jit;

use plcc_ladder::model::*;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).expect("fixture")
}

const LD_FIXTURES: &[&str] = &[
    "tests/fixtures/plcopen/ld_seal_in.xml",
    "tests/fixtures/plcopen/ld_branches.xml",
    "tests/fixtures/plcopen/ld_coils_edges.xml",
    "tests/fixtures/plcopen/ld_jumps.xml",
    "tests/fixtures/plcopen/ld_timer_counter.xml",
];

/// Contact operands no coil or output writes: the inputs to randomize.
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
                        for pin in &b.pins {
                            if pin.dir == PinDir::Output
                                && let Some(v) = &pin.value
                            {
                                written.push(v.clone());
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

/// The model of a PLCopen file lowered to ST, with the file's data types and
/// configuration (not part of the ladder model) taken from the direct path.
fn model_unit(src: &str) -> (Project, plcc_st::CompilationUnit) {
    let (model, errs) = plcc_plcopen::ladder::read(src);
    assert!(errs.is_empty(), "{errs:?}");
    let model = model.expect("model");
    let (mut unit, errs) = plcc_ladder::to_unit(&model, false);
    assert!(errs.is_empty(), "{errs:?}");
    let (direct, _) = plcc_plcopen::parse(src);
    for d in direct.declarations {
        if matches!(
            d,
            plcc_st::Declaration::Configuration(_) | plcc_st::Declaration::TypeDecl(_)
        ) {
            unit.declarations.push(d);
        }
    }
    (model, unit)
}

#[test]
fn plcopen_model_path_runs_like_the_direct_lowering() {
    let _clock = jit::clock();
    let mut report = Vec::new();
    for f in LD_FIXTURES {
        let src = read(f);
        let (direct, errs) = plcc_plcopen::parse(&src);
        assert!(errs.is_empty(), "{f}: {errs:?}");
        let (model, unit) = model_unit(&src);
        let boxes = model
            .pous
            .iter()
            .flat_map(|p| &p.routines)
            .flat_map(|r| &r.rungs)
            .filter(|g| matches!(g.elements.as_slice(), [Element::St(_)]))
            .count();
        let ctx_a = inkwell::context::Context::create();
        let ctx_b = inkwell::context::Context::create();
        let a = jit::load(&ctx_a, "direct", &jit::with_libs(direct, false)).unwrap();
        let b = jit::load(&ctx_b, "model", &jit::with_libs(unit, false))
            .unwrap_or_else(|e| panic!("{f}: {e}\n{}", plcc_st::print_unit(&model_unit(&src).1)));
        let ins = inputs(&model);
        let n = jit::differential(&a, &b, &ins, &[], 300, 7, 30)
            .unwrap_or_else(|e| panic!("{f}: {e}\ninputs {ins:?}"));
        report.push(format!("{f}: {n} comparisons, {boxes} ST-box rungs"));
    }
    eprintln!("{}", report.join("\n"));
}

/// Model JSON with the POU and routine ids, which XML has nowhere to keep.
fn comparable(p: &Project) -> serde_json::Value {
    let mut p = p.clone();
    for pou in &mut p.pous {
        pou.id = 0;
        for r in &mut pou.routines {
            r.id = 0;
        }
    }
    serde_json::to_value(&p).unwrap()
}

#[test]
fn plcopen_round_trip_is_identity() {
    let _clock = jit::clock();
    for f in LD_FIXTURES {
        let src = read(f);
        let (m1, errs) = plcc_plcopen::ladder::read(&src);
        assert!(errs.is_empty(), "{f}: {errs:?}");
        let m1 = m1.unwrap();
        let xml = plcc_plcopen::ladder::write(&m1).unwrap_or_else(|e| panic!("{f}: {e:?}"));
        let (m2, errs) = plcc_plcopen::ladder::read(&xml);
        assert!(errs.is_empty(), "{f}: written XML: {errs:?}\n{xml}");
        let m2 = m2.unwrap();
        if comparable(&m1) != comparable(&m2) {
            let a = serde_json::to_string_pretty(&comparable(&m1)).unwrap();
            let b = serde_json::to_string_pretty(&comparable(&m2)).unwrap();
            let diff: Vec<String> = a
                .lines()
                .zip(b.lines())
                .enumerate()
                .filter(|(_, (x, y))| x != y)
                .take(12)
                .map(|(i, (x, y))| format!("{i}: {x}  |  {y}"))
                .collect();
            panic!(
                "{f}: model -> XML -> model changed:\n{}\n{xml}",
                diff.join("\n")
            );
        }
        // The written XML compiles through the direct LD lowering and runs
        // like the original.
        let (u1, errs) = plcc_plcopen::parse(&src);
        assert!(errs.is_empty());
        let (mut u2, errs) = plcc_plcopen::parse(&xml);
        assert!(errs.is_empty(), "{f}: {errs:?}\n{xml}");
        for d in &u1.declarations {
            if matches!(d, plcc_st::Declaration::Configuration(_)) {
                u2.declarations.push(d.clone());
            }
        }
        let ctx_a = inkwell::context::Context::create();
        let ctx_b = inkwell::context::Context::create();
        let a = jit::load(&ctx_a, "original", &jit::with_libs(u1, false)).unwrap();
        let b = jit::load(&ctx_b, "written", &jit::with_libs(u2, false))
            .unwrap_or_else(|e| panic!("{f}: {e}\n{xml}"));
        jit::differential(&a, &b, &inputs(&m1), &[], 200, 11, 30)
            .unwrap_or_else(|e| panic!("{f}: written XML differs: {e}"));
    }
}

fn l5x_rungs() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let dir = root().join("tests/fixtures/l5x");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("l5x")))
        .collect();
    files.sort();
    for p in files {
        let src = std::fs::read_to_string(&p).unwrap();
        let doc = roxmltree::Document::parse(&src).unwrap();
        for t in doc.descendants().filter(|n| n.has_tag_name("Text")) {
            if t.parent().is_some_and(|r| r.has_tag_name("Rung")) {
                let text: String = t
                    .descendants()
                    .filter(|d| d.is_text())
                    .filter_map(|d| d.text())
                    .collect();
                out.push((p.display().to_string(), text.trim().to_string()));
            }
        }
    }
    out
}

#[test]
fn rung_text_round_trip() {
    let mut canonical = 0;
    let rungs = l5x_rungs();
    assert!(rungs.len() > 50, "{}", rungs.len());
    for (file, text) in &rungs {
        let mut ids = Ids::new();
        let model = plcc_ladder::rll::read(text, &mut ids)
            .unwrap_or_else(|e| panic!("{file}: {text}: {e:?}"));
        let written: String = model
            .iter()
            .map(|g| plcc_ladder::rll::write(g).unwrap())
            .collect();
        // Canonical text comes back unchanged; any text comes back canonical
        // and stable.
        let again: String = plcc_ladder::rll::read(&written, &mut Ids::new())
            .unwrap()
            .iter()
            .map(|g| plcc_ladder::rll::write(g).unwrap())
            .collect();
        assert_eq!(again, written, "{file}: not stable: {text}");
        let squeeze = |s: &str| s.split_whitespace().collect::<String>();
        assert_eq!(squeeze(&written), squeeze(text), "{file}: changed: {text}");
        if &written == text {
            canonical += 1;
        }
    }
    eprintln!(
        "rung text: {} rungs, {canonical} already canonical and returned byte for byte",
        rungs.len()
    );
}

/// Model JSON with every id renumbered in document order: two reads number
/// elements alike only if they meet them in the same order.
fn shape(p: &Project) -> serde_json::Value {
    let mut p = p.clone();
    p.renumber();
    serde_json::to_value(&p).unwrap()
}

const L5X_LADDER: &[&str] = &[
    "tests/fixtures/l5x/seal_in.L5X",
    "tests/fixtures/l5x/bits_branches.L5X",
    "tests/fixtures/l5x/timers_counters.L5X",
    "tests/fixtures/l5x/math.L5X",
];

/// L5X → model → L5X: the model reads back the same, and the written project
/// compiles and runs like the original.
#[test]
fn l5x_round_trip() {
    let _clock = jit::clock();
    for f in L5X_LADDER {
        let src = read(f);
        let (m1, errs) = plcc_l5x::ladder::read(&src);
        assert!(errs.iter().all(|e| e.is_warning()), "{f}: {errs:?}");
        let m1 = m1.unwrap();
        let (l5x, _warnings) =
            plcc_l5x::ladder::write(&m1).unwrap_or_else(|e| panic!("{f}: {e:?}"));
        let (m2, errs) = plcc_l5x::ladder::read(&l5x);
        assert!(errs.iter().all(|e| e.is_warning()), "{f}: {errs:?}");
        let m2 = m2.unwrap();
        assert_eq!(
            shape(&m1),
            shape(&m2),
            "{f}: L5X -> model -> L5X -> model changed"
        );
        let (u1, errs) = plcc_l5x::parse(&src);
        assert!(errs.iter().all(|e| e.is_warning()), "{f}: {errs:?}");
        let (u2, errs) = plcc_l5x::parse(&l5x);
        assert!(
            errs.iter().all(|e| e.is_warning()),
            "{f}: written L5X: {errs:?}\n{l5x}"
        );
        let ctx_a = inkwell::context::Context::create();
        let ctx_b = inkwell::context::Context::create();
        let a = jit::load(&ctx_a, "original", &jit::with_libs(u1, true)).unwrap();
        let b = jit::load(&ctx_b, "written", &jit::with_libs(u2, true))
            .unwrap_or_else(|e| panic!("{f}: {e}"));
        jit::differential(&a, &b, &inputs(&m1), &[], 200, 5, 30)
            .unwrap_or_else(|e| panic!("{f}: written L5X differs: {e}"));
    }
}
