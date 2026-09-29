// SPDX-License-Identifier: MPL-2.0

//! `plcc parse/check/compile` on Rockwell L5X inputs.

use std::path::PathBuf;
use std::process::{Command, Output};

const PLCC: &str = env!("CARGO_BIN_EXE_plcc");

fn fixture(p: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/l5x")
        .join(p)
}

fn out_dir() -> PathBuf {
    let d = std::env::temp_dir().join("plcc_l5x_cli_test");
    std::fs::create_dir_all(&d).expect("temp dir");
    d
}

fn plcc(args: &[&std::ffi::OsStr]) -> Output {
    Command::new(PLCC).args(args).output().expect("spawn plcc")
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn check_and_dump_st() {
    let f = fixture("seal_in.L5X");
    let o = plcc(&["check".as_ref(), f.as_os_str()]);
    assert!(o.status.success(), "{}", text(&o));
    let o = plcc(&["parse".as_ref(), f.as_os_str(), "--dump-st".as_ref()]);
    assert!(o.status.success(), "{}", text(&o));
    let st = String::from_utf8_lossy(&o.stdout);
    assert!(st.contains("FUNCTION_BLOCK lx__P_MainProgram"), "{st}");
    assert!(st.contains("Motor := lx__rc;"), "{st}");
}

#[test]
fn compile_cortex_m_object_with_header() {
    let out = out_dir().join("tasks.o");
    let header = out_dir().join("tasks.h");
    let f = fixture("tasks.L5X");
    let o = plcc(&[
        "compile".as_ref(),
        f.as_os_str(),
        "-o".as_ref(),
        out.as_os_str(),
        "--target".as_ref(),
        "thumbv7em-none-eabi".as_ref(),
        "--emit-header".as_ref(),
        header.as_os_str(),
    ]);
    assert!(o.status.success(), "{}", text(&o));
    // Warnings (inhibited task, unscheduled program) do not stop the build.
    assert!(text(&o).contains("inhibited"), "{}", text(&o));
    let h = std::fs::read_to_string(&header).expect("header");
    assert!(h.contains("PLCC_TASK_FASTTASK"), "{h}");
}

#[test]
fn errors_render_against_the_l5x() {
    let f = fixture("errors/bad_rungs.L5X");
    let o = plcc(&["check".as_ref(), f.as_os_str()]);
    assert!(!o.status.success());
    let t = text(&o);
    assert!(t.contains("bad_rungs.L5X"), "{t}");
    assert!(t.contains("unknown tag `Missing`"), "{t}");
}
