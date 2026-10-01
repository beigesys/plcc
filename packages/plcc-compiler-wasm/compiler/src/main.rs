// SPDX-License-Identifier: MPL-2.0

//! plcc's compiler as a WASI command for the browser: one JSON request on
//! stdin, one JSON response on stdout (no files, no arguments).
//!
//! Request: `plcc_build::Request` (files, entry, io_map, stdlib, device,
//! target, cpu, features, float_abi, image, opt_level, task_interval) plus
//! `link: true` to link a wasm32 object into a module with plcc-wasm-link
//! (what `wasm-ld --no-entry --export-dynamic --allow-undefined
//! --export-table` makes for the simulator).
//!
//! Response: `{ ok, diagnostics, symbols, target, timings, object, module }`
//! with `object` / `module` base64.
//!
//! Run a fresh instance per request: LLVM keeps state between builds in one
//! process (see crates/plcc-cli/tests/build_parity.rs), and a panic aborts.

use std::io::{Read, Write};

fn main() {
    let mut input = String::new();
    let response = match std::io::stdin().read_to_string(&mut input) {
        Ok(_) => run(&input),
        Err(e) => error(format!("reading the request: {e}")),
    };
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(response.to_string().as_bytes());
    let _ = out.flush();
}

fn error(message: String) -> serde_json::Value {
    let d = plcc_driver::Diagnostic::plain(
        None,
        plcc_driver::Stage::Input,
        plcc_driver::Severity::Error,
        message,
    );
    serde_json::json!({ "ok": false, "diagnostics": [d] })
}

fn run(input: &str) -> serde_json::Value {
    let mut value: serde_json::Value = match serde_json::from_str(input) {
        Ok(v) => v,
        Err(e) => return error(format!("malformed request: {e}")),
    };
    let link = value
        .as_object_mut()
        .and_then(|o| o.remove("link"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let req: plcc_build::Request = match serde_json::from_value(value) {
        Ok(r) => r,
        Err(e) => return error(format!("malformed request: {e}")),
    };
    let built = plcc_build::compile(&req);
    let mut response = match serde_json::to_value(&built) {
        Ok(v) => v,
        Err(e) => return error(format!("internal: {e}")),
    };
    let Some(obj) = response.as_object_mut() else {
        return error("internal: response is not an object".into());
    };
    if let Some(object) = &built.object {
        obj.insert("object".into(), plcc_build::base64(object).into());
        if link && built.target.starts_with("wasm32") {
            let t = std::time::Instant::now();
            match plcc_wasm_link::link(object, &plcc_wasm_link::Options::default()) {
                Ok(module) => {
                    obj.insert("module".into(), plcc_build::base64(&module).into());
                }
                Err(e) => {
                    let d = plcc_driver::Diagnostic::plain(
                        None,
                        plcc_driver::Stage::Codegen,
                        plcc_driver::Severity::Error,
                        format!("linking the WebAssembly module: {e}"),
                    );
                    obj.insert("ok".into(), false.into());
                    if let Some(serde_json::Value::Array(ds)) = obj.get_mut("diagnostics") {
                        ds.push(serde_json::to_value(d).unwrap_or_default());
                    }
                }
            }
            if let Some(serde_json::Value::Object(t2)) = obj.get_mut("timings") {
                t2.insert("link_ms".into(), (t.elapsed().as_secs_f64() * 1000.0).into());
            }
        }
    }
    response
}
