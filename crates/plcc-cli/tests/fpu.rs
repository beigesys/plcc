// SPDX-License-Identifier: MPL-2.0

//! `--cpu`, `--features` and `--float-abi`: with the Arduino Opta's flags
//! (`-mcpu=cortex-m7 -mfloat-abi=softfp -mfpu=fpv5-d16` in the Arduino core,
//! `--cpu cortex-m7 --features +fp-armv8d16 --float-abi softfp` here) REAL
//! arithmetic is FPU instructions, and floats still cross calls in integer
//! registers (r0, r1, ...), so the object links with the core's code.
//!
//! The disassembly checks need `llvm-objdump` (from `$LLVM_SYS_211_PREFIX/bin`,
//! `/usr/lib/llvm-21/bin` or `PATH`) and are skipped, with a message, without it.

use std::path::{Path, PathBuf};
use std::process::Command;

const PLCC: &str = env!("CARGO_BIN_EXE_plcc");
const OPTA: &[&str] = &["--cpu", "cortex-m7", "--features", "+fp-armv8d16", "--float-abi", "softfp"];

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/codegen/real_fpu.st")
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("plcc_fpu_{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

fn compile(out: &Path, extra: &[&str]) -> std::process::Output {
    Command::new(PLCC)
        .arg("compile")
        .arg(fixture())
        .arg("-o")
        .arg(out)
        .args(["--target", "thumbv7em-none-eabi"])
        .args(extra)
        .output()
        .unwrap()
}

fn objdump() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os("LLVM_SYS_211_PREFIX") {
        candidates.push(Path::new(&p).join("bin/llvm-objdump"));
    }
    candidates.push("/usr/lib/llvm-21/bin/llvm-objdump".into());
    candidates.push("llvm-objdump-21".into());
    candidates.push("llvm-objdump".into());
    candidates
        .into_iter()
        .find(|c| Command::new(c).arg("--version").output().is_ok_and(|o| o.status.success()))
}

/// Disassembly of the fixture built with `extra`, or None without llvm-objdump.
fn disassemble(name: &str, extra: &[&str]) -> Option<String> {
    let Some(od) = objdump() else {
        eprintln!("skipped: no llvm-objdump found");
        return None;
    };
    let obj = tmp(name);
    let out = compile(&obj, extra);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let dis = Command::new(od)
        .args(["-dr", "--mcpu=cortex-m7", "--no-show-raw-insn"])
        .arg(&obj)
        .output()
        .unwrap();
    assert!(dis.status.success());
    Some(String::from_utf8_lossy(&dis.stdout).into_owned())
}

/// The instructions of function `name` in an objdump listing.
fn function<'a>(dis: &'a str, name: &str) -> &'a str {
    let start = dis.find(&format!("<{name}>:")).unwrap_or_else(|| panic!("no {name} in\n{dis}"));
    let rest = &dis[start..];
    &rest[..rest.find("\n\n").unwrap_or(rest.len())]
}

const SOFT_FLOAT_CALLS: &[&str] = &["__addsf3", "__mulsf3", "__aeabi_fadd", "__aeabi_fmul"];

#[test]
fn opta_flags_use_the_fpu_with_the_soft_float_calling_convention() {
    let Some(dis) = disassemble("fpu.o", OPTA) else { return };
    for insn in ["vadd.f32", "vmul.f32"] {
        assert!(dis.contains(insn), "no {insn}:\n{dis}");
    }
    for call in SOFT_FLOAT_CALLS {
        assert!(!dis.contains(call), "still calls {call}:\n{dis}");
    }
    // Scale(raw, gain, offset): REAL arguments arrive in r0..r2, the result
    // leaves in r0 (AAPCS base variant, like -mfloat-abi=softfp).
    let scale = function(&dis, "scale");
    for arg in ["r0", "r1", "r2"] {
        assert!(
            scale.lines().any(|l| l.contains("vmov") && l.contains(&format!(", {arg}"))),
            "argument {arg} is not moved into the FPU:\n{scale}"
        );
    }
    assert!(scale.contains("ldr\tr0") || scale.contains("vmov\tr0"), "result not in r0:\n{scale}");
}

#[test]
fn generic_cortex_m_code_calls_the_soft_float_library() {
    let Some(dis) = disassemble("soft.o", &[]) else { return };
    assert!(SOFT_FLOAT_CALLS.iter().any(|c| dis.contains(c)), "no soft-float call:\n{dis}");
    assert!(!dis.contains(".f32"), "FPU instruction without an FPU:\n{dis}");
}

#[test]
fn float_abi_soft_turns_the_fpu_off() {
    let Some(dis) = disassemble("off.o", &["--cpu", "cortex-m7", "--float-abi", "soft"]) else {
        return;
    };
    assert!(!dis.contains(".f32"), "FPU instruction with --float-abi soft:\n{dis}");
}

#[test]
fn cpu_and_features_are_recorded_in_the_ir() {
    let ll = tmp("fpu.ll");
    let out = compile(&ll, OPTA);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let ir = std::fs::read_to_string(&ll).unwrap();
    assert!(ir.contains("\"target-cpu\"=\"cortex-m7\""), "{ir}");
    assert!(ir.contains("\"target-features\"=\"+fp-armv8d16\""), "{ir}");
}

#[test]
fn inconsistent_flags_are_errors() {
    let o = tmp("bad.o");
    let out = compile(&o, &["--float-abi", "hard"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("needs an `eabihf` triple"), "{}", stderr(&out));
    let out = compile(&o, &["--float-abi", "fast"]);
    assert!(stderr(&out).contains("is not soft, softfp or hard"), "{}", stderr(&out));
    let out = compile(&o, &["--features", "fp-armv8d16"]);
    assert!(stderr(&out).contains("write +name or -name"), "{}", stderr(&out));
}

/// stderr with miette's line wrapping undone.
fn stderr(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .map(|l| l.trim_start_matches([' ', '│', '×']).trim_end())
        .collect::<Vec<_>>()
        .join(" ")
}
