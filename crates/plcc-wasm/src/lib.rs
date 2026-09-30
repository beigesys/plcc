// SPDX-License-Identifier: MPL-2.0

//! plcc's front end for the browser: parse and type-check Structured Text,
//! PLCopen XML, Rockwell L5X and TwinCAT projects held in memory, with
//! structured diagnostics and a tag outline. No LLVM: code generation is not
//! part of this module (see docs/studio-wasm.md).
//!
//! The interface is JSON in, JSON out, so the JavaScript side
//! (`packages/plcc-wasm`) owns the TypeScript types.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use wasm_bindgen::prelude::*;

/// `check()` request.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub files: BTreeMap<String, String>,
    #[serde(default)]
    pub entry: Option<Vec<String>>,
    #[serde(default)]
    pub io_map: Option<String>,
    /// `"bundled-st"` (default) or `"none"`.
    #[serde(default)]
    pub stdlib: Option<String>,
    /// Include the tag outline in the response (default true).
    #[serde(default)]
    pub tags: Option<bool>,
}

#[derive(Serialize)]
pub struct Response {
    pub ok: bool,
    pub diagnostics: Vec<plcc_driver::Diagnostic>,
    pub tags: Option<plcc_driver::tags::Tags>,
    /// Declarations in the user's files (bundled libraries excluded).
    pub declarations: usize,
}

/// Run a request (the JSON-free core, for native tests).
pub fn run(req: Request) -> Response {
    let no_stdlib = match req.stdlib.as_deref() {
        None | Some("bundled-st") => false,
        Some("none") => true,
        Some(other) => {
            return Response {
                ok: false,
                diagnostics: vec![plcc_driver::Diagnostic::plain(
                    None,
                    plcc_driver::Stage::Input,
                    plcc_driver::Severity::Error,
                    format!("stdlib: `{other}` (expected \"bundled-st\" or \"none\")"),
                )],
                tags: None,
                declarations: 0,
            };
        }
    };
    let project = plcc_driver::Project {
        files: req.files,
        entry: req.entry,
        io_map: req.io_map,
        no_stdlib,
    };
    let checked = plcc_driver::check(&project);
    let parsed = checked.parsed.as_ref();
    Response {
        ok: checked.ok,
        tags: if req.tags.unwrap_or(true) {
            parsed.map(plcc_driver::tags::tags)
        } else {
            None
        },
        declarations: parsed.map_or(0, |p| p.origins.iter().filter(|o| !o.prelude).count()),
        diagnostics: checked.diagnostics,
    }
}

fn error_response(message: String) -> String {
    let r = Response {
        ok: false,
        diagnostics: vec![plcc_driver::Diagnostic::plain(
            None,
            plcc_driver::Stage::Input,
            plcc_driver::Severity::Error,
            message,
        )],
        tags: None,
        declarations: 0,
    };
    serde_json::to_string(&r).unwrap_or_default()
}

/// Parse and type-check a project. `request` is JSON:
/// `{ files: { path: text }, entry?: [path], io_map?: toml, stdlib?: "bundled-st" | "none", tags?: bool }`.
/// Returns JSON `{ ok, diagnostics: [...], tags, declarations }`.
#[wasm_bindgen]
pub fn check(request: &str) -> String {
    match serde_json::from_str::<Request>(request) {
        Ok(req) => serde_json::to_string(&run(req))
            .unwrap_or_else(|e| error_response(format!("internal: {e}"))),
        Err(e) => error_response(format!("malformed request: {e}")),
    }
}

/// The AST of one file as JSON (`plcc parse --dump-ast`), or `null` plus
/// diagnostics: `{ ast, diagnostics }`.
#[wasm_bindgen]
pub fn parse(path: &str, text: &str) -> String {
    #[derive(Serialize)]
    struct Out {
        ast: Option<plcc_driver::CompilationUnit>,
        diagnostics: Vec<plcc_driver::Diagnostic>,
    }
    if let Err(e) = plcc_driver::validate_path(path) {
        return error_response(e);
    }
    let (unit, diagnostics) =
        plcc_driver::parse_file(path, text, &plcc_l5x_options_default());
    let failed = diagnostics
        .iter()
        .any(|d| d.severity == plcc_driver::Severity::Error);
    serde_json::to_string(&Out {
        ast: (!failed).then_some(unit),
        diagnostics,
    })
    .unwrap_or_default()
}

fn plcc_l5x_options_default() -> plcc_driver::L5xOptions {
    plcc_driver::L5xOptions::default()
}

/// `convert()` request.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConvertRequest {
    pub files: BTreeMap<String, String>,
    #[serde(default)]
    pub entry: Option<Vec<String>>,
    /// `"st"`, `"plcopen"`, `"l5x"` or `"ladder-json"`.
    pub to: String,
    /// Ladder dialect of ladder-json / st output: `"iec"` or `"logix"`.
    #[serde(default)]
    pub dialect: Option<String>,
    /// With L5X input and `to: "st"`: append the Logix prelude.
    #[serde(default)]
    pub prelude: bool,
}

#[derive(Serialize)]
pub struct ConvertResponse {
    pub ok: bool,
    pub output: Option<String>,
    pub diagnostics: Vec<plcc_driver::Diagnostic>,
}

fn dialect(s: &str) -> Option<plcc_driver::Dialect> {
    match s {
        "iec" => Some(plcc_driver::Dialect::Iec),
        "logix" => Some(plcc_driver::Dialect::Logix),
        _ => None,
    }
}

fn input_error(message: String) -> plcc_driver::Diagnostic {
    plcc_driver::Diagnostic::plain(
        None,
        plcc_driver::Stage::Input,
        plcc_driver::Severity::Error,
        message,
    )
}

/// Convert without JSON (for native tests).
pub fn run_convert(req: ConvertRequest) -> ConvertResponse {
    let bad = |m: String| ConvertResponse {
        ok: false,
        output: None,
        diagnostics: vec![input_error(m)],
    };
    let Some(to) = plcc_driver::convert::Format::parse(&req.to) else {
        return bad(format!(
            "to: `{}` (expected \"st\", \"plcopen\", \"l5x\" or \"ladder-json\")",
            req.to
        ));
    };
    let d = match req.dialect.as_deref() {
        None => None,
        Some(s) => match dialect(s) {
            Some(d) => Some(d),
            None => return bad(format!("dialect: `{s}` (expected \"iec\" or \"logix\")")),
        },
    };
    let project = plcc_driver::Project {
        files: req.files,
        entry: req.entry,
        ..Default::default()
    };
    let c = plcc_driver::convert::convert(&project, to, d, req.prelude);
    ConvertResponse {
        ok: c.output.is_some(),
        output: c.output,
        diagnostics: c.diagnostics,
    }
}

/// Convert a program between notations (`plcc convert`). `request` is JSON:
/// `{ files, entry?, to: "st" | "plcopen" | "l5x" | "ladder-json", dialect?: "iec" | "logix", prelude?: bool }`.
/// Returns JSON `{ ok, output, diagnostics }`; translation warnings are
/// diagnostics with `stage: "convert"`.
#[wasm_bindgen]
pub fn convert(request: &str) -> String {
    let r = match serde_json::from_str::<ConvertRequest>(request) {
        Ok(req) => run_convert(req),
        Err(e) => ConvertResponse {
            ok: false,
            output: None,
            diagnostics: vec![input_error(format!("malformed request: {e}"))],
        },
    };
    serde_json::to_string(&r).unwrap_or_default()
}

/// The ladder instruction catalog (pins, roles, categories) for `"iec"` or
/// `"logix"`, as JSON (`plcc_ladder::catalog::all`); `null` for another dialect.
#[wasm_bindgen]
pub fn catalog(dialect_name: &str) -> Option<String> {
    dialect(dialect_name).map(plcc_driver::convert::catalog_json)
}

/// `null` if `path` is acceptable as a project path, else why not.
#[wasm_bindgen]
pub fn validate_path(path: &str) -> Option<String> {
    plcc_driver::validate_path(path).err()
}

/// The plcc version this module was built from.
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip() {
        let req = r#"{"files":{"a.st":"PROGRAM P VAR x : INT; END_VAR x := TRUE + 1; END_PROGRAM"}}"#;
        let out: serde_json::Value = serde_json::from_str(&check(req)).unwrap();
        assert_eq!(out["ok"], false);
        assert_eq!(out["diagnostics"][0]["file"], "a.st");
        assert_eq!(out["diagnostics"][0]["span"]["start"]["line"], 1);

        let out: serde_json::Value = serde_json::from_str(&check("{nope")).unwrap();
        assert_eq!(out["ok"], false);
        assert_eq!(out["diagnostics"][0]["stage"], "input");
    }

    #[test]
    fn convert_st_to_ladder_and_back() {
        let st = "PROGRAM P VAR a : BOOL; b : BOOL; q : BOOL; END_VAR q := a AND NOT b; END_PROGRAM";
        let req = format!(
            r#"{{"files":{{"p.st":{}}},"to":"ladder-json"}}"#,
            serde_json::to_string(st).unwrap()
        );
        let out: serde_json::Value = serde_json::from_str(&convert(&req)).unwrap();
        assert_eq!(out["ok"], true, "{out}");
        let json = out["output"].as_str().unwrap().to_string();
        let req = format!(
            r#"{{"files":{{"p.json":{}}},"to":"l5x"}}"#,
            serde_json::to_string(&json).unwrap()
        );
        let out: serde_json::Value = serde_json::from_str(&convert(&req)).unwrap();
        assert_eq!(out["ok"], true, "{out}");
        assert!(out["output"].as_str().unwrap().contains("RSLogix5000Content"));
        assert!(catalog("logix").unwrap().contains("XIC"));
        assert!(catalog("x").is_none());
    }
}
