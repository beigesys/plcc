// SPDX-License-Identifier: MPL-2.0

//! Device manifests: one TOML file per device describing the board, what its
//! runtime exposes (target, process image, console, Modbus, I/O terminals) and
//! how to flash it. The spec is docs/device-manifest.md.
//!
//! Pure Rust with no LLVM, so it builds for `wasm32-unknown-unknown` too.
//!
//! ```
//! let text = plcc_device::catalog::builtin_source("arduino-opta").unwrap();
//! let checked = plcc_device::load(text, Some("arduino-opta.toml"));
//! let device = checked.device.unwrap();
//! assert_eq!(device.target.image.i, 18);
//! assert!(device.io.iter().any(|p| p.terminal == "I1" && p.address == "%IX0.0"));
//! ```
//!
//! Manifests are data from outside the compiler (a download, a project file):
//! nothing here trusts them for safety. Flash limits in particular are only
//! ever narrowed by a manifest, never widened (packages/webdfu enforces its own
//! per-USB-id floors).

pub mod catalog;
pub mod diag;
pub mod expr;
pub mod model;
pub mod validate;

pub use diag::{Diagnostic, Severity};
pub use model::*;
pub use validate::{Checked, load, parse, validate};

/// The JSON Schema of the manifest file format, for editors (taplo, VS Code
/// Even Better TOML). docs/device-manifest.schema.json is this, committed.
pub fn json_schema() -> String {
    let schema = schemars::schema_for!(Manifest);
    let mut value = schema.to_value();
    drop_null_alternatives(&mut value);
    let mut s = serde_json::to_string_pretty(&value).unwrap_or_default();
    s.push('\n');
    s
}

/// TOML has no null: an optional key is simply absent. schemars writes an
/// `Option<T>` as `anyOf: [T, {type: null}]`; keep only `T`.
fn drop_null_alternatives(v: &mut serde_json::Value) {
    use serde_json::Value;
    match v {
        Value::Object(map) => {
            if let Some(Value::Array(alts)) = map.get("anyOf") {
                let rest: Vec<Value> = alts
                    .iter()
                    .filter(|a| a.get("type").and_then(Value::as_str) != Some("null"))
                    .cloned()
                    .collect();
                if rest.len() == 1 && rest.len() < alts.len() {
                    map.remove("anyOf");
                    if let Value::Object(inner) = &rest[0] {
                        for (k, val) in inner {
                            map.entry(k.clone()).or_insert_with(|| val.clone());
                        }
                    }
                }
            }
            if let Some(Value::Array(types)) = map.get_mut("type") {
                types.retain(|t| t.as_str() != Some("null"));
                if types.len() == 1 {
                    let t = types[0].clone();
                    map.insert("type".into(), t);
                }
            }
            for val in map.values_mut() {
                drop_null_alternatives(val);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(drop_null_alternatives),
        _ => {}
    }
}
