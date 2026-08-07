// SPDX-License-Identifier: MPL-2.0

//! Cross-compilation tests: verify we can emit object code for multiple target triples.

use inkwell::context::Context;
use inkwell::targets::{InitializationConfig, Target};
use object::{Object, ObjectSection, ObjectSymbol};
use plcc_codegen::{Compiler, TargetSpec};

/// `case` must be unique per test — several tests share a triple, and the object
/// path is what keeps their parallel runs from clobbering each other's cleanup.
fn compile_to_target(source: &str, triple: &str, case: &str) {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");

    let context = Context::create();
    let mut compiler = Compiler::new(&context, "cross_test");
    compiler.compile(&unit).expect("codegen failed");

    let ir = compiler.emit_ir();
    assert!(!ir.is_empty(), "empty IR");

    // Emit object file
    let dir = std::env::temp_dir().join("plcc_test");
    std::fs::create_dir_all(&dir).ok();
    let safe_name: String = triple
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    let obj_path = dir.join(format!("test_{case}_{safe_name}.o"));

    eprintln!("Emitting to: {}", obj_path.display());
    compiler
        .emit_object(&obj_path, triple)
        .unwrap_or_else(|e| panic!("failed to compile for {triple}: {e}"));

    let meta = std::fs::metadata(&obj_path)
        .unwrap_or_else(|e| panic!("object file not created at {}: {e}", obj_path.display()));
    assert!(meta.len() > 0, "object file is empty for {triple}");

    // Clean up
    std::fs::remove_file(&obj_path).ok();
}

const BLINK_ST: &str = r#"
PROGRAM Blink
VAR
    output : BOOL := FALSE;
END_VAR
    output := NOT output;
END_PROGRAM
"#;

const STATE_MACHINE_ST: &str = r#"
PROGRAM SM
VAR
    state : INT := 0;
    counter : INT := 0;
END_VAR
    CASE state OF
        0:
            counter := 0;
            state := 1;
        1:
            counter := counter + 1;
            IF counter >= 10 THEN
                state := 0;
            END_IF;
    END_CASE;
END_PROGRAM
"#;

#[test]
fn emit_x86_64_linux() {
    compile_to_target(BLINK_ST, "x86_64-unknown-linux-gnu", "emit_x86_64_linux");
}

#[test]
fn emit_x86_64_linux_state_machine() {
    compile_to_target(
        STATE_MACHINE_ST,
        "x86_64-unknown-linux-gnu",
        "emit_x86_64_linux_state_machine",
    );
}

#[test]
fn emit_aarch64_linux() {
    compile_to_target(BLINK_ST, "aarch64-unknown-linux-gnu", "emit_aarch64_linux");
}

#[test]
fn emit_armv7_none_eabi() {
    // Bare-metal ARM Cortex-M — typical MCU target
    compile_to_target(BLINK_ST, "armv7-unknown-none-eabi", "emit_armv7_none_eabi");
}

#[test]
fn emit_thumbv7em_none_eabi() {
    // ARM Cortex-M4/M7 — PLC-class MCU
    compile_to_target(
        BLINK_ST,
        "thumbv7em-unknown-none-eabi",
        "emit_thumbv7em_none_eabi",
    );
}

#[test]
fn emit_wasm32() {
    // WebAssembly — browser/web runtime
    compile_to_target(BLINK_ST, "wasm32-unknown-unknown", "emit_wasm32");
}

#[test]
fn emit_wasm32_state_machine() {
    compile_to_target(
        STATE_MACHINE_ST,
        "wasm32-unknown-unknown",
        "emit_wasm32_state_machine",
    );
}

#[test]
fn emit_riscv32() {
    // RISC-V 32-bit — emerging MCU arch
    compile_to_target(BLINK_ST, "riscv32-unknown-none-elf", "emit_riscv32");
}

// ---------------------------------------------------------------------------
// Properties of the emitted object, not just its existence.
//
// "An object file was produced" is a weak assertion — it held while every REAL
// operation on Cortex-M4F was compiling to a libgcc soft-float call under the
// wrong ABI. These tests read the symbol and section tables back.
// ---------------------------------------------------------------------------

const FLOAT_MATH_ST: &str = r#"
PROGRAM FloatMath
VAR
    a : REAL := 1.5;
    b : REAL := 2.5;
    c : REAL := 0.0;
END_VAR
    c := a * b + a;
    c := c / b - a;
END_PROGRAM
"#;

/// Emit an object for `spec` and hand back its bytes.
fn object_bytes(source: &str, spec: &TargetSpec, case: &str) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");

    let context = Context::create();
    let mut compiler = Compiler::new(&context, "cross_test");
    compiler.compile(&unit).expect("codegen failed");

    let dir = std::env::temp_dir().join("plcc_test");
    std::fs::create_dir_all(&dir).ok();
    let obj_path = dir.join(format!("props_{case}.o"));

    compiler
        .emit_object_for(&obj_path, spec)
        .unwrap_or_else(|e| panic!("failed to compile for {}: {e}", spec.triple));

    let bytes = std::fs::read(&obj_path).expect("object file not created");
    std::fs::remove_file(&obj_path).ok();
    bytes
}

fn undefined_symbols(bytes: &[u8]) -> Vec<String> {
    let file = object::File::parse(bytes).expect("failed to parse object file");
    file.symbols()
        .filter(|s| s.is_undefined())
        .filter_map(|s| s.name().ok().map(str::to_string))
        .filter(|n| !n.is_empty())
        .collect()
}

fn section_names(bytes: &[u8]) -> Vec<String> {
    let file = object::File::parse(bytes).expect("failed to parse object file");
    file.sections()
        .filter_map(|s| s.name().ok().map(str::to_string))
        .collect()
}

/// A hard-float triple must produce hardware FP, not soft-float libcalls.
///
/// LLVM's `generic` CPU assumes no FPU, so `thumbv7em-none-eabihf` used to lower
/// `a * b` to `__mulsf3` — which passes floats in core registers while the triple
/// promises `s0`-`s15`. Nothing failed at link time; the values arrived wrong.
#[test]
fn hard_float_target_avoids_soft_float_libcalls() {
    let spec = TargetSpec::new("thumbv7em-none-eabihf");
    assert_eq!(spec.cpu, "cortex-m4", "expected an FPU-equipped default CPU");

    let bytes = object_bytes(FLOAT_MATH_ST, &spec, "hard_float");
    let undef = undefined_symbols(&bytes);

    let soft_float: Vec<_> = undef
        .iter()
        .filter(|n| {
            n.starts_with("__mulsf")
                || n.starts_with("__addsf")
                || n.starts_with("__subsf")
                || n.starts_with("__divsf")
                || n.starts_with("__aeabi_f")
        })
        .collect();

    assert!(
        soft_float.is_empty(),
        "hard-float target emitted soft-float libcalls: {soft_float:?}"
    );
}

/// Structured Text cannot throw, so nothing should reference the ARM EHABI
/// personality routine. It pulls the whole C++ unwinder out of libgcc — around
/// 5 KB of `__gnu_Unwind_*`, including WMMX register save/restore, into a
/// program with no exceptions.
#[test]
fn no_unwind_personality_dependency() {
    let spec = TargetSpec::new("thumbv7em-none-eabihf");
    let bytes = object_bytes(FLOAT_MATH_ST, &spec, "no_unwind");

    let undef = undefined_symbols(&bytes);
    assert!(
        !undef.iter().any(|n| n.contains("unwind")),
        "object depends on the unwinder: {undef:?}"
    );
}

/// Every function gets its own ELF section so `--gc-sections` can drop the
/// standard function blocks a program never instantiates.
#[test]
fn elf_functions_get_individual_sections() {
    let spec = TargetSpec::new("thumbv7em-none-eabihf");
    let bytes = object_bytes(BLINK_ST, &spec, "func_sections");

    let sections = section_names(&bytes);
    for want in [".text.blink_scan", ".text.blink_init"] {
        assert!(
            sections.iter().any(|s| s == want),
            "missing section {want}; got {sections:?}"
        );
    }
}

/// wasm-ld collects unreachable functions itself, and the wasm object format has
/// no ELF sections to name — so the ELF-only pass must leave it alone.
///
/// Checked by scanning the raw bytes rather than parsing: reading wasm through
/// the `object` crate needs its `wasm` feature and a `wasmparser` dependency,
/// which is a lot of machinery to confirm that a string is absent.
#[test]
fn non_elf_targets_skip_function_sections() {
    let spec = TargetSpec::new("wasm32-unknown-unknown");
    let bytes = object_bytes(BLINK_ST, &spec, "wasm_sections");

    assert_eq!(&bytes[..4], b"\0asm", "expected a wasm object");
    let leaked = bytes
        .windows(b".text.blink".len())
        .any(|w| w == b".text.blink");
    assert!(!leaked, "ELF section naming leaked into a wasm object");
}

/// A hard-float triple paired with an FPU-less part is an ABI mismatch that
/// links cleanly and computes garbage. Refuse it instead.
#[test]
fn hard_float_triple_rejects_fpu_less_cpu() {
    let spec = TargetSpec::new("thumbv7em-none-eabihf").with_cpu("cortex-m3");
    let err = spec
        .validate()
        .expect_err("expected hard-float + FPU-less CPU to be rejected");
    assert!(
        err.to_string().contains("hard-float"),
        "unexpected error: {err}"
    );
}

/// Soft-float triples are not an error — those parts really have no FPU.
#[test]
fn soft_float_triples_accept_fpu_less_cpus() {
    for triple in ["thumbv6m-none-eabi", "thumbv7m-none-eabi"] {
        TargetSpec::new(triple)
            .validate()
            .unwrap_or_else(|e| panic!("{triple} should be valid: {e}"));
    }
}

/// The CPU default must not override an explicit choice.
#[test]
fn explicit_cpu_and_features_win() {
    let spec = TargetSpec::new("thumbv7em-none-eabihf")
        .with_cpu("cortex-m7")
        .with_features("+fp64");
    assert_eq!(spec.cpu, "cortex-m7");
    assert_eq!(spec.features, "+fp64");

    let bytes = object_bytes(FLOAT_MATH_ST, &spec, "explicit_cpu");
    assert!(!bytes.is_empty());
}
