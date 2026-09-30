// SPDX-License-Identifier: MPL-2.0

//! The ST printer over every front end: the AST lowered from each PLCopen,
//! L5X and TwinCAT fixture prints as ST that parses back to the same AST
//! (modulo spans, see `plcc_st::printer::normalized`), printing is idempotent,
//! and `plcc convert --to st` output compiles.

use plcc_st::printer::normalized;
use std::path::{Path, PathBuf};
use std::process::Command;

const PLCC: &str = env!("CARGO_BIN_EXE_plcc");

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn files(dir: &Path, exts: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    fn walk(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n != "errors") {
                    walk(&p, exts, out);
                }
            } else if p
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| exts.iter().any(|e| x.eq_ignore_ascii_case(e)))
            {
                out.push(p);
            }
        }
    }
    walk(dir, exts, &mut out);
    out.sort();
    out
}

fn round_trip(what: &str, unit: &plcc_st::CompilationUnit) -> Result<(), String> {
    let printed = plcc_st::print_unit(unit);
    let (again, errs) = plcc_st::parse(&printed);
    if !errs.is_empty() {
        return Err(format!(
            "{what}: printed ST does not parse: {}\n{printed}",
            errs.iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    let (a, b) = (normalized(unit), normalized(&again));
    if a != b {
        let a = serde_json::to_string_pretty(&a).unwrap();
        let b = serde_json::to_string_pretty(&b).unwrap();
        let first = a
            .lines()
            .zip(b.lines())
            .position(|(x, y)| x != y)
            .unwrap_or(0);
        let ctx = |s: &str| {
            s.lines()
                .skip(first.saturating_sub(12))
                .take(24)
                .collect::<Vec<_>>()
                .join("\n")
        };
        return Err(format!(
            "{what}: AST changed near line {first}\n--- lowered\n{}\n--- reparsed\n{}",
            ctx(&a),
            ctx(&b)
        ));
    }
    // Comments do not survive parsing, so compare from the reparsed unit on.
    let text2 = plcc_st::print_unit(&again);
    if plcc_st::print_unit(&plcc_st::parse(&text2).0) != text2 {
        return Err(format!("{what}: printing is not idempotent"));
    }
    Ok(())
}

fn check_all(units: Vec<(String, plcc_st::CompilationUnit)>) {
    assert!(!units.is_empty());
    let fails: Vec<String> = units
        .iter()
        .filter_map(|(w, u)| round_trip(w, u).err())
        .collect();
    assert!(fails.is_empty(), "{}", fails.join("\n\n"));
}

#[test]
fn plcopen_fixtures_round_trip() {
    let opts = plcc_plcopen::Options {
        annotate_rungs: true,
    };
    let units = files(&root().join("tests/fixtures/plcopen"), &["xml"])
        .into_iter()
        .map(|p| {
            let src = std::fs::read_to_string(&p).unwrap();
            let (u, errs) = plcc_plcopen::parse_with(&src, &opts);
            assert!(errs.is_empty(), "{}: {errs:?}", p.display());
            (p.display().to_string(), u)
        })
        .collect();
    check_all(units);
}

#[test]
fn l5x_fixtures_round_trip() {
    let opts = plcc_l5x::Options::default();
    let mut units: Vec<(String, plcc_st::CompilationUnit)> =
        files(&root().join("tests/fixtures/l5x"), &["l5x"])
            .into_iter()
            .map(|p| {
                let src = std::fs::read_to_string(&p).unwrap();
                let (u, errs) = plcc_l5x::parse_annotated(&src, &opts);
                assert!(
                    errs.iter().all(|e| e.is_warning()),
                    "{}: {errs:?}",
                    p.display()
                );
                (p.display().to_string(), u)
            })
            .collect();
    let (prelude, errs) = plcc_st::parse(&plcc_l5x::prelude());
    assert!(errs.is_empty());
    units.push(("logix prelude".into(), prelude));
    check_all(units);
}

#[test]
fn twincat_fixtures_round_trip() {
    let units = files(
        &root().join("tests/fixtures/twincat"),
        &["TcPOU", "TcDUT", "TcGVL", "TcIO"],
    )
    .into_iter()
    .filter_map(|p| {
        let src = std::fs::read_to_string(&p).unwrap();
        let (u, errs) = plcc_twincat::parse(&src);
        // Drafts/ holds a deliberately unfinished POU.
        let ok = errs.iter().all(|e| {
            matches!(
                miette::Diagnostic::severity(e),
                Some(miette::Severity::Warning)
            )
        });
        ok.then(|| (p.display().to_string(), u))
    })
    .collect();
    check_all(units);
}

/// `plcc convert --to st` of a ladder input compiles, and so does the
/// converted L5X with its prelude.
#[test]
fn converted_st_compiles() {
    let dir = std::env::temp_dir().join("plcc_convert_st_test");
    std::fs::create_dir_all(&dir).unwrap();
    for (input, extra) in [
        ("tests/fixtures/plcopen/ld_timer_counter.xml", None),
        ("tests/fixtures/plcopen/ld_jumps.xml", None),
        ("tests/fixtures/l5x/timers_counters.L5X", Some("--prelude")),
        ("tests/fixtures/l5x/seal_in.L5X", Some("--prelude")),
    ] {
        let st = dir.join(format!(
            "{}.st",
            Path::new(input).file_stem().unwrap().to_string_lossy()
        ));
        let mut cmd = Command::new(PLCC);
        cmd.arg("convert")
            .arg(root().join(input))
            .args(["--to", "st", "-o"])
            .arg(&st);
        if let Some(x) = extra {
            cmd.arg(x);
        }
        let o = cmd.output().unwrap();
        assert!(
            o.status.success(),
            "{input}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        let text = std::fs::read_to_string(&st).unwrap();
        assert!(
            text.contains("(* rung "),
            "{input}: no rung comments\n{text}"
        );
        let o = Command::new(PLCC)
            .arg("compile")
            .arg(&st)
            .arg("-o")
            .arg(dir.join("out.ll"))
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "{input}: converted ST does not compile: {}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
    }
}
