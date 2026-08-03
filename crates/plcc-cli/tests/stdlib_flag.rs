// SPDX-License-Identifier: MPL-2.0

//! `--stdlib` end-to-end.
//!
//! P1 made an unresolved FB instance type a hard error, so a program that says
//! `t : TON;` now either resolves against the injected prelude or fails loudly.
//! Both paths are exercised here through the real binary.

use std::process::Command;

const PLCC: &str = env!("CARGO_BIN_EXE_plcc");

fn write_st(name: &str, source: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("plcc_stdlib_flag_test");
    std::fs::create_dir_all(&dir).expect("failed to create temp dir");
    let path = dir.join(format!("{name}.st"));
    std::fs::write(&path, source).expect("failed to write ST source");
    path
}

fn compile_ir(name: &str, source: &str, extra: &[&str]) -> (String, String, bool) {
    let src = write_st(name, source);
    let out = std::env::temp_dir()
        .join("plcc_stdlib_flag_test")
        .join(format!("{name}.ll"));
    let status = Command::new(PLCC)
        .arg("compile")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .args(extra)
        .output()
        .expect("failed to run plcc compile");
    let ir = std::fs::read_to_string(&out).unwrap_or_default();
    std::fs::remove_file(&src).ok();
    std::fs::remove_file(&out).ok();
    (
        ir,
        String::from_utf8_lossy(&status.stderr).into_owned(),
        status.status.success(),
    )
}

const USES_TON: &str = r#"
PROGRAM UsesTon
VAR
    t : TON;
    running : BOOL;
    q : BOOL;
END_VAR
    t(IN := running, PT := T#500ms);
    q := t.Q;
END_PROGRAM
"#;

#[test]
fn bundled_st_stdlib_resolves_ton_by_default() {
    let (ir, stderr, ok) = compile_ir("uses_ton_default", USES_TON, &[]);
    assert!(ok, "compile failed with the default stdlib:\n{stderr}");
    assert!(
        ir.contains("define void @ton_scan("),
        "the bundled TON must be compiled into the module"
    );
    assert!(
        ir.contains("call void @ton_scan("),
        "the program must actually call ton_scan — a dropped FB call is the \
         silent-miscompile failure mode this whole change exists to prevent"
    );
    assert!(
        ir.contains("declare i64 @plcc_monotonic_ns()"),
        "the bundled TON must import the platform clock"
    );
}

#[test]
fn stdlib_none_makes_ton_a_hard_error() {
    let (_ir, stderr, ok) = compile_ir("uses_ton_none", USES_TON, &["--stdlib", "none"]);
    assert!(!ok, "compiling TON with --stdlib none must fail");
    assert!(
        stderr.contains("TON"),
        "the diagnostic must name TON:\n{stderr}"
    );
}

#[test]
fn every_bundled_block_is_available_to_a_program() {
    let source = r#"
PROGRAM AllBlocks
VAR
    a : SR;
    b : RS;
    c : R_TRIG;
    d : F_TRIG;
    e : CTU;
    f : CTD;
    g : CTUD;
    h : TON;
    i : TOF;
    j : TP;
    x : BOOL;
END_VAR
    a(S1 := x, R := x);
    b(S := x, R1 := x);
    c(CLK := x);
    d(CLK := x);
    e(CU := x, R := x, PV := 5);
    f(CD := x, LD := x, PV := 5);
    g(CU := x, CD := x, R := x, LD := x, PV := 5);
    h(IN := x, PT := T#1s);
    i(IN := x, PT := T#1s);
    j(IN := x, PT := T#1s);
    x := a.Q1;
END_PROGRAM
"#;
    let (ir, stderr, ok) = compile_ir("all_blocks", source, &[]);
    assert!(ok, "compile failed:\n{stderr}");
    for fb in [
        "sr", "rs", "r_trig", "f_trig", "ctu", "ctd", "ctud", "ton", "tof", "tp",
    ] {
        assert!(
            ir.contains(&format!("call void @{fb}_scan(")),
            "the program must call {fb}_scan"
        );
    }
}

#[test]
fn a_user_defined_block_supersedes_the_prelude_one() {
    // The user's TON has a completely different interface. If both definitions were
    // kept, or if the prelude's won, the `LIMIT` input would not resolve.
    let source = r#"
FUNCTION_BLOCK TON
VAR_INPUT
    IN : BOOL;
    LIMIT : INT;
END_VAR
VAR_OUTPUT
    Q : BOOL;
    COUNT : INT;
END_VAR
    IF IN THEN
        COUNT := COUNT + 1;
    ELSE
        COUNT := 0;
    END_IF;
    Q := COUNT >= LIMIT;
END_FUNCTION_BLOCK

PROGRAM UserTon
VAR
    t : TON;
    on : BOOL := TRUE;
    q : BOOL;
END_VAR
    t(IN := on, LIMIT := 3);
    q := t.Q;
END_PROGRAM
"#;
    let (ir, stderr, ok) = compile_ir("user_ton", source, &[]);
    assert!(ok, "compile failed:\n{stderr}");
    assert_eq!(
        ir.matches("define void @ton_scan(").count(),
        1,
        "exactly one ton_scan must be emitted — the user's"
    );
    // The prelude's TON reads the clock; the user's does not. Isolate ton_scan and
    // check it is the user's body. (TOF/TP are still injected and still import the
    // clock, so a module-wide check would prove nothing.)
    let body = ir
        .split("define void @ton_scan(")
        .nth(1)
        .expect("ton_scan must be defined")
        .split("\n}")
        .next()
        .expect("ton_scan must be terminated");
    assert!(
        !body.contains("plcc_monotonic_ns"),
        "ton_scan must be the user's clock-free body, not the prelude's:\n{body}"
    );
    assert!(
        body.contains("LIMIT"),
        "ton_scan must use the user's LIMIT input:\n{body}"
    );
}
