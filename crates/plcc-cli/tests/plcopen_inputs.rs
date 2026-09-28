// SPDX-License-Identifier: MPL-2.0

//! `plcc parse/check/compile` on PLCopen XML (LD/FBD/ST) inputs, alone and mixed
//! with `.st` files.

use std::path::PathBuf;
use std::process::{Command, Output};

const PLCC: &str = env!("CARGO_BIN_EXE_plcc");

fn fixture(p: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(p)
}

fn out_dir() -> PathBuf {
    let d = std::env::temp_dir().join("plcc_plcopen_cli_test");
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
fn parse_dump_ast_shows_the_lowered_ladder() {
    let f = fixture("plcopen/ld_seal_in.xml");
    let o = plcc(&["parse".as_ref(), f.as_os_str(), "--dump-ast".as_ref()]);
    assert!(o.status.success(), "{}", text(&o));
    let json: serde_json::Value = serde_json::from_slice(&o.stdout).expect("AST JSON");
    let decls = json["declarations"].as_array().unwrap();
    assert_eq!(decls[0]["Program"]["name"]["name"], "SealIn");
    assert!(decls[1].get("Configuration").is_some());
    let body = serde_json::to_string(&decls[0]["Program"]["body"]).unwrap();
    assert!(
        body.contains("\"Or\"") && body.contains("\"Not\""),
        "{body}"
    );
}

#[test]
fn check_accepts_ladder() {
    let f = fixture("plcopen/ld_seal_in.xml");
    let o = plcc(&["check".as_ref(), f.as_os_str()]);
    assert!(o.status.success(), "{}", text(&o));
}

#[test]
fn compile_mixes_xml_and_st() {
    let out = out_dir().join("mixed.ll");
    let xml = fixture("plcopen/ld_coils_edges.xml");
    let st = fixture("programs/blink.st");
    let o = plcc(&[
        "compile".as_ref(),
        xml.as_os_str(),
        st.as_os_str(),
        "-o".as_ref(),
        out.as_os_str(),
    ]);
    assert!(o.status.success(), "{}", text(&o));
    let ir = std::fs::read_to_string(&out).unwrap();
    assert!(ir.contains("@coils_scan"), "the LD program");
    assert!(
        ir.contains("r_trig_scan"),
        "hidden edge detectors use the stdlib R_TRIG"
    );
}

#[test]
fn cross_compiles_ladder_for_cortex_m_with_header() {
    let dir = out_dir();
    let obj = dir.join("seal_in.o");
    let hdr = dir.join("seal_in.h");
    let f = fixture("plcopen/ld_seal_in.xml");
    let o = plcc(&[
        "compile".as_ref(),
        f.as_os_str(),
        "-o".as_ref(),
        obj.as_os_str(),
        "--target".as_ref(),
        "thumbv7em-none-eabi".as_ref(),
        "--emit-header".as_ref(),
        hdr.as_os_str(),
    ]);
    assert!(o.status.success(), "{}", text(&o));
    let bytes = std::fs::read(&obj).unwrap();
    assert_eq!(&bytes[..4], b"\x7fELF");
    assert_eq!(bytes[4], 1, "32-bit ELF");
    let h = std::fs::read_to_string(&hdr).unwrap();
    assert!(h.contains("SealIn.Motor AT %QX0.0"), "{h}");
    assert!(
        h.contains("PLCC_TASK_MAINTASK_INTERVAL_NS 20000000"),
        "task from the XML configuration"
    );
}

#[test]
fn xml_detected_by_root_element_without_extension() {
    let dir = out_dir();
    let copy = dir.join("ladder_no_ext");
    std::fs::copy(fixture("plcopen/ld_seal_in.xml"), &copy).unwrap();
    let o = plcc(&["check".as_ref(), copy.as_os_str()]);
    assert!(o.status.success(), "{}", text(&o));
}

#[test]
fn errors_render_against_the_xml_file() {
    let f = fixture("plcopen/errors/ld_bad_wiring.xml");
    let out = out_dir().join("bad.ll");
    let o = plcc(&[
        "compile".as_ref(),
        f.as_os_str(),
        "-o".as_ref(),
        out.as_os_str(),
    ]);
    assert!(!o.status.success());
    let t = text(&o);
    assert!(t.contains("ld_bad_wiring.xml"), "{t}");
    assert!(t.contains("connection refers to unknown localId 99"), "{t}");

    let f = fixture("plcopen/errors/sfc_body.xml");
    let o = plcc(&["check".as_ref(), f.as_os_str()]);
    assert!(!o.status.success());
    assert!(text(&o).contains("SFC bodies are not supported yet"));
}
