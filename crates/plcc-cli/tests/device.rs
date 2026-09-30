// SPDX-License-Identifier: MPL-2.0

//! `plcc device check|list` and `plcc compile --device` (docs/device-manifest.md).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PLCC: &str = env!("CARGO_BIN_EXE_plcc");

fn plcc(args: &[&str]) -> Output {
    Command::new(PLCC).args(args).output().expect("spawn plcc")
}

fn text(o: &Output) -> String {
    // miette wraps long lines; undo it for matching.
    let raw = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    raw.lines()
        .map(|l| l.trim_start_matches([' ', '│', '×']).trim_end())
        .collect::<Vec<_>>()
        .join(" ")
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("plcc_device_{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

fn repo(p: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(p)
        .display()
        .to_string()
}

const PROGRAM: &str = "PROGRAM Main\nVAR\n    x AT %IX0.0 : BOOL;\n    y AT %QX0.4 : BOOL;\n    r AT %MD1 : REAL;\nEND_VAR\n    y := x;\n    r := r * 1.5 + 0.25;\nEND_PROGRAM\n";

fn program() -> String {
    let p = tmp("main.st");
    std::fs::write(&p, PROGRAM).unwrap();
    p.display().to_string()
}

#[test]
fn list_shows_the_catalog() {
    let o = plcc(&["device", "list"]);
    assert!(o.status.success(), "{}", text(&o));
    let t = text(&o);
    assert!(t.contains("arduino-opta") && t.contains("simulator"), "{t}");
}

#[test]
fn list_falls_back_to_the_built_in_copies() {
    let o = Command::new(PLCC)
        .args(["device", "list"])
        .env("PLCC_DEVICES", tmp("no-such-dir"))
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", text(&o));
    let t = text(&o);
    assert!(t.contains("built-in copies only") && t.contains("arduino-opta"), "{t}");
}

#[test]
fn check_expands_to_json() {
    let o = plcc(&["device", "check", "--json", &repo("crates/plcc-device/builtin/arduino-opta.toml")]);
    assert!(o.status.success(), "{}", text(&o));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["device"]["id"], "arduino-opta");
    assert_eq!(v["io"].as_array().unwrap().len(), 54);
    assert_eq!(v["io"][0]["address"], "%IX0.0");
}

#[test]
fn check_reports_line_and_column() {
    let f = tmp("bad.toml");
    let src = std::fs::read_to_string(repo("crates/plcc-device/builtin/simulator.toml"))
        .unwrap()
        .replace("address = \"%IX0.{n}\"", "address = \"%QX0.{n}\"");
    std::fs::write(&f, src).unwrap();
    let o = plcc(&["device", "check", f.to_str().unwrap()]);
    assert!(!o.status.success());
    let t = text(&o);
    assert!(t.contains("bad.toml:30:11: error: io[0].address"), "{t}");
    assert!(t.contains("points live in %I"), "{t}");
}

#[test]
fn compile_takes_target_cpu_and_image_from_the_device() {
    let h = tmp("opta.h");
    let j = tmp("opta.json");
    let o = plcc(&[
        "compile", &program(), "-o", tmp("opta.o").to_str().unwrap(),
        "--device", "arduino-opta",
        "--emit-header", h.to_str().unwrap(),
        "--emit-symbols", j.to_str().unwrap(),
    ]);
    assert!(o.status.success(), "{}", text(&o));
    let header = std::fs::read_to_string(&h).unwrap();
    for line in [
        "#define PLCC_TARGET_TRIPLE \"thumbv7em-none-eabi\"",
        "#define PLCC_TARGET_CPU \"cortex-m7\"",
        "#define PLCC_TARGET_FEATURES \"+fp-armv8d16\"",
        "#define PLCC_DEVICE_ID \"arduino-opta\"",
        "#define PLCC_DEVICE_MANIFEST_VERSION 1u",
        "#define PLCC_IMAGE_I_SIZE 18u",
        "#define PLCC_IMAGE_Q_SIZE 1u",
        "#define PLCC_IMAGE_M_SIZE 64u",
    ] {
        assert!(header.contains(line), "missing {line}");
    }
    let sym: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&j).unwrap()).unwrap();
    assert_eq!(sym["device"]["id"], "arduino-opta");
    assert_eq!(sym["cpu"], "cortex-m7");
}

#[test]
fn explicit_flags_override_the_device() {
    let h = tmp("override.h");
    let o = plcc(&[
        "compile", &program(), "-o", tmp("override.o").to_str().unwrap(),
        "--device", "arduino-opta",
        "--image-size", "M=128",
        "--cpu", "cortex-m4",
        "--target", "thumbv7em-none-eabihf", "--float-abi", "hard",
        "--emit-header", h.to_str().unwrap(),
    ]);
    assert!(o.status.success(), "{}", text(&o));
    let header = std::fs::read_to_string(&h).unwrap();
    assert!(header.contains("#define PLCC_IMAGE_M_SIZE 128u"));
    assert!(header.contains("#define PLCC_IMAGE_I_SIZE 18u"));
    assert!(header.contains("#define PLCC_TARGET_CPU \"cortex-m4\""));
    assert!(header.contains("#define PLCC_TARGET_TRIPLE \"thumbv7em-none-eabihf\""));
    // A hard-float triple with the manifest's softfp ABI is a contradiction.
    let o = plcc(&[
        "compile", &program(), "-o", tmp("x.o").to_str().unwrap(),
        "--device", "arduino-opta", "--target", "thumbv7em-none-eabihf",
    ]);
    assert!(!o.status.success());
    assert!(text(&o).contains("is a hard-float triple"), "{}", text(&o));
}

#[test]
fn device_file_and_unknown_ids() {
    let o = plcc(&[
        "compile", &program(), "-o", tmp("host.ll").to_str().unwrap(),
        "--device", &repo("crates/plcc-device/tests/data/expressions.toml"),
    ]);
    assert!(o.status.success(), "{}", text(&o));
    let o = plcc(&["compile", &program(), "-o", tmp("y.o").to_str().unwrap(), "--device", "no-such-board"]);
    assert!(!o.status.success());
    let t = text(&o);
    assert!(t.contains("`no-such-board` is neither a file nor a catalog device") && t.contains("arduino-opta"), "{t}");
}

#[test]
fn image_too_small_for_the_program_is_an_error() {
    // expressions.toml has a 4-byte %I area; %ID1 needs 8.
    let st = tmp("big.st");
    std::fs::write(&st, "PROGRAM P VAR d AT %ID1 : DINT; END_VAR d := d + 1; END_PROGRAM\n").unwrap();
    let o = plcc(&[
        "compile", st.to_str().unwrap(), "-o", tmp("big.ll").to_str().unwrap(),
        "--device", &repo("crates/plcc-device/tests/data/expressions.toml"),
    ]);
    assert!(!o.status.success(), "{}", text(&o));
}
