// SPDX-License-Identifier: MPL-2.0

//! End-to-end test for the JIT PRINT path.
//!
//! `crates/plcc-codegen/tests/print_stmt.rs` covers PRINT at the IR level, but
//! stops short of execution. The remaining risk lives in `plcc sim`, which maps
//! the `plcc_print` symbol onto a Rust `extern "C"` function by casting the
//! function item to an address. Nothing verified that the address was real, so
//! a bad cast would have produced silently missing output rather than a failure.
//!
//! These tests drive the actual binary so the mapping is exercised for real.

use std::process::Command;

/// Path to the `plcc` binary cargo built for this test.
const PLCC: &str = env!("CARGO_BIN_EXE_plcc");

fn sim(source: &str, name: &str) -> (String, String, bool) {
    let dir = std::env::temp_dir().join("plcc_cli_test");
    std::fs::create_dir_all(&dir).expect("failed to create temp dir");
    let path = dir.join(format!("{name}.st"));
    std::fs::write(&path, source).expect("failed to write ST source");

    let out = Command::new(PLCC)
        .args(["sim", path.to_str().unwrap()])
        .args(["--scans", "1", "--interval-ms", "0"])
        .output()
        .expect("failed to run plcc sim");

    std::fs::remove_file(&path).ok();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.success(),
    )
}

#[test]
fn print_reaches_the_host_through_the_jit() {
    let source = r#"
PROGRAM PrintSim
VAR
    x : INT := 0;
END_VAR
    PRINT('plcc print works');
    x := x + 1;
END_PROGRAM
"#;
    let (stdout, stderr, ok) = sim(source, "print_reaches_host");
    let combined = format!("{stdout}{stderr}");

    assert!(
        ok,
        "plcc sim exited non-zero\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        combined.contains("plcc print works"),
        "PRINT output never reached the host — the plcc_print symbol mapping is \
         broken.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        combined.contains("[PLC]"),
        "expected the [PLC] prefix from plcc_print_impl\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

/// End-to-end smoke test for the clock hook through `plcc sim`.
///
/// `MONOTONIC_NS()` compiles to a call to the external `plcc_monotonic_ns`. If the
/// JIT cannot resolve that symbol it aborts the process before any user code runs,
/// so reaching the PRINT at all proves the symbol was resolved and called.
#[test]
fn monotonic_clock_resolves_through_the_jit() {
    let source = r#"
PROGRAM ClockSim
VAR
    now : LINT;
    prev : LINT;
END_VAR
    prev := now;
    now := MONOTONIC_NS();
    IF now >= prev THEN
        PRINT('clock ok');
    ELSE
        PRINT('clock went backwards');
    END_IF;
END_PROGRAM
"#;
    let (stdout, stderr, ok) = sim(source, "monotonic_clock_resolves");
    let combined = format!("{stdout}{stderr}");
    assert!(
        ok,
        "plcc sim exited non-zero — plcc_monotonic_ns was probably \
         unresolved\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        combined.contains("clock ok"),
        "expected the monotonic branch to be taken\n{combined}"
    );
    assert!(
        !combined.contains("clock went backwards"),
        "the host clock decreased between scans\n{combined}"
    );
}

#[test]
fn print_runs_once_per_scan() {
    let source = r#"
PROGRAM PrintScans
VAR
    x : INT := 0;
END_VAR
    PRINT('tick');
    x := x + 1;
END_PROGRAM
"#;
    let dir = std::env::temp_dir().join("plcc_cli_test");
    std::fs::create_dir_all(&dir).expect("failed to create temp dir");
    let path = dir.join("print_runs_once_per_scan.st");
    std::fs::write(&path, source).expect("failed to write ST source");

    let out = Command::new(PLCC)
        .args(["sim", path.to_str().unwrap()])
        .args(["--scans", "3", "--interval-ms", "0"])
        .output()
        .expect("failed to run plcc sim");
    std::fs::remove_file(&path).ok();

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let ticks = combined.matches("tick").count();
    assert_eq!(
        ticks, 3,
        "PRINT should fire once per scan cycle, saw {ticks}\n{combined}"
    );
}
