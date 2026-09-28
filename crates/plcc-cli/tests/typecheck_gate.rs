// SPDX-License-Identifier: MPL-2.0

//! `plcc compile` runs the type checker first, as CODESYS does: a project with a
//! type error does not build. `plcc check` runs the very same check, so the two
//! agree; `--no-typecheck` skips it.

use std::path::PathBuf;
use std::process::Command;

const PLCC: &str = env!("CARGO_BIN_EXE_plcc");

fn dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("plcc_typecheck_gate_{}", std::process::id()));
    std::fs::create_dir_all(&d).expect("temp dir");
    d
}

fn write_st(name: &str, source: &str) -> PathBuf {
    let path = dir().join(format!("{name}.st"));
    std::fs::write(&path, source).expect("write ST");
    path
}

struct Run {
    ok: bool,
    stderr: String,
}

fn run(args: &[&str], files: &[&PathBuf]) -> Run {
    let out = Command::new(PLCC)
        .args(args)
        .args(files.iter().map(|p| p.as_os_str()))
        .output()
        .expect("run plcc");
    Run {
        ok: out.status.success(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn compile(files: &[&PathBuf], extra: &[&str]) -> Run {
    let obj = dir().join(format!(
        "{}.ll",
        files[0].file_stem().unwrap().to_string_lossy()
    ));
    let mut args = vec!["compile", "-o", obj.to_str().unwrap()];
    args.extend_from_slice(extra);
    run(&args, files)
}

fn check(files: &[&PathBuf]) -> Run {
    run(&["check"], files)
}

const TYPE_ERROR: &str = r#"
PROGRAM p
VAR
    i : INT;
    r : REAL;
END_VAR
    r := 'hello';
    i := i + 1;
END_PROGRAM
"#;

#[test]
fn compile_refuses_a_type_error() {
    let f = write_st("type_error", TYPE_ERROR);
    let c = compile(&[&f], &[]);
    assert!(!c.ok, "a type error must stop the build");
    assert!(c.stderr.contains("type mismatch"), "{}", c.stderr);
    assert!(c.stderr.contains("--no-typecheck"), "{}", c.stderr);
}

#[test]
fn no_typecheck_skips_the_checker() {
    // Codegen itself has no objection to this assignment's *shape*, so the build
    // goes as far as codegen takes it; what matters is that the checker did not
    // run.
    let f = write_st(
        "no_tc",
        "PROGRAM p VAR i : INT; t : TIME; END_VAR i := t; END_PROGRAM",
    );
    assert!(!compile(&[&f], &[]).ok);
    let c = compile(&[&f], &["--no-typecheck"]);
    assert!(!c.stderr.contains("type mismatch"), "{}", c.stderr);
    assert!(c.ok, "--no-typecheck build failed: {}", c.stderr);
}

#[test]
fn warnings_do_not_stop_the_build() {
    let f = write_st(
        "narrowing",
        "PROGRAM p VAR i : INT; r : REAL := 2.5; END_VAR i := r; END_PROGRAM",
    );
    let c = compile(&[&f], &[]);
    assert!(c.ok, "{}", c.stderr);
    assert!(c.stderr.contains("implicit conversion"), "{}", c.stderr);
}

#[test]
fn check_and_compile_agree() {
    let sources = [
        ("agree_bad", TYPE_ERROR),
        ("agree_real", "PROGRAM p VAR x : REAL; END_VAR x := 2.0 * x; END_PROGRAM"),
        ("agree_byte", "PROGRAM p VAR b : BYTE; END_VAR b := b AND 16#0F; END_PROGRAM"),
        (
            "agree_ton",
            "PROGRAM p VAR t : TON; q : BOOL; END_VAR t(IN := TRUE, PT := T#1s); q := t.Q; END_PROGRAM",
        ),
        ("agree_cond", "PROGRAM p VAR i : INT; END_VAR IF i THEN i := 0; END_IF; END_PROGRAM"),
    ];
    for (name, src) in sources {
        let f = write_st(name, src);
        let (k, c) = (check(&[&f]), compile(&[&f], &[]));
        assert_eq!(k.ok, c.ok, "{name}: check={} compile={}\n{}\n{}", k.ok, c.ok, k.stderr, c.stderr);
    }
}

#[test]
fn check_takes_several_files_like_compile() {
    let lib = write_st(
        "lib_fn",
        "FUNCTION TWICE : REAL VAR_INPUT x : REAL; END_VAR TWICE := 2.0 * x; END_FUNCTION",
    );
    let main = write_st(
        "main_uses_lib",
        "PROGRAM p VAR r : REAL; END_VAR r := TWICE(1.5); END_PROGRAM",
    );
    assert!(check(&[&lib, &main]).ok);
    assert!(compile(&[&lib, &main], &[]).ok);
}

#[test]
fn diagnostic_points_at_the_right_file() {
    let good = write_st("where_good", "FUNCTION G : INT G := 1; END_FUNCTION");
    let bad = write_st("where_bad", TYPE_ERROR);
    let k = check(&[&good, &bad]);
    assert!(!k.ok);
    assert!(k.stderr.contains("where_bad.st"), "{}", k.stderr);
    assert!(k.stderr.contains("'hello'"), "source line shown: {}", k.stderr);
}
