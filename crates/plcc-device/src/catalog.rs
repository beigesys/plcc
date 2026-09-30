// SPDX-License-Identifier: MPL-2.0

//! The device catalog: the manifests in the repository's `devices/` directory
//! (a git submodule, possibly not checked out), with a built-in copy of the
//! Opta and Simulator manifests compiled into this crate as the fallback.
//!
//! Lookup order for a directory catalog: `$PLCC_DEVICES`, then the `devices/`
//! directory of the plcc checkout this crate was built from. Entries found
//! there replace built-in entries with the same id.

use std::path::{Path, PathBuf};

/// Built-in manifests: `(file name, TOML text)`.
pub const BUILTIN: &[(&str, &str)] = &[
    ("arduino-opta.toml", include_str!("../builtin/arduino-opta.toml")),
    ("simulator.toml", include_str!("../builtin/simulator.toml")),
];

/// Where a catalog entry came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    Builtin,
    File(PathBuf),
}

/// One manifest in the catalog.
#[derive(Clone, Debug)]
pub struct Entry {
    /// `device.id`, or the file stem when the file does not parse.
    pub id: String,
    pub file_name: String,
    pub source: String,
    pub origin: Origin,
}

impl Entry {
    pub fn load(&self) -> crate::Checked {
        crate::load(&self.source, Some(&self.display_name()))
    }

    pub fn display_name(&self) -> String {
        match &self.origin {
            Origin::Builtin => format!("<builtin>/{}", self.file_name),
            Origin::File(p) => p.display().to_string(),
        }
    }
}

fn entry(file_name: &str, source: String, origin: Origin) -> Entry {
    let id = crate::parse(&source, None)
        .map(|m| m.device.id)
        .unwrap_or_else(|_| file_name.trim_end_matches(".toml").to_string());
    Entry {
        id,
        file_name: file_name.to_string(),
        source,
        origin,
    }
}

/// The built-in manifests.
pub fn builtin() -> Vec<Entry> {
    BUILTIN
        .iter()
        .map(|(name, text)| entry(name, text.to_string(), Origin::Builtin))
        .collect()
}

/// The TOML text of the built-in manifest with this id.
pub fn builtin_source(id: &str) -> Option<&'static str> {
    BUILTIN
        .iter()
        .find(|(name, _)| name.trim_end_matches(".toml") == id)
        .map(|(_, text)| *text)
}

/// Every `*.toml` directly in `dir`, sorted by file name. A missing directory
/// is an empty catalog.
pub fn load_dir(dir: &Path) -> std::io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e),
    };
    for e in rd {
        let path = e?.path();
        if path.extension().is_some_and(|x| x == "toml") && path.is_file() {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let text = std::fs::read_to_string(&path)?;
            out.push(entry(&name, text, Origin::File(path)));
        }
    }
    out.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    Ok(out)
}

/// The repository catalog directory: `$PLCC_DEVICES`, else `devices/` of the
/// checkout this crate was built in, if it holds any manifest.
pub fn catalog_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("PLCC_DEVICES") {
        return Some(PathBuf::from(d));
    }
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices");
    let has_toml = std::fs::read_dir(&d).ok()?.flatten().any(|e| {
        e.path().extension().is_some_and(|x| x == "toml")
    });
    has_toml.then_some(d)
}

/// The catalog: the directory catalog's entries, then built-in entries whose
/// id the directory does not have.
pub fn catalog() -> Vec<Entry> {
    let mut out = catalog_dir()
        .and_then(|d| load_dir(&d).ok())
        .unwrap_or_default();
    for b in builtin() {
        if !out.iter().any(|e| e.id == b.id) {
            out.push(b);
        }
    }
    out
}

/// The catalog entry with this id.
pub fn find(id: &str) -> Option<Entry> {
    catalog().into_iter().find(|e| e.id == id)
}
