// SPDX-License-Identifier: MPL-2.0

//! `plcc device check|list` and `plcc compile --device`: device manifests
//! (docs/device-manifest.md).

use miette::{IntoDiagnostic, NamedSource, Result};
use plcc_device::catalog::{self, Origin};
use plcc_device::{Device, Diagnostic};
use std::path::{Path, PathBuf};

/// A manifest named on the command line: a file, or a catalog id.
pub struct Loaded {
    pub device: Device,
    /// What to call it in messages (the file path, or `<builtin>/x.toml`).
    pub name: String,
}

fn print_diagnostics(name: &str, source: &str, diags: &[Diagnostic]) {
    let shared = std::sync::Arc::new(source.to_string());
    for d in diags {
        let report = miette::Report::new(d.clone())
            .with_source_code(NamedSource::new(name, shared.clone()));
        eprintln!("{report:?}");
    }
}

/// Load and validate a manifest; diagnostics are printed, and an invalid
/// manifest is an error.
fn load_text(name: &str, source: &str) -> Result<Device> {
    let checked = plcc_device::load(source, Some(name));
    print_diagnostics(name, source, &checked.diagnostics);
    let n = checked.errors().count();
    checked
        .device
        .ok_or_else(|| miette::miette!("{name}: {n} error(s) in the device manifest"))
}

/// `--device ARG`: an existing file, else a catalog id.
pub fn resolve(arg: &str) -> Result<Loaded> {
    let path = Path::new(arg);
    if path.is_file() {
        let source = std::fs::read_to_string(path).into_diagnostic()?;
        let name = path.display().to_string();
        return Ok(Loaded {
            device: load_text(&name, &source)?,
            name,
        });
    }
    let entry = catalog::find(arg).ok_or_else(|| {
        let ids: Vec<String> = catalog::catalog().into_iter().map(|e| e.id).collect();
        miette::miette!(
            "--device: `{arg}` is neither a file nor a catalog device (catalog: {})",
            ids.join(", ")
        )
    })?;
    let name = entry.display_name();
    Ok(Loaded {
        device: load_text(&name, &entry.source)?,
        name,
    })
}

/// `plcc device check FILE...`
pub fn check(files: &[PathBuf], json: bool) -> Result<()> {
    if files.is_empty() {
        miette::bail!("at least one manifest file is required");
    }
    let mut failed = 0;
    let mut expanded = Vec::new();
    for f in files {
        let source = std::fs::read_to_string(f).into_diagnostic()?;
        let name = f.display().to_string();
        match load_text(&name, &source) {
            Ok(d) => {
                if !json {
                    println!(
                        "OK: {name}: {} v{} ({}), {} I/O point(s), target {}",
                        d.device.id,
                        d.device.version,
                        d.device.name,
                        d.io.len(),
                        d.target.triple
                    );
                }
                expanded.push(d);
            }
            Err(e) => {
                eprintln!("{e}");
                failed += 1;
            }
        }
    }
    if json {
        let v = if expanded.len() == 1 && files.len() == 1 {
            serde_json::to_string_pretty(&expanded[0])
        } else {
            serde_json::to_string_pretty(&expanded)
        }
        .into_diagnostic()?;
        println!("{v}");
    }
    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

/// `plcc device list`
pub fn list() -> Result<()> {
    let entries = catalog::catalog();
    let from_dir = entries.iter().any(|e| matches!(e.origin, Origin::File(_)));
    match catalog::catalog_dir() {
        Some(d) if from_dir => println!("catalog: {}", d.display()),
        _ => println!(
            "catalog: built-in copies only (no manifests in devices/, or the submodule is not checked out; PLCC_DEVICES names another directory)"
        ),
    }
    let mut bad = 0;
    for e in &entries {
        let c = plcc_device::load(&e.source, Some(&e.display_name()));
        let from = match &e.origin {
            Origin::Builtin => "built-in".to_string(),
            Origin::File(p) => p.display().to_string(),
        };
        match &c.device {
            Some(d) => println!(
                "{:<16} v{:<3} {:<22} {:<26} {}",
                d.device.id, d.device.version, d.device.name, d.target.triple, from
            ),
            None => {
                bad += 1;
                println!("{:<16} INVALID ({} error(s); `plcc device check {from}`)", e.id, c.errors().count());
            }
        }
    }
    if bad > 0 {
        std::process::exit(1);
    }
    Ok(())
}
