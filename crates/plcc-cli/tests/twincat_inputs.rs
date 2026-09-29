// SPDX-License-Identifier: MPL-2.0

//! `plcc parse/check/compile` on TwinCAT 3 inputs: object files, a `.plcproj`,
//! a project directory, mixed with `.st` files.

use std::path::PathBuf;
use std::process::{Command, Output};

const PLCC: &str = env!("CARGO_BIN_EXE_plcc");

fn fixture(p: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(p)
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
fn parse_a_project_file_and_a_directory() {
    for input in [
        fixture("twincat/Demo/Demo.plcproj"),
        fixture("twincat/Demo"),
    ] {
        let o = plcc(&["parse".as_ref(), input.as_os_str()]);
        assert!(o.status.success(), "{}", text(&o));
        assert!(
            text(&o).contains("OK: 7 declaration(s) parsed"),
            "{}",
            text(&o)
        );
    }
}

#[test]
fn parse_a_single_object_dumps_its_ast() {
    let o = plcc(&[
        "parse".as_ref(),
        fixture("twincat/Demo/POUs/FB_Counter.TcPOU").as_os_str(),
        "--dump-ast".as_ref(),
    ]);
    assert!(o.status.success(), "{}", text(&o));
    let json: serde_json::Value = serde_json::from_slice(&o.stdout).expect("AST JSON");
    let fb = &json["declarations"][0]["FunctionBlock"];
    assert_eq!(fb["name"]["name"], "FB_Counter");
    assert_eq!(fb["properties"][0]["name"]["name"], "Step");
    assert_eq!(fb["actions"][0]["name"]["name"], "Reset");
}

#[test]
fn graphical_bodies_are_named_in_the_error() {
    let f = fixture("twincat/errors/FB_Ladder.TcPOU");
    let o = plcc(&["parse".as_ref(), f.as_os_str()]);
    assert!(!o.status.success());
    let t = text(&o);
    assert!(t.contains("function block `FB_Ladder`"), "{t}");
    assert!(t.contains("not yet supported"), "{t}");
    assert!(t.contains("FB_Ladder.TcPOU:10:7"), "{t}");
    assert!(t.contains("SFC transition `T_Done`"), "{t}");
}

#[test]
fn syntax_errors_are_reported_at_the_line_in_the_tcpou() {
    let f = fixture("twincat/errors/FB_Syntax.TcPOU");
    let o = plcc(&["check".as_ref(), f.as_os_str()]);
    assert!(!o.status.success());
    let t = text(&o);
    assert!(t.contains("FB_Syntax.TcPOU:16:35"), "{t}");
}

#[test]
fn a_directory_with_several_projects_asks_which_one() {
    let dir = std::env::temp_dir().join("plcc_twincat_two_projects");
    for sub in ["A", "B"] {
        let d = dir.join(sub);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(format!("{sub}.plcproj")), "<Project/>").unwrap();
    }
    let o = plcc(&["parse".as_ref(), dir.as_os_str()]);
    assert!(!o.status.success());
    let t = text(&o);
    assert!(t.contains("holds 2 TwinCAT projects"), "{t}");
}
