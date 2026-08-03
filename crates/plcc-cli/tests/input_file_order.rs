// SPDX-License-Identifier: MPL-2.0

//! `plcc` merges its input files in argument order, so the order of `.st` files on
//! the command line must not change whether — or how — a program compiles.
//!
//! It used to. `compile_fb_call` resolves the callee with
//! `module.get_function("<fb>_scan")`, and that `FunctionValue` was created by the
//! callee's own `compile_function_block`. Passing the caller's file first therefore
//! failed outright:
//!
//! ```text
//! $ plcc compile tests/external/oscat/DRIVER_4.EXP tests/external/oscat/DRIVER_1.EXP
//! Codegen error: LLVM error: FB scan function 'driver_1_scan' not found
//! $ plcc compile tests/external/oscat/DRIVER_1.EXP tests/external/oscat/DRIVER_4.EXP
//! Compiled 2 file(s) to ...
//! ```
//!
//! The prototypes are now declared during layout, before any body is compiled.
//!
//! These drive the real binary. `plcc sim` JITs and runs the program, and the ST
//! below only prints its success string when the accumulated value is exactly right,
//! so this asserts execution rather than "it compiled" — a dropped FB scan call
//! still produces a valid object file.

use std::path::{Path, PathBuf};
use std::process::Command;

const PLCC: &str = env!("CARGO_BIN_EXE_plcc");

/// An FB that accumulates across scans, in its own file.
const INNER_ST: &str = r#"
FUNCTION_BLOCK COUNTER
VAR_INPUT
    STEP : DINT;
END_VAR
VAR_OUTPUT
    TOTAL : DINT;
END_VAR
VAR
    ACC : DINT := 0;
END_VAR
    ACC := ACC + STEP;
    TOTAL := ACC;
END_FUNCTION_BLOCK
"#;

/// A wrapper FB plus the PROGRAM, in a second file. The PROGRAM prints its verdict
/// so the value is observable from outside the process.
const OUTER_ST: &str = r#"
FUNCTION_BLOCK OUTER
VAR_INPUT
    STEP : DINT;
END_VAR
VAR_OUTPUT
    OUT : DINT;
END_VAR
VAR
    c : COUNTER;
END_VAR
    c(STEP := STEP);
    OUT := c.TOTAL;
END_FUNCTION_BLOCK

PROGRAM OrderProg
VAR
    r : DINT;
    o : OUTER;
END_VAR
    o(STEP := 3);
    r := o.OUT;
    IF r = 12 THEN
        PRINT('accumulated exactly 12');
    END_IF;
    IF r > 12 THEN
        PRINT('accumulated past 12');
    END_IF;
END_PROGRAM
"#;

fn tmp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("plcc_input_file_order");
    std::fs::create_dir_all(&dir).expect("failed to create temp dir");
    dir
}

/// Run `plcc sim <files...> --scans 4` and return combined stdout+stderr and success.
fn sim(files: &[&Path]) -> (String, bool) {
    let out = Command::new(PLCC)
        .arg("sim")
        .args(files)
        .args(["--scans", "4", "--interval-ms", "0"])
        .output()
        .expect("failed to run plcc sim");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (combined, out.status.success())
}

#[test]
fn either_input_file_order_produces_the_same_run() {
    let dir = tmp_dir();
    let inner = dir.join("inner.st");
    let outer = dir.join("outer.st");
    std::fs::write(&inner, INNER_ST).expect("write inner.st");
    std::fs::write(&outer, OUTER_ST).expect("write outer.st");

    for (label, files) in [
        ("caller first", vec![outer.as_path(), inner.as_path()]),
        ("callee first", vec![inner.as_path(), outer.as_path()]),
    ] {
        let (combined, ok) = sim(&files);
        assert!(ok, "{label}: plcc sim exited non-zero\n{combined}");
        assert_eq!(
            combined.matches("accumulated exactly 12").count(),
            1,
            "{label}: 4 scans of STEP := 3 must pass through exactly 12 once — \
             a dropped nested FB scan call leaves it at 0\n{combined}"
        );
        assert!(
            !combined.contains("accumulated past 12"),
            "{label}: the nested FB ran more often than once per scan\n{combined}"
        );
    }
}

/// The pair from the original report. OSCAT is fetched into the gitignored
/// `tests/external/`, so skip when it is not present.
#[test]
fn oscat_driver_pair_compiles_in_either_order() {
    let d1 = Path::new("../../tests/external/oscat/DRIVER_1.EXP");
    let d4 = Path::new("../../tests/external/oscat/DRIVER_4.EXP");
    if !d1.exists() || !d4.exists() {
        eprintln!("SKIP: no OSCAT corpus (clone into tests/external/oscat)");
        return;
    }
    let dir = tmp_dir();
    for (label, files) in [("DRIVER_4 first", [d4, d1]), ("DRIVER_1 first", [d1, d4])] {
        let out = dir.join(format!("{}.o", label.replace(' ', "_")));
        let result = Command::new(PLCC)
            .arg("compile")
            .args(files)
            .arg("-o")
            .arg(&out)
            .output()
            .expect("failed to run plcc compile");
        assert!(
            result.status.success(),
            "{label}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}
