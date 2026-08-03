// SPDX-License-Identifier: MPL-2.0

use glob::glob;
use std::path::Path;

#[test]
fn parse_oscat_basic() {
    let pattern = "../../tests/external/oscat/*.EXP";
    let entries: Vec<_> = glob(pattern)
        .expect("failed to read glob pattern")
        .filter_map(Result::ok)
        .collect();

    if entries.is_empty() {
        eprintln!("SKIP: no OSCAT files found (clone into tests/external/oscat)");
        return;
    }

    let total = entries.len();
    let mut passed = 0;
    let mut failed_files = Vec::new();

    for path in &entries {
        let source = match std::fs::read(path) {
            Ok(bytes) => match String::from_utf8(bytes.clone()) {
                Ok(s) => s,
                Err(_) => bytes.iter().map(|&b| b as char).collect(),
            },
            Err(e) => {
                failed_files.push((path.display().to_string(), format!("read error: {e}")));
                continue;
            }
        };
        let (unit, errors) = plcc_st::parse(&source);
        if errors.is_empty() && !unit.declarations.is_empty() {
            passed += 1;
        } else {
            let err_summary = errors
                .iter()
                .take(3)
                .map(|e| format!("{e}"))
                .collect::<Vec<_>>()
                .join("; ");
            failed_files.push((path.display().to_string(), err_summary));
        }
    }

    let fail_rate = (total - passed) as f64 / total as f64 * 100.0;
    eprintln!("\nOSCAT parse results: {passed}/{total} passed ({fail_rate:.1}% failure rate)");

    if !failed_files.is_empty() {
        eprintln!("\nFailed files ({}):", failed_files.len());
        for (file, err) in &failed_files {
            let name = Path::new(file)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            eprintln!("  {name}: {err}");
        }
    }

    // Phase 1 exit criteria: <5% failure rate
    assert!(
        fail_rate < 5.0,
        "OSCAT failure rate {fail_rate:.1}% exceeds 5% threshold"
    );
}
