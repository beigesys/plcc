// SPDX-License-Identifier: MPL-2.0

//! `plcc compile` over in-memory files: the front end of [`plcc_driver`]
//! (every input format, ladder models included), the type checker, LLVM code
//! generation and the runtime contract's symbol table, with structured
//! diagnostics. No file system: this is what the browser compiler
//! (`packages/plcc-compiler-wasm`) runs, built for `wasm32-wasip1` against
//! LLVM compiled to WebAssembly, and it follows `plcc compile`
//! (`crates/plcc-cli`) option for option so both produce the same object.

use plcc_codegen::direct_address::Area;
use plcc_driver::{Diagnostic, Severity, Stage};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What to build.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// `path → text`, as for `check`.
    pub files: BTreeMap<String, String>,
    #[serde(default)]
    pub entry: Option<Vec<String>>,
    /// L5X I/O map (TOML).
    #[serde(default)]
    pub io_map: Option<String>,
    /// `"bundled-st"` (default) or `"none"`.
    #[serde(default)]
    pub stdlib: Option<String>,
    /// A device manifest (TOML text, docs/device-manifest.md): the target,
    /// CPU, features, float ABI and image sizes the fields below leave out.
    #[serde(default)]
    pub device: Option<String>,
    /// LLVM target triple; default: the device's, else `wasm32-unknown-unknown`.
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub cpu: Option<String>,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub float_abi: Option<String>,
    /// Process-image sizes in bytes, by area (`"I"`, `"Q"`, `"M"`).
    #[serde(default)]
    pub image: BTreeMap<String, u32>,
    /// 0 (no IR optimization) to 3.
    #[serde(default)]
    pub opt_level: u8,
    /// Interval of the implicit task (default `T#20ms`).
    #[serde(default)]
    pub task_interval: Option<String>,
}

/// What came out.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Output {
    pub ok: bool,
    pub diagnostics: Vec<Diagnostic>,
    /// The relocatable object.
    #[serde(skip)]
    pub object: Option<Vec<u8>>,
    /// The symbol table (`--emit-symbols`), JSON.
    pub symbols: Option<serde_json::Value>,
    /// The triple the object is for.
    pub target: String,
    /// Milliseconds spent: front end + type check, code generation,
    /// optimization + emission.
    pub timings: Timings,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Timings {
    pub check_ms: f64,
    pub codegen_ms: f64,
    pub emit_ms: f64,
}

fn error(stage: Stage, message: String) -> Diagnostic {
    Diagnostic::plain(None, stage, Severity::Error, message)
}

fn has_error(d: &[Diagnostic]) -> bool {
    d.iter().any(|d| d.severity == Severity::Error)
}

/// A clock that works where `std::time::Instant` may not (it does on WASI).
fn now() -> std::time::Instant {
    std::time::Instant::now()
}

fn ms(since: std::time::Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// Compile a request: check, generate code, emit the object and symbols.
pub fn compile(req: &Request) -> Output {
    let mut out = Output::default();
    let fail = |mut out: Output, d: Diagnostic| {
        out.diagnostics.push(d);
        out
    };

    // The device manifest supplies what the request leaves out.
    let device = match &req.device {
        None => None,
        Some(text) => {
            let c = plcc_device::load(text, Some("device.toml"));
            match c.device {
                Some(d) => Some(d),
                None => {
                    for d in c.diagnostics {
                        out.diagnostics.push(error(Stage::Codegen, format!("device manifest: {d}")));
                    }
                    return out;
                }
            }
        }
    };
    if let Some(d) = &device {
        let abi = plcc_codegen::compiler::contract::ABI_VERSION;
        if d.target.runtime.abi != abi {
            return fail(
                out,
                error(
                    Stage::Codegen,
                    format!(
                        "the {} runtime of {} implements runtime ABI {}, this plcc compiles for ABI {abi}",
                        d.target.runtime.kind, d.device.id, d.target.runtime.abi
                    ),
                ),
            );
        }
    }
    let target = req
        .target
        .clone()
        .or_else(|| device.as_ref().map(|d| d.target.triple.clone()))
        .unwrap_or_else(|| "wasm32-unknown-unknown".to_string());
    out.target = target.clone();
    let cpu = req
        .cpu
        .clone()
        .or_else(|| device.as_ref().and_then(|d| d.target.cpu.clone()));
    let features = if req.features.is_empty() {
        device
            .as_ref()
            .map(|d| d.target.features.clone())
            .unwrap_or_default()
    } else {
        req.features.clone()
    };
    let float_abi = req.float_abi.clone().or_else(|| {
        device
            .as_ref()
            .and_then(|d| d.target.float_abi.map(|a| a.as_str().to_string()))
    });
    let mut image: Vec<(Area, u32)> = Vec::new();
    for (a, n) in &req.image {
        let area = match a.to_ascii_uppercase().as_str() {
            "I" => Area::Input,
            "Q" => Area::Output,
            "M" => Area::Memory,
            other => {
                return fail(out, error(Stage::Input, format!("image: unknown area `{other}` (I, Q or M)")));
            }
        };
        image.push((area, *n));
    }
    if let Some(d) = &device {
        let img = d.target.image;
        for (area, bytes) in [(Area::Input, img.i), (Area::Output, img.q), (Area::Memory, img.m)] {
            if !image.iter().any(|(a, _)| *a == area) {
                image.push((area, bytes));
            }
        }
    }
    let machine = match machine_options(&target, cpu.as_deref(), &features, float_abi.as_deref()) {
        Ok(m) => m,
        Err(e) => return fail(out, error(Stage::Codegen, e)),
    };
    let interval_text = req.task_interval.as_deref().unwrap_or("T#20ms");
    let Some(interval) = plcc_codegen::compiler::contract::parse_duration_ns(interval_text) else {
        return fail(
            out,
            error(Stage::Input, format!("task_interval: `{interval_text}` is not a TIME")),
        );
    };
    let no_stdlib = match req.stdlib.as_deref() {
        None | Some("bundled-st") => false,
        Some("none") => true,
        Some(other) => {
            return fail(
                out,
                error(Stage::Input, format!("stdlib: `{other}` (expected \"bundled-st\" or \"none\")")),
            );
        }
    };

    // Front end and type check: exactly `check`.
    let t0 = now();
    let project = plcc_driver::Project {
        files: req.files.clone(),
        entry: req.entry.clone(),
        io_map: req.io_map.clone(),
        no_stdlib,
    };
    let checked = plcc_driver::check(&project);
    out.diagnostics = checked.diagnostics;
    out.timings.check_ms = ms(t0);
    let Some(parsed) = checked.parsed.filter(|_| checked.ok) else {
        return out;
    };

    // Code generation, as `plcc compile`.
    let t1 = now();
    let context = inkwell::context::Context::create();
    let name = req
        .entry
        .as_ref()
        .and_then(|e| e.first())
        .or_else(|| req.files.keys().next())
        .cloned()
        .unwrap_or_else(|| "program".into());
    let mut compiler = plcc_codegen::Compiler::new(&context, &name);
    compiler.set_machine_options(machine);
    for (area, bytes) in image {
        compiler.set_image_size(area, bytes);
    }
    register_sources(&mut compiler, &parsed);
    compiler.set_task_options(plcc_codegen::TaskOptions {
        default_interval_ns: interval,
    });
    if let Err(e) = compiler.compile(&parsed.unit) {
        out.diagnostics.push(codegen_error(&e, &project, &parsed));
        return out;
    }
    if let Err(e) = compiler.set_target(&target) {
        return fail(out, error(Stage::Codegen, e.to_string()));
    }
    out.timings.codegen_ms = ms(t1);
    let t2 = now();
    if req.opt_level > 0
        && let Err(e) = compiler.optimize(&target, req.opt_level)
    {
        return fail(out, error(Stage::Codegen, e.to_string()));
    }
    match compiler.runtime_contract(&target) {
        Ok(mut contract) => {
            contract.device = device.as_ref().map(|d| plcc_codegen::DeviceStamp {
                id: d.device.id.clone(),
                version: d.device.version,
            });
            out.symbols =
                serde_json::from_str(&plcc_codegen::header::symbols_json(&contract)).ok();
        }
        Err(e) => return fail(out, error(Stage::Codegen, e.to_string())),
    }
    match compiler.emit_object_bytes(&target) {
        Ok(bytes) => out.object = Some(bytes),
        Err(e) => return fail(out, error(Stage::Codegen, e.to_string())),
    }
    out.timings.emit_ms = ms(t2);
    out.ok = !has_error(&out.diagnostics);
    out
}

/// A code generation error, placed in its file when it carries a span and
/// the program has one input.
fn codegen_error(
    e: &plcc_codegen::compiler::CodegenError,
    project: &plcc_driver::Project,
    parsed: &plcc_driver::Parsed,
) -> Diagnostic {
    let mut d = error(Stage::Codegen, e.to_string());
    let user: Vec<&plcc_driver::Origin> = parsed.origins.iter().filter(|o| !o.prelude).collect();
    if let (Some(span), Some(first)) = (e.span(), user.first())
        && user.iter().all(|o| o.name == first.name)
        && span.end <= first.source.len()
    {
        let ix = plcc_driver::LineIndex::new(&first.source);
        let label = plcc_driver::Label {
            start: ix.position(span.start),
            end: ix.position(span.end),
            message: None,
        };
        d.file = Some(first.name.clone());
        d.span = Some(label.clone());
        d.labels = vec![label];
        plcc_driver::remap_ladder(project, std::slice::from_mut(&mut d));
    }
    d
}

/// `--cpu`, `--features`, `--float-abi` as `plcc compile` reads them.
pub fn machine_options(
    triple: &str,
    cpu: Option<&str>,
    features: &[String],
    float_abi: Option<&str>,
) -> Result<plcc_codegen::MachineOptions, String> {
    let mut feats: Vec<String> = Vec::new();
    for f in features.iter().map(|f| f.trim()).filter(|f| !f.is_empty()) {
        if !(f.starts_with('+') || f.starts_with('-')) || f.len() < 2 {
            return Err(format!("features: `{f}` is not a target feature; write +name or -name"));
        }
        feats.push(f.to_string());
    }
    if let Some(abi) = float_abi {
        let abi = plcc_device::FloatAbi::parse(abi)
            .ok_or_else(|| format!("float_abi: `{abi}` is not soft, softfp or hard"))?;
        let extra = abi
            .features_for(triple)
            .map_err(|e| format!("float_abi: {e}"))?;
        feats.extend(extra.into_iter().map(str::to_string));
    }
    Ok(plcc_codegen::MachineOptions {
        cpu: cpu
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .unwrap_or("generic")
            .to_string(),
        features: feats.join(","),
    })
}

/// Which file each POU of the merged unit came from (fault sites name
/// `file:line:col`), and Logix array bounds faults, as `plcc compile`.
fn register_sources(compiler: &mut plcc_codegen::Compiler<'_>, parsed: &plcc_driver::Parsed) {
    let mut files: Vec<(&plcc_driver::Origin, Vec<String>)> = Vec::new();
    for (decl, origin) in parsed.unit.declarations.iter().zip(&parsed.origins) {
        let Some(name) = plcc_driver::declaration_name(decl) else {
            continue;
        };
        match files
            .iter_mut()
            .find(|(o, _)| std::rc::Rc::ptr_eq(&o.source, &origin.source))
        {
            Some((_, pous)) => pous.push(name),
            None => files.push((origin, vec![name])),
        }
    }
    for (origin, pous) in files {
        if origin.logix {
            compiler.fault_on_array_bounds(&pous);
        }
        compiler.add_source_file(&origin.name, &origin.source, pous);
    }
}

/// Base64 (RFC 4648, padded), for binary outputs in JSON.
pub fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16
            | (c.get(1).copied().unwrap_or(0) as u32) << 8
            | c.get(2).copied().unwrap_or(0) as u32;
        s.push(A[(n >> 18) as usize & 63] as char);
        s.push(A[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { A[n as usize & 63] as char } else { '=' });
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
