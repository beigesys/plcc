// SPDX-License-Identifier: MPL-2.0

//! The browser compiler (`plcc-build`, which `packages/plcc-compiler-wasm`
//! runs) and `plcc compile` must produce the same object and symbol table for
//! the same inputs and options: one compiler, no drift.

use std::path::Path;
use std::process::Command;

const PLCC: &str = env!("CARGO_BIN_EXE_plcc");

fn root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

struct Case<'a> {
    input: &'a str,
    target: Option<&'a str>,
    device: Option<&'a str>,
    io_map: Option<&'a str>,
    opt: u8,
}

fn check(case: Case) {
    let out = std::env::temp_dir().join(format!(
        "plcc_build_parity_{}_{}",
        std::process::id(),
        case.input.replace('/', "_")
    ));
    let obj = out.with_extension("o");
    let sym = out.with_extension("json");
    let mut cmd = Command::new(PLCC);
    cmd.current_dir(root())
        .args(["compile", case.input, "-o"])
        .arg(&obj)
        .arg("--emit-symbols")
        .arg(&sym)
        .args(["-O", &case.opt.to_string()]);
    if let Some(t) = case.target {
        cmd.args(["--target", t]);
    }
    if let Some(d) = case.device {
        cmd.args(["--device", d]);
    }
    if let Some(m) = case.io_map {
        cmd.args(["--io-map", m]);
    }
    let o = cmd.output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));

    let read = |p: &str| std::fs::read_to_string(root().join(p)).unwrap();
    let req = plcc_build::Request {
        files: [(case.input.to_string(), read(case.input))].into(),
        entry: Some(vec![case.input.to_string()]),
        io_map: case.io_map.map(read),
        target: case.target.map(str::to_string),
        device: case.device.map(read),
        opt_level: case.opt,
        ..Default::default()
    };
    let built = plcc_build::compile(&req);
    assert!(built.ok, "{:#?}", built.diagnostics);
    let native = std::fs::read(&obj).unwrap();
    assert!(
        built.object.as_deref() == Some(&native[..]),
        "{}: objects differ",
        case.input
    );
    let want: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&sym).unwrap()).unwrap();
    assert_eq!(built.symbols.as_ref(), Some(&want), "{}: symbols differ", case.input);
    let _ = std::fs::remove_file(obj);
    let _ = std::fs::remove_file(sym);
}


/// One test, in this order: within one process, a WebAssembly build changes
/// how later Arm builds lower `CASE` (jump tables) — LLVM keeps state across
/// target machines. `plcc compile` runs one build per process, and so does
/// the browser compiler (a fresh instance per request); here the Arm builds
/// go first so each comparison sees a clean LLVM.
#[test]
fn plcc_build_matches_plcc_compile() {
    let programs = ["blink", "pid_simple", "state_machine", "water_treatment"];
    check(Case {
        input: "tests/fixtures/l5x/opta_io.L5X",
        target: None,
        device: Some("crates/plcc-device/builtin/arduino-opta.toml"),
        io_map: Some("tests/fixtures/l5x/opta_io.toml"),
        opt: 2,
    });
    for (target, opt) in [("thumbv7em-none-eabi", 0), ("wasm32-unknown-unknown", 2)] {
        for p in programs {
            check(Case {
                input: &format!("tests/fixtures/programs/{p}.st"),
                target: Some(target),
                device: None,
                io_map: None,
                opt,
            });
        }
    }
}
