// SPDX-License-Identifier: MPL-2.0

//! A ladder model (`plcc-ladder`'s JSON, as plcc studio saves it) as a
//! compiler input. A `.json` input file is read as a model:
//!
//! * a Logix-dialect model is written as L5X ([`plcc_l5x::ladder::write_mapped`])
//!   and compiled as one, with each variable's `address` bound to the process
//!   image (the L5X I/O map, docs/l5x.md); the model's tasks schedule its
//!   programs;
//! * an IEC-dialect model is lowered to Structured Text
//!   ([`plcc_ladder::to_unit`]).
//!
//! Diagnostics are traced back to the model: [`Diagnostic::ladder`] names the
//! program, routine, rung, element and operand, and the span becomes a range
//! in the rung's text (or in the ST box's code). Tags a rung uses but the
//! model does not declare are errors on the element that names them.

use crate::{Diagnostic, Label, LineIndex, Severity, Stage};
use plcc_l5x::ladder::{MapEntry, MapKind};
use plcc_ladder::model::{Dialect, Element, Id, Project as Model, walk};
use serde::Serialize;

/// Where in a ladder model a diagnostic belongs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LadderRef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pou: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routine: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rung: Option<Id>,
    /// The innermost element (an ST box for a problem in its code).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element: Option<Id>,
    /// Index of the operand (pin) of the element, Logix order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operand: Option<usize>,
    /// A variable (tag) the diagnostic is about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

/// Whether a path names a ladder model input.
pub fn is_model_path(path: &str) -> bool {
    crate::extension(path) == "json"
}

/// A ladder model ready for the readers.
pub struct Lowered {
    pub model: Model,
    /// The model as L5X (Logix dialect), or `None` for an IEC model.
    pub l5x: Option<String>,
    pub map: Vec<MapEntry>,
    /// `(tag path, address)` bindings from the variables' addresses.
    pub io: Vec<(String, String)>,
}

fn diag(path: &str, severity: Severity, message: String, at: LadderRef) -> Diagnostic {
    let mut d = Diagnostic::plain(Some(path), Stage::Convert, severity, message);
    d.ladder = Some(at);
    d
}

/// Read and check a model; `Err` when it cannot be compiled at all.
pub fn lower(path: &str, source: &str) -> Result<(Lowered, Vec<Diagnostic>), Vec<Diagnostic>> {
    let model = Model::from_json(source).map_err(|e| {
        vec![Diagnostic::plain(
            Some(path),
            Stage::Convert,
            Severity::Error,
            format!("not a ladder model: {e}"),
        )]
    })?;
    let mut diags = undeclared(path, &model);
    let mut io = Vec::new();
    for v in &model.globals {
        if let Some(a) = v.address.as_deref().filter(|a| !a.trim().is_empty()) {
            io.push((v.name.clone(), a.trim().to_string()));
        }
    }
    for p in &model.pous {
        for v in &p.variables {
            if let Some(a) = v.address.as_deref().filter(|a| !a.trim().is_empty()) {
                io.push((format!("Program:{}.{}", p.name, v.name), a.trim().to_string()));
            }
        }
    }
    if model.dialect == Dialect::Iec {
        return Ok((
            Lowered {
                model,
                l5x: None,
                map: Vec::new(),
                io,
            },
            diags,
        ));
    }
    match plcc_l5x::ladder::write_mapped(&model) {
        Ok((text, _warnings, map)) => Ok((
            Lowered {
                model,
                l5x: Some(text),
                map,
                io,
            },
            diags,
        )),
        Err(errors) => {
            for e in errors {
                let at = locate_element(&model, e.element);
                diags.push(diag(path, Severity::Error, e.message, at));
            }
            Err(diags)
        }
    }
}

/// The base tag of a Logix operand (`T1.DN` → `T1`, `a[3].x` → `a`), or
/// `None` for an immediate, an expression, `?` or a module / status tag.
fn base_tag(op: &str) -> Option<&str> {
    let op = op.trim();
    let first = op.chars().next()?;
    if !(first.is_ascii_alphabetic() || first == '_') || op.contains(':') {
        return None;
    }
    let end = op.find(['.', '[']).unwrap_or(op.len());
    let base = &op[..end];
    if !base.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    // `a.b + c` is an expression: the rest must be members and indices.
    let rest = &op[end..];
    if rest
        .chars()
        .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '[' | ']' | ',')))
    {
        return None;
    }
    Some(base)
}

/// Pins whose value is not a tag.
fn not_a_tag(block: &str, pin: &str) -> bool {
    pin.eq_ignore_ascii_case("Routine Name")
        || (block.eq_ignore_ascii_case("JSR") && pin.eq_ignore_ascii_case("Input Count"))
}

/// Logix: errors for operands naming tags the model does not declare.
fn undeclared(path: &str, model: &Model) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    if model.dialect != Dialect::Logix {
        return out;
    }
    let globals: Vec<String> = model
        .globals
        .iter()
        .map(|v| v.name.to_ascii_lowercase())
        .collect();
    for p in &model.pous {
        let locals: Vec<String> = p
            .variables
            .iter()
            .map(|v| v.name.to_ascii_lowercase())
            .collect();
        let known = |n: &str| {
            let n = n.to_ascii_lowercase();
            globals.contains(&n) || locals.contains(&n)
        };
        for r in &p.routines {
            for g in &r.rungs {
                walk(&g.elements, &mut |e| {
                    let mut check = |k: usize, op: &str| {
                        if let Some(b) = base_tag(op)
                            && !known(b)
                        {
                            out.push(diag(
                                path,
                                Severity::Error,
                                format!("unknown tag `{b}`"),
                                LadderRef {
                                    pou: Some(p.name.clone()),
                                    routine: Some(r.name.clone()),
                                    rung: Some(g.id),
                                    element: Some(e.id()),
                                    operand: Some(k),
                                    tag: Some(b.to_string()),
                                },
                            ));
                        }
                    };
                    match e {
                        Element::Contact(c) => check(0, &c.operand),
                        Element::Coil(c) => check(0, &c.operand),
                        Element::Block(b) => {
                            for (k, pin) in b.pins.iter().enumerate() {
                                if !not_a_tag(&b.name, &pin.name)
                                    && let Some(v) = &pin.value
                                {
                                    check(k, v);
                                }
                            }
                        }
                        _ => {}
                    }
                });
            }
        }
    }
    out
}

/// The POU, routine and rung holding element (or rung) `id`.
pub fn locate_element(model: &Model, id: Id) -> LadderRef {
    for p in &model.pous {
        for r in &p.routines {
            for g in &r.rungs {
                let mut found = g.id == id;
                walk(&g.elements, &mut |e| found |= e.id() == id);
                if found {
                    return LadderRef {
                        pou: Some(p.name.clone()),
                        routine: Some(r.name.clone()),
                        rung: Some(g.id),
                        element: (g.id != id).then_some(id),
                        ..Default::default()
                    };
                }
            }
        }
    }
    LadderRef::default()
}

fn code_of(model: &Model, id: Id) -> Option<&str> {
    for p in &model.pous {
        for r in &p.routines {
            for g in &r.rungs {
                let mut code = None;
                walk(&g.elements, &mut |e| {
                    if let Element::St(s) = e
                        && s.id == id
                    {
                        code = Some(s.code.as_str());
                    }
                });
                if code.is_some() {
                    return code;
                }
            }
        }
    }
    None
}

/// Where byte `offset` of the written L5X is in the model, with the position
/// re-based into the rung's text or the ST box's code.
pub fn locate_offset(
    lowered: &Lowered,
    offset: usize,
) -> Option<(LadderRef, crate::Position, String)> {
    let l5x = lowered.l5x.as_deref()?;
    let e = lowered
        .map
        .iter()
        .find(|e| e.range.start <= offset && offset <= e.range.end)?;
    let rel = offset - e.range.start;
    let mut at = LadderRef {
        pou: Some(e.pou.clone()),
        routine: Some(e.routine.clone()),
        rung: Some(e.rung),
        ..Default::default()
    };
    match &e.kind {
        MapKind::Rung(elements) => {
            let text = l5x.get(e.range.clone())?;
            if let Some(m) = elements
                .iter()
                .rev()
                .find(|m| m.span.start <= rel && rel < m.span.end.max(m.span.start + 1))
            {
                at.element = Some(m.id);
                at.operand = m
                    .operands
                    .iter()
                    .position(|o| o.start <= rel && rel <= o.end);
            }
            let pos = LineIndex::new(text).position(rel);
            Some((at, pos, text.to_string()))
        }
        MapKind::StLine {
            element,
            code_offset,
            ..
        } => {
            at.element = Some(*element);
            let code = code_of(&lowered.model, *element)?;
            let pos = LineIndex::new(code).position(code_offset + rel);
            Some((at, pos, code.to_string()))
        }
    }
}

/// Trace diagnostics reported against the written L5X back to the model.
pub fn remap(diags: &mut [Diagnostic], path: &str, lowered: &Lowered) {
    if lowered.l5x.is_none() {
        return;
    }
    for d in diags.iter_mut() {
        if d.file.as_deref() != Some(path) || d.ladder.is_some() {
            continue;
        }
        let Some(span) = d.span.clone() else {
            d.ladder = Some(LadderRef::default());
            continue;
        };
        let start = span.start.offset as usize;
        match locate_offset(lowered, start) {
            Some((at, pos, text)) => {
                let len = (span.end.offset - span.start.offset) as usize;
                let end_rel = (pos.offset as usize + len).min(text.len());
                let end = LineIndex::new(&text).position(end_rel);
                let label = Label {
                    start: pos,
                    end,
                    message: span.message.clone(),
                };
                d.span = Some(label.clone());
                d.labels = vec![label];
                d.ladder = Some(at);
            }
            None => {
                // Tag declarations, the I/O map: name the tag when the
                // message does.
                let tag = lowered
                    .model
                    .globals
                    .iter()
                    .chain(lowered.model.pous.iter().flat_map(|p| p.variables.iter()))
                    .find(|v| d.message.contains(&format!("`{}`", v.name)))
                    .map(|v| v.name.clone());
                d.span = None;
                d.labels.clear();
                d.ladder = Some(LadderRef {
                    tag,
                    ..Default::default()
                });
            }
        }
    }
}

/// A fault site `file:line:col` of a program compiled from a ladder model
/// (the line and column are in the written L5X): where it is in the model.
pub fn locate_site(source: &str, line: u32, col: u32) -> Option<LadderRef> {
    let (lowered, _) = lower("model.json", source).ok()?;
    let l5x = lowered.l5x.as_deref()?;
    let line_start = l5x
        .split_inclusive('\n')
        .take(line.saturating_sub(1) as usize)
        .map(str::len)
        .sum::<usize>();
    let line_text = l5x[line_start..].split('\n').next().unwrap_or("");
    // `col` counts UTF-16 units.
    let mut units = 0u32;
    let mut rel = line_text.len();
    for (i, ch) in line_text.char_indices() {
        if units + 1 >= col {
            rel = i;
            break;
        }
        units += ch.len_utf16() as u32;
    }
    locate_offset(&lowered, line_start + rel).map(|(at, _, _)| at)
}
