// SPDX-License-Identifier: MPL-2.0

//! Cross-compilation tests: verify we can emit object code for multiple target triples.

use inkwell::context::Context;
use inkwell::targets::{InitializationConfig, Target};
use plcc_codegen::Compiler;

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
