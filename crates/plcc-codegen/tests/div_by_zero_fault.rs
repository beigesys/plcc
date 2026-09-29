// SPDX-License-Identifier: MPL-2.0

//! Integer division / MOD by zero is a runtime fault, as in CODESYS (the task
//! stops with an exception): compiled code calls `plcc_fault(PLCC_FAULT_DIV_BY_ZERO,
//! where)` and traps if that returns. The module carries a weak default
//! `plcc_fault` that traps, so objects link without the runtime defining it.
//!
//! The JIT harness maps `plcc_fault` onto a handler that unwinds out of the scan
//! and records the fault; the statement after the division must not have run.

mod common;
use common::{run, run_o3, run_with};
use inkwell::context::Context;
use inkwell::targets::TargetMachine;
use plcc_codegen::Compiler;
use std::path::{Path, PathBuf};
use std::process::Command;

const DIV_BY_ZERO: u32 = 1;

/// `x <op> zero` for one type, then `after := 1`.
fn case(ty: &str, expr: &str) -> String {
    format!(
        "PROGRAM p
VAR
  zero : {ty}; x : {ty} := {ty}#7; r : {ty}; after : INT; before : INT;
END_VAR
before := 1;
r := {expr};
after := 1;
END_PROGRAM"
    )
}

#[test]
fn division_and_mod_by_zero_fault_on_every_integer_type() {
    let types = [
        "SINT", "INT", "DINT", "LINT", "USINT", "UINT", "UDINT", "ULINT", "BYTE", "WORD", "DWORD",
        "LWORD",
    ];
    for ty in types {
        for expr in ["x / zero", "x MOD zero", "DIV(x, zero)"] {
            let src = case(ty, expr);
            for (opt, s) in [("O0", run(&src)), ("O3", run_o3(&src))] {
                let f = s
                    .fault
                    .as_ref()
                    .unwrap_or_else(|| panic!("{ty}: `{expr}` did not fault at {opt}"));
                assert_eq!(f.code, DIV_BY_ZERO, "{ty}: {expr} at {opt}");
                assert!(f.site.ends_with("p") || f.site.ends_with("P"), "site {:?}", f.site);
                assert_eq!(s.i64("before"), 1, "{ty}: {expr} at {opt}");
                assert_eq!(s.i64("after"), 0, "{ty}: {expr} at {opt}: the task must stop");
            }
        }
    }
}

#[test]
fn time_division_by_zero_faults() {
    for expr in ["t / n", "DIV_TIME(t, n)", "t MOD n"] {
        let src = format!(
            "PROGRAM p VAR t : TIME := T#1s; n : DINT; r : TIME; after : INT; END_VAR
             r := {expr}; after := 1; END_PROGRAM"
        );
        for s in [run(&src), run_o3(&src)] {
            assert_eq!(s.fault.as_ref().map(|f| f.code), Some(DIV_BY_ZERO), "{expr}");
            assert_eq!(s.i64("after"), 0);
        }
    }
}

#[test]
fn a_fault_stops_the_run_at_the_scan_it_happens_in() {
    // Divides by 3, 2, 1, 0: the fourth scan faults.
    let src = "PROGRAM p VAR d : DINT := 4; q : DINT; n : INT; END_VAR
               d := d - 1; q := 12 / d; n := n + 1; END_PROGRAM";
    let s = run_with(src, 10, 10, false);
    assert_eq!(s.scans, 3);
    assert_eq!(s.i64("n"), 3);
    assert_eq!(s.i64("q"), 12);
    assert_eq!(s.fault.as_ref().map(|f| f.code), Some(DIV_BY_ZERO));
}

#[test]
fn the_site_names_the_pou_and_method() {
    let src = "FUNCTION_BLOCK FB_X
               VAR_INPUT d : DINT; END_VAR
               METHOD Calc : DINT
                 Calc := 10 / d;
               END_METHOD
               END_FUNCTION_BLOCK
               FUNCTION f : DINT VAR_INPUT d : DINT; END_VAR f := 10 MOD d; END_FUNCTION
               PROGRAM p VAR fb : FB_X; r : DINT; sel : INT; END_VAR
               r := f(0);
               END_PROGRAM";
    let s = run(src);
    assert_eq!(s.fault.as_ref().map(|f| f.site.as_str()), Some("f"));
    let src2 = src.replace("r := f(0);", "r := fb.Calc();");
    let s = run(&src2);
    assert_eq!(s.fault.as_ref().map(|f| f.site.as_str()), Some("FB_X.Calc"));
}

#[test]
fn the_site_has_file_line_and_column_when_the_source_is_registered() {
    let src = "PROGRAM p\nVAR a : INT; b : INT; END_VAR\n  a := 1;\n  a := a / b;\nEND_PROGRAM\n";
    let (unit, errors) = plcc_st::parse(src);
    assert!(errors.is_empty(), "{errors:?}");
    let ctx = Context::create();
    let mut c = Compiler::new(&ctx, "m");
    c.add_source_file("main.st", src, ["p"]);
    c.compile(&unit).unwrap();
    let ir = c.emit_ir();
    assert!(ir.contains("c\"main.st:4:8: p\\00\""), "{ir}");
    assert!(ir.contains("define weak void @plcc_fault(i32"), "{ir}");
}

#[test]
fn nonzero_divisors_do_not_fault_and_edge_cases_keep_their_values() {
    let src = "PROGRAM p
VAR
  m1 : INT := -1; mn : INT := -32768; lmn : LINT := LINT#-9223372036854775808; lm1 : LINT := -1;
  a : INT; b : INT; c : LINT; d : DINT; e : DINT; u : UDINT := 7; f : UDINT;
  rz : REAL; r1 : REAL; r2 : LREAL; zr : REAL; nd : DINT := -7; three : DINT := 3;
END_VAR
a := mn / m1;
b := mn MOD m1;
c := lmn / lm1;
d := nd / three;
e := nd MOD three;
f := u / UDINT#2;
r1 := 1.0 / zr;
r2 := 0.0 / LREAL#0.0;
rz := -1.0 / zr;
END_PROGRAM";
    for s in [run(src), run_o3(src)] {
        assert_eq!(s.fault, None);
        assert_eq!(s.i64("a"), -32768, "MIN / -1 wraps");
        assert_eq!(s.i64("b"), 0);
        assert_eq!(s.i64("c"), i64::MIN);
        assert_eq!(s.i64("d"), -2);
        assert_eq!(s.i64("e"), -1, "MOD keeps the dividend's sign");
        assert_eq!(s.u64("f"), 3);
        assert_eq!(s.f64("r1"), f64::INFINITY, "REAL division is IEEE");
        assert!(s.f64("r2").is_nan());
        assert_eq!(s.f64("rz"), f64::NEG_INFINITY);
    }
}

fn optimized_ir(src: &str) -> String {
    let (unit, errors) = plcc_st::parse(src);
    assert!(errors.is_empty(), "{errors:?}");
    let ctx = Context::create();
    let mut c = Compiler::new(&ctx, "m");
    c.compile(&unit).unwrap();
    let triple = TargetMachine::get_default_triple();
    let triple = triple.as_str().to_string_lossy();
    c.optimize(&triple, 2).unwrap();
    c.emit_ir()
}

#[test]
fn a_constant_divisor_pays_nothing() {
    let ir = optimized_ir(
        "PROGRAM p VAR x : DINT; y : INT; b : BYTE; t : TIME; a : DINT; c : INT; d : BYTE; e : TIME; END_VAR
         a := x / 7; c := y MOD 10; d := b / 3; e := t / 4; a := a + DIV(x, -1);
         END_PROGRAM",
    );
    assert!(!ir.contains("plcc_fault"), "{ir}");
    let ir = optimized_ir("PROGRAM p VAR x : DINT; n : DINT; a : DINT; END_VAR a := x / n; END_PROGRAM");
    assert!(ir.contains("call void @plcc_fault(i32 1"), "{ir}");
}

// ── Linking: the weak default and a runtime's strong override ─────────────────

const LINK_PROGRAM: &str = "PROGRAM p VAR n : DINT; q : DINT; END_VAR q := 100 / n; END_PROGRAM";

fn object(dir: &Path, triple: &str) {
    let (unit, errors) = plcc_st::parse(LINK_PROGRAM);
    assert!(errors.is_empty());
    let ctx = Context::create();
    let mut c = Compiler::new(&ctx, "program");
    c.add_source_file("prog.st", LINK_PROGRAM, ["p"]);
    c.compile(&unit).unwrap();
    let contract = c.runtime_contract(triple).unwrap();
    std::fs::write(
        dir.join("program.h"),
        plcc_codegen::header::c_header(&contract, "PROGRAM_H"),
    )
    .unwrap();
    c.optimize(triple, 2).unwrap();
    c.emit_object(&dir.join("program.o"), triple).unwrap();
}

fn work_dir(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn have(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").output().is_ok()
}

const MAIN_C: &str = r#"
#include <stdint.h>
#include <stdio.h>
#include <unistd.h>
#include "program.h"
#ifdef STRONG
void plcc_fault(uint32_t code, const char *where) {
    printf("fault %u%s at %s\n", code, code == PLCC_FAULT_DIV_BY_ZERO ? " (div by zero)" : "", where);
    fflush(stdout);
    _exit(3);
}
#endif
int main(void) {
    plcc_init();
    plcc_run_task(0);
    printf("returned\n");
    return 0;
}
"#;

#[test]
fn host_objects_link_without_a_handler_and_a_strong_handler_overrides() {
    if !have("cc") {
        eprintln!("skipping: no system C compiler");
        return;
    }
    let dir = work_dir("fault_link_host");
    let triple = TargetMachine::get_default_triple();
    object(&dir, &triple.as_str().to_string_lossy());
    std::fs::write(dir.join("main.c"), MAIN_C).unwrap();
    for (exe, strong) in [("weak", false), ("strong", true)] {
        let mut cmd = Command::new("cc");
        cmd.current_dir(&dir)
            .args(["-std=c11", "-D_DEFAULT_SOURCE", "-Wall", "-Werror", "-no-pie"])
            .args(["main.c", "program.o", "-o", exe]);
        if strong {
            cmd.arg("-DSTRONG");
        }
        let out = cmd.output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }
    // No handler: the weak default traps (SIGILL on x86, SIGTRAP elsewhere).
    let run = Command::new(dir.join("weak")).output().unwrap();
    assert!(!run.status.success());
    assert!(!String::from_utf8_lossy(&run.stdout).contains("returned"));
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert!(run.status.signal().is_some(), "{:?}", run.status);
    }
    // A strong handler replaces it.
    let run = Command::new(dir.join("strong")).output().unwrap();
    assert_eq!(run.status.code(), Some(3));
    assert_eq!(String::from_utf8_lossy(&run.stdout), "fault 1 (div by zero) at prog.st:1:48: p\n");
}

fn arm_gcc() -> Option<PathBuf> {
    let mut candidates = vec![PathBuf::from("arm-none-eabi-gcc")];
    if let Some(home) = std::env::var_os("HOME") {
        let base = Path::new(&home).join(".arduino15/packages/arduino/tools/arm-none-eabi-gcc");
        if let Ok(rd) = std::fs::read_dir(base) {
            for e in rd.flatten() {
                candidates.push(e.path().join("bin/arm-none-eabi-gcc"));
            }
        }
    }
    candidates
        .into_iter()
        .find(|c| Command::new(c).arg("--version").output().is_ok())
}

#[test]
fn thumbv7em_objects_link_without_a_handler() {
    let Some(gcc) = arm_gcc() else {
        eprintln!("skipping: no arm-none-eabi-gcc");
        return;
    };
    let nm = gcc.with_file_name("arm-none-eabi-nm");
    let dir = work_dir("fault_link_thumbv7em");
    object(&dir, "thumbv7em-none-eabi");
    std::fs::write(dir.join("main.c"), MAIN_C.replace("#include <unistd.h>", "#include <unistd.h>\nint64_t plcc_monotonic_ns(void) { return 0; }")).unwrap();
    for (exe, strong, kind) in [("weak.elf", false, " W plcc_fault"), ("strong.elf", true, " T plcc_fault")] {
        let mut cmd = Command::new(&gcc);
        cmd.current_dir(&dir).args([
            "-mcpu=cortex-m7",
            "-mthumb",
            "--specs=nosys.specs",
            "main.c",
            "program.o",
            "-o",
            exe,
        ]);
        if strong {
            cmd.arg("-DSTRONG");
        }
        let out = cmd.output().unwrap();
        assert!(out.status.success(), "{exe}: {}", String::from_utf8_lossy(&out.stderr));
        let syms = Command::new(&nm).current_dir(&dir).arg(exe).output().unwrap();
        let syms = String::from_utf8_lossy(&syms.stdout);
        assert!(syms.contains(kind), "{exe}: expected `{kind}` in\n{syms}");
    }
}
