// SPDX-License-Identifier: MPL-2.0

//! The compact lowering of Logix rungs (runs of input instructions folded
//! into one rung-condition expression, `lx__rc := (Start OR Motor) AND NOT
//! Stop;`) against the long form (one `lx__rc := lx__rc AND ...` per
//! instruction, `plcc_l5x::Options::long_rungs`), which is the lowering as
//! it was before the compact form. Both are JIT-compiled and run side by side
//! over randomized inputs on a fake clock, comparing every variable (status
//! flags included) after every scan: every L5X fixture, and generated
//! programs of random rungs.

mod jit;

use plcc_codegen::compiler::contract::VariableInfo;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn lower(name: &str, src: &str, long_rungs: bool) -> plcc_st::CompilationUnit {
    let opts = plcc_l5x::Options {
        long_rungs,
        ..Default::default()
    };
    let (unit, errs) = plcc_l5x::parse_with(src, &opts);
    let errs: Vec<_> = errs.iter().filter(|e| !e.is_warning()).collect();
    assert!(errs.is_empty(), "{name} (long_rungs {long_rungs}): {errs:?}");
    unit
}

/// Variables both programs have, but not the rung-condition temporaries
/// (which the two forms use differently) or the prelude's helper state.
fn pairs(a: &jit::Plc, b: &jit::Plc) -> Vec<(VariableInfo, VariableInfo)> {
    a.contract
        .variables
        .iter()
        .filter(|v| v.bit.is_some() || matches!(v.size, 1 | 2 | 4 | 8))
        // Addresses differ between the two programs.
        .filter(|v| {
            let t = v.iec_type.to_ascii_uppercase();
            !(t.starts_with("REF") || t.starts_with("POINTER"))
        })
        .filter(|v| {
            !v.path.split('.').any(|s| {
                let s = s.to_ascii_lowercase();
                s.starts_with("lx__rc") || s.starts_with("lx__bs") || s.starts_with("lx__bo")
            })
        })
        .filter_map(|v| b.variable(&v.path).map(|w| (v.clone(), w.clone())))
        .collect()
}

/// Run both over `scans` scans; before each, BOOL tags are set at random
/// (each with probability 1/3) and integer tags (1/8, -10..49), and the clock
/// advances 0..30 ms. Returns the number of comparisons.
fn run_alike(name: &str, a: &jit::Plc, b: &jit::Plc, scans: usize, seed: u64) -> usize {
    let pairs = pairs(a, b);
    assert!(!pairs.is_empty(), "{name}: no variables");
    let tag = |v: &VariableInfo| !v.path.split('.').any(|s| s.to_ascii_lowercase().starts_with("lx__"));
    let is_bool = |v: &VariableInfo| v.bit.is_some() || v.iec_type.eq_ignore_ascii_case("BOOL");
    let is_int = |v: &VariableInfo| {
        matches!(
            v.iec_type.to_ascii_uppercase().as_str(),
            "SINT" | "INT" | "DINT" | "LINT"
        )
    };
    let mut rng = jit::Rng::new(seed);
    let mut compared = 0;
    for scan in 0..scans {
        for (va, vb) in &pairs {
            if !tag(va) {
                continue;
            }
            if is_bool(va) && rng.below(3) == 0 {
                let x = rng.bool() as i64;
                a.set_var(va, x);
                b.set_var(vb, x);
            } else if is_int(va) && rng.below(8) == 0 {
                let x = rng.below(60) as i64 - 10;
                a.set_var(va, x);
                b.set_var(vb, x);
            }
        }
        jit::advance(rng.below(31) as i64 * jit::MS);
        a.scan();
        b.scan();
        for (va, vb) in &pairs {
            let (x, y) = (a.get_var(va), b.get_var(vb));
            assert_eq!(
                x, y,
                "{name}: scan {scan}: {} is {x:?} in the long form, {y:?} in the compact form",
                va.path
            );
            compared += 1;
        }
    }
    compared
}

fn compare(name: &str, src: &str, scans: usize, seed: u64) -> usize {
    let long = lower(name, src, true);
    let compact = lower(name, src, false);
    let ctx_a = inkwell::context::Context::create();
    let ctx_b = inkwell::context::Context::create();
    let a = jit::load(&ctx_a, "long", &jit::with_libs(long, true))
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    let b = jit::load(&ctx_b, "compact", &jit::with_libs(compact, true))
        .unwrap_or_else(|e| panic!("{name}: {e}\n{}", plcc_l5x::to_st(src, &Default::default()).0));
    run_alike(name, &a, &b, scans, seed)
}

#[test]
fn compact_rungs_run_like_the_long_form_on_every_fixture() {
    let _clock = jit::clock();
    let mut files: Vec<_> = std::fs::read_dir(root().join("tests/fixtures/l5x"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("l5x")))
        .collect();
    files.sort();
    assert!(files.len() >= 11, "{files:?}");
    let mut report = Vec::new();
    let mut folded = 0;
    for (k, p) in files.iter().enumerate() {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(p).unwrap();
        let (long_st, _) = plcc_l5x::to_st(
            &src,
            &plcc_l5x::Options {
                long_rungs: true,
                ..Default::default()
            },
        );
        let (st, _) = plcc_l5x::to_st(&src, &Default::default());
        if st != long_st {
            folded += 1;
        }
        let n = compare(&name, &src, 300, 101 + k as u64);
        report.push(format!("{name}: {n} comparisons"));
    }
    eprintln!("{}", report.join("\n"));
    // The compact form is not the long one under another name.
    assert!(folded >= 8, "{folded}");
}

/// A random rung of input instructions, outputs, one-shots, timers,
/// counters, math and branches (nested up to `depth`).
fn series(rng: &mut jit::Rng, depth: u32, out: &mut String) {
    let n = 1 + rng.below(4);
    for _ in 0..n {
        let b = |rng: &mut jit::Rng| format!("b{}", rng.below(8));
        let k = rng.below(if depth > 0 { 18 } else { 16 });
        let s = match k {
            0..=3 => format!("XIC({})", b(rng)),
            4 | 5 => format!("XIO({})", b(rng)),
            6 => format!("GRT(n{},{})", rng.below(3), rng.below(30)),
            7 => format!("LIM({},n{},{})", rng.below(30), rng.below(3), rng.below(30)),
            8 => format!("EQU(n{},{})", rng.below(3), rng.below(5)),
            9 => format!("OTE({})", b(rng)),
            10 => format!(
                "{}({})",
                if rng.bool() { "OTL" } else { "OTU" },
                b(rng)
            ),
            11 => format!("ONS(os{})", rng.below(4)),
            12 => match rng.below(3) {
                0 => format!("TON(t{},40,0)", rng.below(2)),
                1 => format!("TOF(t{},40,0)", rng.below(2)),
                _ => format!("XIC(t{}.DN)", rng.below(2)),
            },
            13 => match rng.below(3) {
                0 => "CTU(c0,5,0)".to_string(),
                1 => "XIC(c0.DN)".to_string(),
                _ => "RES(c0)".to_string(),
            },
            14 => format!("ADD(n{},1,n{})", rng.below(3), rng.below(3)),
            15 => format!("OSR(os{},b{})", rng.below(4), rng.below(8)),
            _ => {
                let legs = 2 + rng.below(2);
                let mut t = String::from("[");
                for l in 0..legs {
                    if l > 0 {
                        t.push(',');
                    }
                    series(rng, depth - 1, &mut t);
                    t.push(' ');
                }
                t.push(']');
                t
            }
        };
        out.push_str(&s);
    }
}

fn generated(seed: u64) -> String {
    let mut rng = jit::Rng::new(seed);
    let mut tags = String::new();
    let mut tag = |n: String, t: &str| {
        tags.push_str(&format!("<Tag Name=\"{n}\" TagType=\"Base\" DataType=\"{t}\"/>\n"));
    };
    for i in 0..8 {
        tag(format!("b{i}"), "BOOL");
    }
    for i in 0..4 {
        tag(format!("os{i}"), "BOOL");
    }
    for i in 0..3 {
        tag(format!("n{i}"), "DINT");
    }
    tag("t0".into(), "TIMER");
    tag("t1".into(), "TIMER");
    tag("c0".into(), "COUNTER");
    let mut rungs = String::new();
    for k in 0..(4 + rng.below(6)) {
        let mut r = String::new();
        series(&mut rng, 2, &mut r);
        rungs.push_str(&format!(
            "<Rung Number=\"{k}\" Type=\"N\"><Text><![CDATA[{r};]]></Text></Rung>\n"
        ));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<RSLogix5000Content SchemaRevision="1.0" SoftwareRevision="33.01" TargetName="G" TargetType="Controller">
<Controller Use="Target" Name="G" ProcessorType="1769-L33ER" MajorRev="33" MinorRev="11">
<DataTypes/><Modules/><AddOnInstructionDefinitions/>
<Tags>
{tags}</Tags>
<Programs><Program Name="P" MainRoutineName="R"><Tags/><Routines>
<Routine Name="R" Type="RLL"><RLLContent>
{rungs}</RLLContent></Routine></Routines></Program></Programs>
<Tasks><Task Name="T" Type="CONTINUOUS" Priority="10"><ScheduledPrograms><ScheduledProgram Name="P"/></ScheduledPrograms></Task></Tasks>
</Controller>
</RSLogix5000Content>
"#
    )
}

#[test]
fn compact_rungs_run_like_the_long_form_on_generated_programs() {
    let _clock = jit::clock();
    let mut total = 0;
    for seed in 1..=40u64 {
        let src = generated(seed);
        total += compare(&format!("generated program {seed}\n{src}"), &src, 150, seed);
    }
    eprintln!("40 generated programs: {total} comparisons");
}
