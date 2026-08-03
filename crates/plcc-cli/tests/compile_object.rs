// SPDX-License-Identifier: MPL-2.0

//! `plcc compile` all the way to a native object file, with the default stdlib.
//!
//! Every other end-to-end test either stops at `.ll` (no backend runs) or JITs at
//! `OptimizationLevel::None`. `emit_object` runs LLVM at `OptimizationLevel::Default`,
//! and that is the only path that ever saw the malformed IF/ELSIF join blocks: a
//! `merge: br label %merge` self-loop with the rest of the body appended after the
//! terminator. At -O0 it was a silent miscompile; at -O2 the optimizer never
//! terminated, so `plcc compile` hung forever on every program in the tree while the
//! whole test suite stayed green.
//!
//! So: run the real binary, emit a real object, under a wall-clock timeout.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const PLCC: &str = env!("CARGO_BIN_EXE_plcc");
const TIMEOUT: Duration = Duration::from_secs(120);

fn tmp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("plcc_compile_object_test");
    std::fs::create_dir_all(&dir).expect("failed to create temp dir");
    dir
}

/// Run `plcc compile <src> -o <out>` and kill it if it outlives `TIMEOUT`.
///
/// Returns `Err` with a description on timeout or on a non-zero exit.
fn compile_object(src: &Path, out: &Path, extra: &[&str]) -> Result<(), String> {
    let mut child = Command::new(PLCC)
        .arg("compile")
        .arg(src)
        .arg("-o")
        .arg(out)
        .args(extra)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn plcc");

    let start = Instant::now();
    loop {
        match child.try_wait().expect("failed to poll plcc") {
            Some(status) => {
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    use std::io::Read;
                    pipe.read_to_string(&mut stderr).ok();
                }
                return if status.success() {
                    Ok(())
                } else {
                    Err(format!("exited with {status}:\n{stderr}"))
                };
            }
            None => {
                if start.elapsed() > TIMEOUT {
                    child.kill().ok();
                    child.wait().ok();
                    return Err(format!(
                        "did not terminate within {}s — this is the -O2 hang",
                        TIMEOUT.as_secs()
                    ));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

fn workspace_root() -> PathBuf {
    // crates/plcc-cli -> crates -> <root>
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

fn assert_object_produced(name: &str, src: &Path, extra: &[&str]) {
    let out = tmp_dir().join(format!("{name}.o"));
    std::fs::remove_file(&out).ok();
    match compile_object(src, &out, extra) {
        Ok(()) => {}
        Err(e) => panic!("plcc compile {} failed: {e}", src.display()),
    }
    let len = std::fs::metadata(&out)
        .unwrap_or_else(|e| panic!("no object at {}: {e}", out.display()))
        .len();
    assert!(len > 0, "{} produced an empty object file", src.display());
    std::fs::remove_file(&out).ok();
}

/// The exact command the bug report used, with the default (bundled-ST) stdlib.
#[test]
fn blink_compiles_to_an_object_with_the_default_stdlib() {
    let src = workspace_root().join("demo/st-programs/blink.st");
    assert!(src.exists(), "missing fixture {}", src.display());
    assert_object_produced("blink", &src, &[]);
}

/// Every demo program, default stdlib, native object. These are the programs a user
/// actually runs; all fifteen of them hung.
#[test]
fn every_demo_program_compiles_to_an_object() {
    let dir = workspace_root().join("demo/st-programs");
    let mut sources: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "st"))
        .collect();
    sources.sort();
    assert!(
        sources.len() >= 5,
        "expected the demo programs to be present, found {}",
        sources.len()
    );
    for (i, src) in sources.iter().enumerate() {
        assert_object_produced(&format!("demo_{i}"), src, &[]);
    }
}

/// A nested IF/ELSIF as the last statement of a later ELSIF branch — the shape the
/// bundled CTUD body has, and the one that produced the self-looping join block.
/// Compiled with `--stdlib none` so this is a pure control-flow regression test.
#[test]
fn nested_elsif_tail_compiles_to_an_object() {
    let src = tmp_dir().join("nested_elsif.st");
    std::fs::write(
        &src,
        r#"
PROGRAM NestedElsif
VAR
    a : BOOL;
    b : BOOL;
    c : BOOL;
    d : BOOL;
    n : INT;
END_VAR
    IF a THEN
        n := 0;
    ELSIF b THEN
        n := 1;
    ELSIF c THEN
        IF d THEN
            n := 2;
        ELSIF a THEN
            n := 3;
        END_IF;
    END_IF;
    n := n + 1;
END_PROGRAM
"#,
    )
    .expect("write source");
    assert_object_produced("nested_elsif", &src, &["--stdlib", "none"]);
    std::fs::remove_file(&src).ok();
}
