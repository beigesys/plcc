// SPDX-License-Identifier: MPL-2.0

//! `plcc convert` over in-memory files: canonical Structured Text, IEC ladder
//! (PLCopen XML), Rockwell ladder (L5X) or the ladder model as JSON. Dialect
//! translation warnings, write warnings and ST-to-ladder notes come back as
//! structured diagnostics (`stage: "convert"`). Same behaviour as
//! `crates/plcc-cli/src/convert.rs`.

use crate::{Diagnostic, Project, Severity, Stage, expand, extension, is_l5x, parse_file_with};
use plcc_ladder::model::{Dialect, Project as Model};

/// Output notation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    St,
    Plcopen,
    L5x,
    LadderJson,
}

impl Format {
    pub fn parse(s: &str) -> Option<Format> {
        Some(match s {
            "st" => Format::St,
            "plcopen" => Format::Plcopen,
            "l5x" => Format::L5x,
            "ladder-json" => Format::LadderJson,
            _ => return None,
        })
    }
}

/// Outcome of [`convert`].
#[derive(Debug)]
pub struct Converted {
    /// The converted text, unless an error stopped the conversion.
    pub output: Option<String>,
    pub diagnostics: Vec<Diagnostic>,
}

fn note(severity: Severity, message: String) -> Diagnostic {
    let mut d = Diagnostic::plain(None, Stage::Convert, severity, message);
    d.code = Some(
        match severity {
            Severity::Error => "plcc::convert::error",
            Severity::Warning => "plcc::convert::warning",
            Severity::Advice => "plcc::convert::note",
        }
        .to_string(),
    );
    d
}

fn has_error(d: &[Diagnostic]) -> bool {
    d.iter().any(|d| d.severity == Severity::Error)
}

/// Convert `project` to `to`. `dialect` translates ladder output between IEC
/// and Logix (`--dialect`); `prelude` appends the Logix prelude to ST from L5X.
pub fn convert(project: &Project, to: Format, dialect: Option<Dialect>, prelude: bool) -> Converted {
    let mut diags = Vec::new();
    for path in project.files.keys() {
        if let Err(e) = crate::validate_path(path) {
            diags.push(Diagnostic::plain(None, Stage::Input, Severity::Error, e));
        }
    }
    if has_error(&diags) {
        return Converted { output: None, diagnostics: diags };
    }
    let any_json = entries(project).iter().any(|p| extension(p) == "json");
    let output = match to {
        Format::St if !any_json && dialect.is_none() => to_st(project, prelude, &mut diags),
        Format::St => load_model(project, &mut diags)
            .map(|m| translate(m, dialect, &mut diags))
            .and_then(|m| model_to_st(&m, prelude, &mut diags)),
        Format::LadderJson => load_model(project, &mut diags)
            .map(|m| translate(m, dialect, &mut diags))
            .map(|m| m.to_json() + "\n"),
        Format::Plcopen => load_model(project, &mut diags)
            .map(|m| translate(m, Some(Dialect::Iec), &mut diags))
            .and_then(|m| written(plcc_plcopen::ladder::write(&m), "PLCopen XML", &mut diags)),
        Format::L5x => load_model(project, &mut diags)
            .map(|m| translate(m, Some(Dialect::Logix), &mut diags))
            .and_then(|m| written(plcc_l5x::ladder::write(&m), "L5X", &mut diags)),
    };
    let output = if has_error(&diags) { None } else { output };
    Converted { output, diagnostics: diags }
}

/// The entry files (explicit, or every file for a conversion).
fn entries(project: &Project) -> Vec<String> {
    match &project.entry {
        Some(e) => e.clone(),
        None => project.files.keys().cloned().collect(),
    }
}

fn written<E: std::fmt::Display>(
    r: Result<(String, Vec<String>), Vec<E>>,
    what: &str,
    diags: &mut Vec<Diagnostic>,
) -> Option<String> {
    match r {
        Ok((text, warnings)) => {
            diags.extend(warnings.into_iter().map(|w| note(Severity::Warning, w)));
            Some(text)
        }
        Err(errs) => {
            diags.extend(
                errs.iter()
                    .map(|e| note(Severity::Error, format!("cannot write {what}: {e}"))),
            );
            None
        }
    }
}

fn translate(model: Model, to: Option<Dialect>, diags: &mut Vec<Diagnostic>) -> Model {
    match to {
        Some(to) if to != model.dialect => {
            let (out, warnings) = plcc_l5x::ladder::translate(&model, to);
            diags.extend(warnings.into_iter().map(|w| note(Severity::Warning, w)));
            out
        }
        _ => model,
    }
}

/// The single input as a ladder model.
fn load_model(project: &Project, diags: &mut Vec<Diagnostic>) -> Option<Model> {
    let entries = entries(project);
    let [path] = entries.as_slice() else {
        diags.push(note(
            Severity::Error,
            format!("ladder conversions take one input file, not {}", entries.len()),
        ));
        return None;
    };
    let Some(source) = project.files.get(path) else {
        diags.push(Diagnostic::plain(
            Some(path),
            Stage::Input,
            Severity::Error,
            format!("entry `{path}` is not one of the files"),
        ));
        return None;
    };
    if extension(path) == "json" {
        return match Model::from_json(source) {
            Ok(m) => Some(m),
            Err(e) => {
                diags.push(Diagnostic::plain(
                    Some(path),
                    Stage::Convert,
                    Severity::Error,
                    format!("not a ladder model: {e}"),
                ));
                None
            }
        };
    }
    if is_l5x(path, source) {
        let (model, errs) = plcc_l5x::ladder::read(source);
        for e in &errs {
            let sev = Some(if e.is_warning() { Severity::Warning } else { Severity::Error });
            diags.push(Diagnostic::from_miette(e, path, source, Stage::L5x, sev));
        }
        return model.filter(|_| !errs.iter().any(|e| !e.is_warning()));
    }
    if extension(path) == "xml" || plcc_plcopen::is_plcopen(source) {
        let (model, errs) = plcc_plcopen::ladder::read(source);
        for e in &errs {
            diags.push(Diagnostic::from_miette(e, path, source, Stage::Plcopen, None));
        }
        return model.filter(|_| errs.is_empty());
    }
    // Structured Text: the drawable subset becomes rungs.
    let (unit, errs) = plcc_st::parse(source);
    for e in &errs {
        diags.push(Diagnostic::from_miette(e, path, source, Stage::Parse, None));
    }
    if !errs.is_empty() {
        return None;
    }
    let (model, notes) = plcc_ladder::from_st::from_unit(&unit);
    diags.extend(notes.into_iter().map(|n| note(Severity::Advice, n)));
    Some(model)
}

fn model_to_st(model: &Model, prelude: bool, diags: &mut Vec<Diagnostic>) -> Option<String> {
    match model.dialect {
        Dialect::Iec => {
            let (unit, errs) = plcc_ladder::to_unit(model, true);
            if !errs.is_empty() {
                diags.extend(errs.iter().map(|e| {
                    note(Severity::Error, format!("the ladder model does not lower: {e}"))
                }));
                return None;
            }
            Some(plcc_st::print_unit(&unit))
        }
        Dialect::Logix => {
            let l5x = written(plcc_l5x::ladder::write(model), "L5X", diags)?;
            let (unit, errs) = plcc_l5x::parse_annotated(&l5x, &plcc_l5x::Options::default());
            if errs.iter().any(|e| !e.is_warning()) {
                diags.extend(errs.iter().filter(|e| !e.is_warning()).map(|e| {
                    note(Severity::Error, format!("the Logix model does not lower: {e}"))
                }));
                return None;
            }
            // The text is not complete without the prelude: say what it holds.
            let mut out = String::from(plcc_l5x::PRELUDE_NOTE);
            out.push_str(&plcc_st::print_unit(&unit));
            if prelude {
                out.push_str("\n(* ---- Logix prelude (plcc-l5x) ---- *)\n");
                out.push_str(&plcc_l5x::prelude());
            }
            Some(out)
        }
    }
}

fn to_st(project: &Project, prelude: bool, diags: &mut Vec<Diagnostic>) -> Option<String> {
    let inputs = expand(project, diags);
    let opts = plcc_l5x::Options::default();
    let mut out = String::new();
    let mut any_l5x = false;
    let mut all = Vec::new();
    for (path, _) in &inputs.files {
        let source = &project.files[path];
        let logix = is_l5x(path, source);
        any_l5x |= logix;
        let (unit, file_diags) = parse_file_with(path, source, &opts, true);
        let failed = has_error(&file_diags);
        diags.extend(file_diags);
        if failed {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        let name = path.rsplit('/').next().unwrap_or(path);
        out.push_str(&format!("(* Converted by plcc from {name}. *)\n"));
        if logix {
            out.push_str(
                "(* Logix instructions are calls into the Logix prelude (lx__ton, lx__put_DINT_i, ...;\n   \
                 docs/l5x.md \"Reading the generated ST\"); `prelude` appends it. *)\n",
            );
        }
        out.push('\n');
        out.push_str(&plcc_st::print_unit(&unit));
        all.extend(unit.declarations);
    }
    if has_error(diags) {
        return None;
    }
    let has_config = all
        .iter()
        .any(|d| matches!(d, plcc_st::ast::Declaration::Configuration(_)));
    if !has_config
        && let Some(text) = plcc_twincat::configuration_source(&inputs.tasks)
        && let (unit, errs) = plcc_st::parse(&text)
        && errs.is_empty()
    {
        out.push_str("\n(* TwinCAT task configuration *)\n");
        out.push_str(&plcc_st::print_unit(&unit));
    }
    if any_l5x && prelude {
        out.push_str("\n(* ---- Logix prelude (plcc-l5x) ---- *)\n");
        out.push_str(&plcc_l5x::prelude());
    }
    Some(out)
}

/// The instruction pin catalog for an editor's palette, as JSON.
pub fn catalog_json(dialect: Dialect) -> String {
    serde_json::to_string(&plcc_ladder::catalog::all(dialect)).unwrap_or_else(|_| "[]".into())
}
