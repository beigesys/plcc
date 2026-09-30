// SPDX-License-Identifier: MPL-2.0

//! The ST printer round trip: parse → print → parse gives the same AST
//! (modulo spans, see `printer::normalized`), and printing is idempotent,
//! over every `.st` fixture and every OSCAT file (each parsed on its own).

use plcc_st::printer::normalized;
use std::path::{Path, PathBuf};

fn read(path: &Path) -> String {
    let bytes = std::fs::read(path).expect("read");
    match String::from_utf8(bytes.clone()) {
        Ok(s) => s,
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

/// Round-trip one unit; describe the failure, if any.
pub fn check_unit(what: &str, unit: &plcc_st::CompilationUnit) -> Option<String> {
    let printed = plcc_st::print_unit(unit);
    let (again, errs2) = plcc_st::parse(&printed);
    if !errs2.is_empty() {
        return Some(format!(
            "{what}: printed ST does not parse: {}\n{printed}",
            errs2
                .iter()
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
                .skip(first.saturating_sub(10))
                .take(20)
                .collect::<Vec<_>>()
                .join("\n")
        };
        return Some(format!(
            "{what}: AST changed in the round trip near line {first}:\n--- before\n{}\n--- after\n{}",
            ctx(&a),
            ctx(&b)
        ));
    }
    if plcc_st::print_unit(&again) != printed {
        return Some(format!("{what}: printing is not idempotent"));
    }
    None
}

fn check(path: &Path) -> Option<String> {
    let src = read(path);
    let (unit, errs) = plcc_st::parse(&src);
    if !errs.is_empty() {
        return None; // only valid inputs have a defined round trip
    }
    check_unit(&path.display().to_string(), &unit)
}

fn files(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            files(&p, exts, out);
        } else if p
            .extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| exts.iter().any(|e| x.eq_ignore_ascii_case(e)))
        {
            out.push(p);
        }
    }
}

fn run(dir: &str, exts: &[&str]) -> (usize, Vec<String>) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(dir);
    let mut paths = Vec::new();
    files(&root, exts, &mut paths);
    paths.sort();
    let fails: Vec<String> = paths.iter().filter_map(|p| check(p)).collect();
    (paths.len(), fails)
}

#[test]
fn fixtures_round_trip() {
    let (n, fails) = run("tests/fixtures", &["st"]);
    assert!(n > 20, "found only {n} fixtures");
    assert!(
        fails.is_empty(),
        "{} of {n} failed:\n{}",
        fails.len(),
        fails.join("\n\n")
    );
}

#[test]
fn oscat_round_trips() {
    let (n, fails) = run("tests/external/oscat", &["exp", "st"]);
    if n == 0 {
        eprintln!("SKIP: no OSCAT files (clone into tests/external/oscat)");
        return;
    }
    eprintln!("OSCAT print round trip: {} of {n} files", n - fails.len());
    assert!(
        fails.is_empty(),
        "{} of {n} failed:\n{}",
        fails.len(),
        fails
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n")
    );
}
