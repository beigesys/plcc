// SPDX-License-Identifier: MPL-2.0

//! The module carries its target's triple and data layout.
//!
//! It carried neither, so an emitted `.ll`/`.bc` optimized later (`opt -O2`,
//! `clang -O2 prog.ll`) had its struct accesses folded into byte offsets with
//! LLVM's default layout, where an i64 is 4-byte aligned: a LINT after a DINT
//! moved from offset 8 to 4 — no longer where the generated C header (and the
//! runtime) put it.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::passes::PassBuilderOptions;
use inkwell::targets::{CodeModel, InitializationConfig, RelocMode, Target, TargetMachine};
use plcc_codegen::Compiler;

#[test]
fn optimized_module_keeps_the_contract_layout() {
    let src = r#"
PROGRAM p
VAR a : DINT; l : LINT; END_VAR
a := 1;
l := 16#1122334455667788;
END_PROGRAM
"#;
    let (unit, errors) = plcc_st::parse(src);
    assert!(errors.is_empty(), "{errors:?}");
    let ctx = Context::create();
    let mut c = Compiler::new(&ctx, "layout");
    c.compile(&unit).expect("codegen");
    let triple = TargetMachine::get_default_triple();
    let triple_str = triple.as_str().to_string_lossy().into_owned();
    c.set_target(&triple_str).expect("set_target");
    let ir = c.emit_ir();
    assert!(ir.contains("target datalayout"), "no data layout in:\n{ir}");
    assert!(ir.contains("target triple"), "no triple in:\n{ir}");

    let contract = c.runtime_contract(&triple_str).expect("contract");
    let ty = &contract.types[contract.programs[0].type_index];
    let off = ty.fields.iter().find(|f| f.name == "l").expect("l").offset as usize;

    // What an external `opt -O3` does to the emitted module.
    Target::initialize_native(&InitializationConfig::default()).unwrap();
    let tm = Target::from_triple(&triple)
        .unwrap()
        .create_target_machine(
            &triple,
            "generic",
            "",
            OptimizationLevel::Aggressive,
            RelocMode::Default,
            CodeModel::Default,
        )
        .unwrap();
    c.module()
        .run_passes("default<O3>", &tm, PassBuilderOptions::create())
        .unwrap();
    let ee = c
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .unwrap();
    let mut state = vec![0u8; 64];
    unsafe {
        let f: extern "C" fn(*mut u8) =
            std::mem::transmute(ee.get_function_address("p_scan").unwrap());
        f(state.as_mut_ptr());
    }
    let l = i64::from_ne_bytes(state[off..off + 8].try_into().unwrap());
    assert_eq!(l, 0x1122334455667788, "LINT written at the header's offset {off}");
}
