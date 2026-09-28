// SPDX-License-Identifier: MPL-2.0

//! `--emit-header`: the generated C header must compile, its layout asserts must
//! hold for the target it was generated for (and fail when the layout is wrong),
//! and a generic C runtime written only against the header must be able to run a
//! compiled object without knowing anything about the program in it.

use inkwell::context::Context;
use inkwell::targets::TargetMachine;
use plcc_codegen::Compiler;
use std::path::{Path, PathBuf};
use std::process::Command;

const PROGRAM: &str = r#"
TYPE Recipe : STRUCT
    speed : INT;
    temp : REAL;
    label : STRING[7];
END_STRUCT
END_TYPE

FUNCTION_BLOCK Edge
VAR_INPUT clk : BOOL; END_VAR
VAR_OUTPUT q : BOOL; END_VAR
VAR prev : BOOL; END_VAR
    q := clk AND NOT prev;
    prev := clk;
END_FUNCTION_BLOCK

PROGRAM Main
VAR
    btn AT %IX0.2 : BOOL;
    lamp AT %QX0.0 : BOOL;
    presses AT %QW1 : INT;
    raw AT %ID1 : DINT;
    doubled AT %QD1 : DINT;
    e : Edge;
    r : Recipe;
    history : ARRAY[0..4] OF LREAL;
    ptr : POINTER TO INT;
END_VAR
VAR RETAIN
    total : UDINT;
END_VAR
    e(clk := btn);
    IF e.q THEN
        presses := presses + 1;
        total := total + 1;
    END_IF;
    lamp := btn;
    doubled := raw * 2;
END_PROGRAM

CONFIGURATION Plant
    RESOURCE Cpu ON Board
        TASK Cyclic (INTERVAL := T#10ms, PRIORITY := 1);
        PROGRAM main1 WITH Cyclic : Main;
    END_RESOURCE
END_CONFIGURATION
"#;

/// A runtime that knows nothing about the program: it only uses the header.
const RUNTIME_C: &str = r#"
#include <stdio.h>
#include <string.h>
#include "program.h"

int64_t plcc_monotonic_ns(void) { return 0; }
void plcc_print(const char *msg) { (void)msg; }

int main(void) {
    const plcc_app_t *app = plcc_get_app();
    if (app->abi_version != PLCC_ABI_VERSION) return 10;
    if (app != &plcc_app) return 11;
    plcc_init();
    int32_t raw = 21;
    memcpy(&plcc_image_i[4], &raw, sizeof raw); /* %ID1 */
    for (int cycle = 0; cycle < 6; cycle++) {
        /* latch inputs: press the button on odd cycles */
        if (cycle & 1) plcc_image_i[0] |= (1u << PLCC_AT_MAIN_BTN_BIT);
        else plcc_image_i[0] &= (uint8_t)~(1u << PLCC_AT_MAIN_BTN_BIT);
        for (uint32_t t = 0; t < app->task_count; t++) app->run_task(t);
    }
    int16_t presses;
    memcpy(&presses, PLCC_AT_MAIN_PRESSES_PTR, sizeof presses);
    int32_t doubled;
    memcpy(&doubled, &plcc_image_q[4], sizeof doubled);
    const plcc_task_t *t = &app->tasks[0];
    printf("task=%s interval=%lld prio=%u progs=%u inst=%s\n", t->name,
           (long long)t->interval_ns, t->priority, t->program_count, t->programs[0].name);
    printf("presses=%d doubled=%d lamp=%d\n", presses, doubled, plcc_image_q[0] & 1);
    printf("total=%u retain=%s:%llu sig_ok=%d\n", plcc_inst_cpu_main1.total,
           app->retain[0].name, (unsigned long long)app->retain[0].size,
           app->retain_signature == PLCC_RETAIN_SIGNATURE);
    printf("state_size_ok=%d\n", t->programs[0].state_size == sizeof(plcc_prog_main_t));
    return 0;
}
"#;

fn work_dir(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn host_triple() -> String {
    TargetMachine::get_default_triple()
        .as_str()
        .to_string_lossy()
        .into_owned()
}

/// Compile PROGRAM for `triple`, writing `program.h` (and `program.o` if asked).
fn build(dir: &Path, triple: &str, object: bool) -> String {
    let (unit, errors) = plcc_st::parse(PROGRAM);
    assert!(errors.is_empty(), "{errors:?}");
    let ctx = Context::create();
    let mut compiler = Compiler::new(&ctx, "program");
    compiler.compile(&unit).expect("codegen");
    let contract = compiler.runtime_contract(triple).expect("contract");
    let header = plcc_codegen::header::c_header(&contract, "PROGRAM_H");
    std::fs::write(dir.join("program.h"), &header).unwrap();
    if object {
        compiler
            .emit_object(&dir.join("program.o"), triple)
            .expect("object");
    }
    header
}

fn have(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").output().is_ok()
}

#[test]
fn generic_c_runtime_runs_the_program_through_the_header() {
    if !have("cc") {
        eprintln!("skipping: no system C compiler");
        return;
    }
    let dir = work_dir("c_runtime");
    build(&dir, &host_triple(), true);
    std::fs::write(dir.join("runtime.c"), RUNTIME_C).unwrap();
    let out = Command::new("cc")
        .current_dir(&dir)
        .args(["-std=c11", "-Wall", "-Werror", "-no-pie", "runtime.c", "program.o", "-o", "runtime", "-lm"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "runtime failed to build:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(dir.join("runtime")).output().unwrap();
    assert!(run.status.success(), "runtime exited with {:?}", run.status);
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        stdout.contains("task=Cyclic interval=10000000 prio=1 progs=1 inst=Cpu.main1"),
        "{stdout}"
    );
    assert!(stdout.contains("presses=3 doubled=42 lamp=1"), "{stdout}");
    assert!(stdout.contains("total=3 retain=Cpu.main1.total:4 sig_ok=1"), "{stdout}");
    assert!(stdout.contains("state_size_ok=1"), "{stdout}");
}

#[test]
fn header_compiles_as_c_and_cxx_and_asserts_bite() {
    if !have("cc") {
        eprintln!("skipping: no system C compiler");
        return;
    }
    let dir = work_dir("c_header");
    let header = build(&dir, &host_triple(), false);
    std::fs::write(dir.join("use.c"), "#include \"program.h\"\n").unwrap();
    let cc = |file: &str, extra: &[&str]| {
        Command::new("cc")
            .current_dir(&dir)
            .args(["-fsyntax-only", "-Wall", "-Werror"])
            .args(extra)
            .arg(file)
            .output()
            .unwrap()
    };
    let ok = cc("use.c", &["-std=c11"]);
    assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stderr));
    if have("c++") {
        let ok = Command::new("c++")
            .current_dir(&dir)
            .args(["-fsyntax-only", "-Wall", "-Werror", "-std=c++11", "-x", "c++", "use.c"])
            .output()
            .unwrap();
        assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stderr));
    }

    // Every layout fact is asserted: move one offset and the header must not compile.
    let line = header
        .lines()
        .find(|l| l.contains("offsetof(plcc_prog_main_t, history)"))
        .expect("history offset assert");
    let n: u64 = line
        .split("== ")
        .nth(1)
        .and_then(|s| s.split(',').next())
        .and_then(|s| s.trim().parse().ok())
        .expect("offset");
    let bad = header.replace(line, &line.replace(&format!("== {n},"), &format!("== {},", n + 1)));
    std::fs::write(dir.join("program.h"), bad).unwrap();
    let err = cc("use.c", &["-std=c11"]);
    assert!(!err.status.success(), "a wrong offset must fail the static assert");
}

/// The Arduino Opta is a Cortex-M7 with hardware floating point. If an ARM
/// toolchain is around (system or Arduino's), check the asserts hold there too.
#[test]
fn header_asserts_hold_on_thumbv7em() {
    let mut candidates = vec![PathBuf::from("arm-none-eabi-gcc")];
    if let Some(home) = std::env::var_os("HOME") {
        let base = Path::new(&home).join(".arduino15/packages/arduino/tools/arm-none-eabi-gcc");
        if let Ok(rd) = std::fs::read_dir(base) {
            for e in rd.flatten() {
                candidates.push(e.path().join("bin/arm-none-eabi-gcc"));
            }
        }
    }
    let found: Vec<PathBuf> = candidates
        .into_iter()
        .filter(|c| Command::new(c).arg("--version").output().is_ok())
        .collect();
    if found.is_empty() {
        eprintln!("skipping: no arm-none-eabi-gcc");
        return;
    }
    for triple in ["thumbv7em-none-eabihf", "thumbv7em-none-eabi"] {
        let dir = work_dir(&format!("c_header_{triple}"));
        build(&dir, triple, false);
        std::fs::write(dir.join("use.c"), "#include \"program.h\"\n").unwrap();
        for gcc in &found {
            let out = Command::new(gcc)
                .current_dir(&dir)
                .args(["-fsyntax-only", "-std=c11", "-mcpu=cortex-m7", "-mthumb", "-Wall", "-Werror", "use.c"])
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{triple} with {}:\n{}",
                gcc.display(),
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
}
