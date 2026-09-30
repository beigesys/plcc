// SPDX-License-Identifier: MPL-2.0

//! `plcc convert` between ladder notations, end to end through the binary:
//! every output is valid input for `plcc compile`, and dialect differences
//! are printed as warnings.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PLCC: &str = env!("CARGO_BIN_EXE_plcc");

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn out_dir() -> PathBuf {
    let d = std::env::temp_dir().join("plcc_convert_ladder_test");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run(args: &[&std::ffi::OsStr]) -> Output {
    Command::new(PLCC).args(args).output().expect("spawn plcc")
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn convert(input: &Path, to: &str, out: &Path, extra: &[&str]) -> Output {
    let mut args: Vec<&std::ffi::OsStr> = vec![
        "convert".as_ref(),
        input.as_os_str(),
        "--to".as_ref(),
        to.as_ref(),
        "-o".as_ref(),
        out.as_os_str(),
    ];
    args.extend(extra.iter().map(|e| std::ffi::OsStr::new(*e)));
    let o = run(&args);
    assert!(
        o.status.success(),
        "convert {input:?} --to {to}: {}",
        text(&o)
    );
    o
}

fn compiles(input: &Path) {
    let o = run(&[
        "compile".as_ref(),
        input.as_os_str(),
        "-o".as_ref(),
        out_dir().join("out.ll").as_os_str(),
    ]);
    assert!(
        o.status.success(),
        "{input:?} does not compile: {}",
        text(&o)
    );
}

#[test]
fn plcopen_to_json_to_l5x_and_back() {
    let d = out_dir();
    let xml = root().join("tests/fixtures/plcopen/ld_timer_counter.xml");
    let json = d.join("tc.json");
    convert(&xml, "ladder-json", &json, &[]);
    let model = std::fs::read_to_string(&json).unwrap();
    assert!(model.contains("\"dialect\": \"iec\""), "{model}");
    assert!(model.contains("\"power_in\": \"IN\""), "{model}");
    // IEC -> Logix: warnings name the elements that differ.
    let l5x = d.join("tc.L5X");
    let o = convert(&json, "l5x", &l5x, &[]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("warning:") && err.contains("TON T1"), "{err}");
    compiles(&l5x);
    // Logix -> IEC.
    let back = d.join("tc_back.xml");
    convert(&l5x, "plcopen", &back, &[]);
    compiles(&back);
    // The model in the other dialect.
    let logix_json = d.join("tc_logix.json");
    convert(&json, "ladder-json", &logix_json, &["--dialect", "logix"]);
    let m = std::fs::read_to_string(&logix_json).unwrap();
    assert!(m.contains("\"dialect\": \"logix\""), "{m}");
}

#[test]
fn st_to_plcopen_and_l5x() {
    let d = out_dir();
    let st = root().join("tests/fixtures/programs/stdlib_blinker.st");
    let xml = d.join("blinker.xml");
    convert(&st, "plcopen", &xml, &[]);
    compiles(&xml);
    let json = d.join("blinker.json");
    convert(&st, "ladder-json", &json, &[]);
    let back = d.join("blinker.st");
    convert(&json, "st", &back, &[]);
    compiles(&back);
}

#[test]
fn l5x_to_l5x_keeps_rung_text() {
    let d = out_dir();
    let src = root().join("tests/fixtures/l5x/seal_in.L5X");
    let out = d.join("seal.L5X");
    convert(&src, "l5x", &out, &[]);
    let t = std::fs::read_to_string(&out).unwrap();
    assert!(
        t.contains("[XIC(Start) ,XIC(Motor) ]XIO(Stop)OTE(Motor);"),
        "{t}"
    );
    compiles(&out);
}
