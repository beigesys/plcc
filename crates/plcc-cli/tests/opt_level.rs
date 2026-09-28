// SPDX-License-Identifier: MPL-2.0

//! `plcc compile -O<n>` runs LLVM's optimization pipeline, and every output
//! carries the target's triple and data layout (so a LINT after a DINT stays at
//! the offset the C header gives it, optimized or not).

use std::process::Command;

const PLCC: &str = env!("CARGO_BIN_EXE_plcc");

#[test]
fn optimized_ir_keeps_the_header_layout() {
    let d = std::env::temp_dir().join(format!("plcc_opt_level_{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let st = d.join("p.st");
    std::fs::write(
        &st,
        "PROGRAM p VAR a : DINT; l : LINT; END_VAR l := 16#1122334455667788; END_PROGRAM\n",
    )
    .unwrap();
    for (flag, name) in [("-O0", "o0.ll"), ("-O2", "o2.ll"), ("-O3", "o3.ll")] {
        let ll = d.join(name);
        let out = Command::new(PLCC)
            .args(["compile", "--stdlib", "none", flag])
            .arg(&st)
            .arg("-o")
            .arg(&ll)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let ir = std::fs::read_to_string(&ll).unwrap();
        assert!(ir.contains("target datalayout"), "{flag}: no data layout");
        if flag != "-O0" {
            // Optimized: the store is folded to a byte offset, which must be 8.
            assert!(
                ir.contains("getelementptr inbounds nuw i8, ptr %0, i64 8"),
                "{flag}:\n{ir}"
            );
        }
    }
    let bad = Command::new(PLCC)
        .args(["compile", "-O7"])
        .arg(&st)
        .arg("-o")
        .arg(d.join("x.ll"))
        .output()
        .unwrap();
    assert!(!bad.status.success());
}
