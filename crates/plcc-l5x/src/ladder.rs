// SPDX-License-Identifier: MPL-2.0

//! L5X ↔ the ladder model of `plcc-ladder` (Logix dialect).
//!
//! **Reading** ([`read`]): controller tags become the project's globals,
//! each program a POU with its program tags and its routines (the main
//! routine first). RLL rungs are read through `plcc_ladder::rll` (the same
//! reader the compiler lowers from), with each rung's `<Comment>`; an ST
//! routine becomes one rung holding an ST box with its lines (Logix ST).
//! Tag types are Logix type names, arrays as `DINT[10]`; an alias tag has the
//! type `ALIAS` and its target as the initial value. Tasks, UDTs, AOIs and
//! modules are not part of the model.
//!
//! **Writing** ([`write`]) makes a minimal L5X a Logix project can be
//! created from: a controller (1756-L83E) with its local module, controller
//! tags, one program per POU with its tags and RLL routines, and a continuous
//! task scheduling every program. Tags the model does not declare are
//! derived from how the rungs use them (see [`derive_tags`]). An ST box
//! inside a rung becomes an ST routine called with `JSR` in its place; a
//! routine that is only an ST box is written as an ST routine.

use crate::model::{self as l5, RoutineKind};
use crate::xml;
use crate::{L5xError, byte_offset};
use plcc_ladder::model::*;
use plcc_ladder::rll;
use plcc_st::Span;
use std::collections::BTreeMap;
use std::fmt::Write;

/// Read an L5X project into the ladder model.
pub fn read(source: &str) -> (Option<Project>, Vec<L5xError>) {
    let opt = roxmltree::ParsingOptions {
        allow_dtd: false,
        ..Default::default()
    };
    let doc = match roxmltree::Document::parse_with_options(source, opt) {
        Ok(d) => d,
        Err(e) => {
            let at = byte_offset(source, e.pos());
            return (
                None,
                vec![L5xError::new(
                    format!("malformed XML: {e}"),
                    Span::new(at, at),
                )],
            );
        }
    };
    let root = doc.root_element();
    if xml::name(root) != "RSLogix5000Content" {
        return (
            None,
            vec![L5xError::new(
                "not an L5X export",
                xml::tag_span(source, root),
            )],
        );
    }
    let mut reader = l5::Reader {
        src: source,
        errors: Vec::new(),
    };
    let lp = reader.project(root);
    let mut errors = reader.errors;
    let mut ids = Ids::new();
    let name = xml::attr(root, "TargetName").unwrap_or("").to_string();
    let mut project = Project {
        dialect: Dialect::Logix,
        name,
        globals: lp
            .tags
            .iter()
            .map(|t| variable(t, VarSection::Global))
            .collect(),
        pous: Vec::new(),
        declarations: Vec::new(),
        tasks: lp.tasks.iter().filter_map(task).collect(),
    };
    for pr in &lp.programs {
        let mut pou = Pou {
            id: ids.fresh(),
            name: pr.name.text.clone(),
            kind: PouKind::Program,
            return_type: None,
            variables: pr
                .tags
                .iter()
                .map(|t| variable(t, VarSection::Local))
                .collect(),
            routines: Vec::new(),
            members: String::new(),
        };
        let main = pr
            .main_routine
            .as_ref()
            .map(|m| m.text.to_ascii_lowercase());
        let mut routines: Vec<&l5::RoutineDef> = pr.routines.iter().collect();
        routines.sort_by_key(|r| Some(r.name.text.to_ascii_lowercase()) != main);
        for r in routines {
            match routine(r, &mut ids) {
                Ok(rt) => pou.routines.push(rt),
                Err(e) => errors.push(e),
            }
        }
        project.pous.push(pou);
    }
    (Some(project), errors)
}

/// A continuous or periodic task (event tasks and inhibited tasks are left
/// out: the model has no triggers).
fn task(t: &l5::TaskDef) -> Option<Task> {
    if t.kind == l5::TaskType::Event || t.inhibited {
        return None;
    }
    Some(Task {
        name: t.name.text.clone(),
        interval_ms: if t.kind == l5::TaskType::Periodic {
            t.rate_ms
        } else {
            None
        },
        priority: Some(t.priority),
        programs: t.programs.iter().map(|p| p.text.clone()).collect(),
    })
}

fn variable(t: &l5::TagDef, section: VarSection) -> Variable {
    let mut data_type = t
        .data_type
        .as_ref()
        .map(|n| n.text.clone())
        .unwrap_or_default();
    if !t.dims.is_empty() {
        let d: Vec<String> = t.dims.iter().map(|d| d.to_string()).collect();
        let _ = write!(data_type, "[{}]", d.join(","));
    }
    let mut initial = t.decorated.and_then(|d| {
        if let Some(st) = xml::child(d, "Structure") {
            // `(PRE := 150, ACC := 0)`: the flat members of a structure.
            let members: Vec<String> = xml::children(st, "DataValueMember")
                .filter_map(|m| {
                    Some(format!(
                        "{} := {}",
                        xml::attr(m, "Name")?,
                        xml::attr(m, "Value")?
                    ))
                })
                .collect();
            return (!members.is_empty()).then(|| format!("({})", members.join(", ")));
        }
        let dv = if xml::name(d) == "DataValue" {
            Some(d)
        } else {
            xml::child(d, "DataValue")
        }?;
        let v = xml::attr(dv, "Value")?.trim().to_string();
        (v != "0" && v != "0.0" && v != "0.00000000e+000").then_some(v)
    });
    if initial.is_none()
        && let Some(l) = &t.l5k
    {
        // L5K data (`[0,50,0]`, `5`), kept as written.
        let v = l.text.trim().to_string();
        if !v.is_empty() && v != "0" {
            initial = Some(v);
        }
    }
    if t.kind == l5::TagKind::Alias {
        data_type = "ALIAS".into();
        initial = t.alias_for.as_ref().map(|a| a.text.trim().to_string());
    }
    let section = match t.usage {
        l5::Usage::Input => VarSection::Input,
        l5::Usage::Output => VarSection::Output,
        l5::Usage::InOut => VarSection::InOut,
        _ => section,
    };
    Variable {
        name: t.name.text.clone(),
        data_type,
        section,
        initial,
        address: None,
        comment: None,
        constant: t.constant,
        retain: false,
    }
}

fn routine(r: &l5::RoutineDef, ids: &mut Ids) -> Result<Routine, L5xError> {
    let mut out = Routine {
        id: ids.fresh(),
        name: r.name.text.clone(),
        rungs: Vec::new(),
    };
    match r.kind {
        RoutineKind::Rll => {
            for rung in &r.rungs {
                let Some(text) = &rung.text else { continue };
                let rs = rll::read(&text.text, ids).map_err(|e| {
                    L5xError::new(
                        format!("rung {}: {}", rung.number.unwrap_or(0), e.message),
                        text.span(e.span),
                    )
                })?;
                for (k, mut g) in rs.into_iter().enumerate() {
                    if k == 0 {
                        g.comment = rung.comment.clone();
                    }
                    out.rungs.push(g);
                }
            }
        }
        RoutineKind::St => {
            let code: Vec<&str> = r.lines.iter().map(|l| l.text.as_str()).collect();
            out.rungs.push(Rung {
                id: ids.fresh(),
                elements: vec![Element::St(StBox {
                    id: ids.fresh(),
                    code: code.join("\n"),
                    notes: vec!["a Logix ST routine".into()],
                })],
                ..Default::default()
            });
        }
        _ => {
            return Err(L5xError::warning(
                format!(
                    "routine `{}` ({}) is not ladder or ST: left out of the model",
                    r.name.text, r.kind_text
                ),
                r.span,
            ));
        }
    }
    Ok(out)
}

// ── Operands for translation ──

/// Logix operands to IEC ST, through plcc's Logix expression parser (the one
/// the L5X lowering uses): CPT/CMP precedence (AND/OR/XOR bind tighter than
/// comparisons) becomes explicit parentheses, `&&`/`||`/`!` become AND/OR/NOT,
/// SQR/ASN/ACS/ATN/TRN become SQRT/ASIN/ACOS/ATAN/TRUNC, an indirect bit
/// `x.[i]` is read as `(SHR(x, i) AND 1) = 1`, and `S:FS` becomes `S_FS`.
pub struct LogixOperands;

impl plcc_ladder::translate::Operands for LogixOperands {
    fn logix_to_iec(&self, text: &str, write: bool) -> Result<String, String> {
        let e = crate::operand::parse_expr(text, 0..text.len()).map_err(|e| e.message)?;
        let x = to_iec(&e, write)?;
        Ok(plcc_st::print_expression(&x))
    }
}

/// [`plcc_ladder::translate::translate_with`] with [`LogixOperands`].
pub fn translate(project: &Project, to: Dialect) -> (Project, Vec<String>) {
    plcc_ladder::translate::translate_with(project, to, &LogixOperands)
}

fn to_iec(e: &crate::operand::LExpr, write: bool) -> Result<plcc_st::Expression, String> {
    use crate::operand::{BinOp as B, LKind, Seg, UnOp as U};
    use plcc_st::{BinaryOp, Expression, ExpressionKind as K, Ident, UnaryOp};
    let sp = Span::empty();
    let ex = |kind| Expression { kind, span: sp };
    let id = |n: &str| ex(K::Identifier(Ident::new(n, sp)));
    let bin = |op, l, r| {
        ex(K::BinaryOp {
            op,
            left: Box::new(l),
            right: Box::new(r),
        })
    };
    let call = |f: &str, args: Vec<Expression>| {
        ex(K::FunctionCall {
            callee: Box::new(id(f)),
            args: args
                .into_iter()
                .map(|value| plcc_st::CallArg {
                    name: None,
                    value,
                    is_output: false,
                    negated: false,
                    span: sp,
                })
                .collect(),
        })
    };
    Ok(match &e.kind {
        LKind::Int(v) => ex(K::IntegerLiteral(*v)),
        LKind::Real(v) if v.is_finite() => ex(K::RealLiteral(*v)),
        LKind::Real(_) => return Err("infinity (`1.$`) has no IEC literal".into()),
        LKind::Str(s) => ex(K::StringLiteral(s.clone())),
        LKind::Unset => return Err("an unset operand `?`".into()),
        LKind::Path(p) => {
            let mut x = if p.base.eq_ignore_ascii_case("S:FS") {
                if write {
                    return Err("S:FS cannot be written".into());
                }
                id("S_FS")
            } else if p.base.contains(':') {
                return Err(format!("{} is a Logix module or system tag", p.base));
            } else {
                id(&p.base)
            };
            for (k, seg) in p.segs.iter().enumerate() {
                x = match seg {
                    Seg::Member(m, _) => ex(K::MemberAccess {
                        object: Box::new(x),
                        member: Ident::new(m.clone(), sp),
                    }),
                    Seg::Bit(b, _) => ex(K::MemberAccess {
                        object: Box::new(x),
                        member: Ident::new(b.to_string(), sp),
                    }),
                    Seg::Index(ix, _) => ex(K::ArrayIndex {
                        array: Box::new(x),
                        indices: ix
                            .iter()
                            .map(|i| to_iec(i, false))
                            .collect::<Result<_, _>>()?,
                    }),
                    Seg::IndirectBit(i) => {
                        if write || k + 1 != p.segs.len() {
                            return Err("an indirect bit `.[i]` can only be read".into());
                        }
                        let shifted = call("SHR", vec![x, to_iec(i, false)?]);
                        bin(
                            BinaryOp::Equal,
                            ex(K::Parenthesized(Box::new(bin(
                                BinaryOp::And,
                                shifted,
                                ex(K::IntegerLiteral(1)),
                            )))),
                            ex(K::IntegerLiteral(1)),
                        )
                    }
                };
            }
            x
        }
        LKind::Unary(op, x) => ex(K::UnaryOp {
            op: match op {
                U::Neg => UnaryOp::Neg,
                U::Not | U::LNot => UnaryOp::Not,
            },
            operand: Box::new(to_iec(x, false)?),
        }),
        LKind::Binary(op, l, r) => {
            let op = match op {
                B::Pow => BinaryOp::Power,
                B::Mul => BinaryOp::Mul,
                B::Div => BinaryOp::Div,
                B::Mod => BinaryOp::Mod,
                B::Add => BinaryOp::Add,
                B::Sub => BinaryOp::Sub,
                B::And | B::LAnd => BinaryOp::And,
                B::Xor | B::LXor => BinaryOp::Xor,
                B::Or | B::LOr => BinaryOp::Or,
                B::Eq => BinaryOp::Equal,
                B::Ne => BinaryOp::NotEqual,
                B::Lt => BinaryOp::Less,
                B::Le => BinaryOp::LessEqual,
                B::Gt => BinaryOp::Greater,
                B::Ge => BinaryOp::GreaterEqual,
            };
            bin(op, to_iec(l, false)?, to_iec(r, false)?)
        }
        LKind::Call(f, args) => {
            let args: Vec<Expression> = args
                .iter()
                .map(|a| to_iec(a, false))
                .collect::<Result<_, _>>()?;
            let name = match f.as_str() {
                "SQR" | "SQRT" => "SQRT",
                "ASN" | "ASIN" => "ASIN",
                "ACS" | "ACOS" => "ACOS",
                "ATN" | "ATAN" => "ATAN",
                "TRN" | "TRUNC" => "TRUNC",
                "ABS" | "SIN" | "COS" | "TAN" | "LN" | "LOG" => f.as_str(),
                "DEG" | "RAD" => {
                    let k = if f == "DEG" {
                        180.0 / std::f64::consts::PI
                    } else {
                        std::f64::consts::PI / 180.0
                    };
                    let a = args.into_iter().next().ok_or("DEG/RAD need an argument")?;
                    return Ok(bin(BinaryOp::Mul, a, ex(K::RealLiteral(k))));
                }
                other => return Err(format!("the function {other} has no IEC counterpart")),
            };
            call(name, args)
        }
    })
}

// ── Writing ──

/// Why a model cannot be written as L5X.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteError {
    pub element: Id,
    pub message: String,
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "element {}: {}", self.element, self.message)
    }
}

fn cdata(s: &str) -> String {
    format!("<![CDATA[{}]]>", s.replace("]]>", "]]]]><![CDATA[>"))
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const ATOMIC: &[&str] = &[
    "BOOL", "SINT", "INT", "DINT", "LINT", "USINT", "UINT", "UDINT", "ULINT", "REAL", "LREAL",
];

/// Write a Logix-dialect model as an L5X project; warnings name tags whose
/// type had to be guessed.
pub fn write(project: &Project) -> Result<(String, Vec<String>), Vec<WriteError>> {
    write_mapped(project).map(|(text, warnings, _)| (text, warnings))
}

/// Where [`write_mapped`] put a rung's text or a line of Structured Text in
/// the L5X, so positions in it (diagnostics, fault sites) can be traced back
/// to the model.
#[derive(Clone, Debug, PartialEq)]
pub struct MapEntry {
    pub pou: String,
    pub routine: String,
    pub rung: Id,
    /// Byte range of the text (inside its CDATA) in the L5X.
    pub range: std::ops::Range<usize>,
    pub kind: MapKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MapKind {
    /// A rung's text: each element (depth first, as [`walk`] visits them)
    /// with its byte range and its operands' ranges, relative to the text.
    Rung(Vec<MappedElement>),
    /// Line `line` (0-based) of an ST box's code (or of an ST routine, whose
    /// one box is `element`); `code_offset` is where the line starts in the
    /// code, in bytes.
    StLine {
        element: Id,
        line: usize,
        code_offset: usize,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct MappedElement {
    pub id: Id,
    pub span: std::ops::Range<usize>,
    pub operands: Vec<std::ops::Range<usize>>,
}

/// [`write`], and where each rung and ST line went.
pub fn write_mapped(
    project: &Project,
) -> Result<(String, Vec<String>, Vec<MapEntry>), Vec<WriteError>> {
    let mut map: Vec<MapEntry> = Vec::new();
    if project.dialect != Dialect::Logix {
        return Err(vec![WriteError {
            element: 0,
            message: "L5X holds Logix ladder: translate an IEC model to the Logix dialect first"
                .into(),
        }]);
    }
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let name = if project.name.is_empty() {
        "plcc"
    } else {
        &project.name
    };
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n");
    let _ = writeln!(
        out,
        "<RSLogix5000Content SchemaRevision=\"1.0\" SoftwareRevision=\"33.01\" TargetName=\"{n}\" TargetType=\"Controller\" ContainsContext=\"false\" ExportDate=\"Thu Jan 01 00:00:00 2026\" ExportOptions=\"NoRawData L5KData DecoratedData ForceProtectedEncoding AllProjDocTrans\">",
        n = esc(name)
    );
    let _ = writeln!(
        out,
        "<Controller Use=\"Target\" Name=\"{}\" ProcessorType=\"1756-L83E\" MajorRev=\"33\" MinorRev=\"11\" ProjectSN=\"16#0000_0000\" MatchProjectToController=\"false\" CanUseRPIFromProducer=\"false\" InhibitAutomaticFirmwareUpdate=\"0\" PassThroughConfiguration=\"EnabledWithAppend\" DownloadProjectDocumentationAndExtendedProperties=\"true\" DownloadProjectCustomProperties=\"true\" ReportMinorOverflow=\"false\">",
        esc(name)
    );
    out.push_str("<DataTypes/>\n<Modules>\n<Module Name=\"Local\" CatalogNumber=\"1756-L83E\" Vendor=\"1\" ProductType=\"14\" ProductCode=\"166\" Major=\"33\" Minor=\"11\" ParentModule=\"Local\" ParentModPortId=\"1\" Inhibited=\"false\" MajorFault=\"true\">\n<EKey State=\"Disabled\"/>\n<Ports>\n<Port Id=\"1\" Address=\"0\" Type=\"ICP\" Upstream=\"false\">\n<Bus Size=\"10\"/>\n</Port>\n</Ports>\n</Module>\n</Modules>\n<AddOnInstructionDefinitions/>\n");
    // Tags: declared ones, then the ones the rungs need.
    let derived = derive_tags(project, &mut warnings);
    for d in &project.declarations {
        let first = d.lines().next().unwrap_or("");
        warnings.push(format!("not written to L5X (not ladder): {first}"));
    }
    for p in project.pous.iter().filter(|p| !p.members.is_empty()) {
        warnings.push(format!(
            "{}: methods / properties / actions are not written to L5X",
            p.name
        ));
    }
    out.push_str("<Tags>\n");
    for v in &project.globals {
        tag(&mut out, v);
    }
    out.push_str("</Tags>\n<Programs>\n");
    for pou in &project.pous {
        let main = pou
            .routines
            .first()
            .map(|r| r.name.as_str())
            .unwrap_or("MainRoutine");
        let _ = writeln!(
            out,
            "<Program Name=\"{}\" TestEdits=\"false\" MainRoutineName=\"{}\" Disabled=\"false\" UseAsFolder=\"false\">",
            esc(&pou.name),
            esc(main)
        );
        out.push_str("<Tags>\n");
        for v in &pou.variables {
            tag(&mut out, v);
        }
        for v in derived.get(&pou.name).into_iter().flatten() {
            tag(&mut out, v);
        }
        out.push_str("</Tags>\n<Routines>\n");
        let mut extra: Vec<(String, String, Id, Id, String)> = Vec::new();
        for r in &pou.routines {
            if let [g] = r.rungs.as_slice()
                && let [Element::St(s)] = g.elements.as_slice()
                && g.label.is_none()
            {
                for (line, code_offset, range) in st_routine(&mut out, &r.name, &s.code) {
                    map.push(MapEntry {
                        pou: pou.name.clone(),
                        routine: r.name.clone(),
                        rung: g.id,
                        range,
                        kind: MapKind::StLine {
                            element: s.id,
                            line,
                            code_offset,
                        },
                    });
                }
                continue;
            }
            let _ = writeln!(
                out,
                "<Routine Name=\"{}\" Type=\"RLL\">\n<RLLContent>",
                esc(&r.name)
            );
            for (n, g) in r.rungs.iter().enumerate() {
                // ST boxes: an ST routine each, called in their place.
                let mut g = g.clone();
                let mut boxes = Vec::new();
                replace_st_boxes(&mut g.elements, &r.name, &mut boxes);
                extra.extend(
                    boxes
                        .into_iter()
                        .map(|(rn, code, id)| (rn, code, g.id, id, r.name.clone())),
                );
                match rll::write(&g) {
                    Ok(text) => {
                        let _ = write!(out, "<Rung Number=\"{n}\" Type=\"N\">\n");
                        if let Some(c) = &g.comment {
                            let _ = writeln!(out, "<Comment>\n{}\n</Comment>", cdata(c));
                        }
                        out.push_str("<Text>\n<![CDATA[");
                        let start = out.len();
                        out.push_str(&text.replace("]]>", "]]]]><![CDATA[>"));
                        let end = out.len();
                        out.push_str("]]>\n</Text>\n</Rung>\n");
                        map.push(MapEntry {
                            pou: pou.name.clone(),
                            routine: r.name.clone(),
                            rung: g.id,
                            range: start..end,
                            kind: MapKind::Rung(mapped_elements(&g, &text)),
                        });
                    }
                    Err(e) => errors.push(WriteError {
                        element: e.element,
                        message: format!("{}: {}", pou.name, e.message),
                    }),
                }
            }
            out.push_str("</RLLContent>\n</Routine>\n");
        }
        for (rn, code, rung, element, routine) in extra {
            for (line, code_offset, range) in st_routine(&mut out, &rn, &code) {
                map.push(MapEntry {
                    pou: pou.name.clone(),
                    routine: routine.clone(),
                    rung,
                    range,
                    kind: MapKind::StLine {
                        element,
                        line,
                        code_offset,
                    },
                });
            }
        }
        out.push_str("</Routines>\n</Program>\n");
    }
    out.push_str("</Programs>\n<Tasks>\n");
    let default_task;
    let tasks = if project.tasks.is_empty() {
        default_task = [Task {
            name: "MainTask".into(),
            interval_ms: None,
            priority: Some(10),
            programs: project.pous.iter().map(|p| p.name.clone()).collect(),
        }];
        &default_task[..]
    } else {
        &project.tasks[..]
    };
    for t in tasks {
        let kind = match t.interval_ms {
            Some(ms) => format!("Type=\"PERIODIC\" Rate=\"{ms}\""),
            None => "Type=\"CONTINUOUS\"".to_string(),
        };
        let _ = writeln!(
            out,
            "<Task Name=\"{}\" {kind} Priority=\"{}\" Watchdog=\"500\" DisableUpdateOutputs=\"false\" InhibitTask=\"false\">\n<ScheduledPrograms>",
            esc(&t.name),
            t.priority.unwrap_or(10)
        );
        for p in &t.programs {
            let _ = writeln!(out, "<ScheduledProgram Name=\"{}\"/>", esc(p));
        }
        out.push_str("</ScheduledPrograms>\n</Task>\n");
    }
    out.push_str("</Tasks>\n</Controller>\n</RSLogix5000Content>\n");
    if errors.is_empty() {
        Ok((out, warnings, map))
    } else {
        Err(errors)
    }
}

/// The elements of a rung written as `text`, with their places in it: the
/// text read back (rung text round-trips) visits its elements in the same
/// order as the rung's own.
fn mapped_elements(rung: &Rung, text: &str) -> Vec<MappedElement> {
    let mut ids = Vec::new();
    walk(&rung.elements, &mut |e| ids.push(e.id()));
    let Ok(read) = rll::read(text, &mut Ids::new()) else {
        return Vec::new();
    };
    let mut srcs = Vec::new();
    for g in &read {
        walk(&g.elements, &mut |e| {
            let src = match e {
                Element::Contact(c) => c.src.as_ref(),
                Element::Coil(c) => c.src.as_ref(),
                Element::Branch(b) => b.src.as_ref(),
                Element::Block(b) => b.src.as_ref(),
                Element::Jump(j) => j.src.as_ref(),
                Element::Return(r) => r.src.as_ref(),
                Element::St(_) => None,
            };
            srcs.push(src.cloned());
        });
    }
    if srcs.len() != ids.len() {
        return Vec::new();
    }
    ids.into_iter()
        .zip(srcs)
        .filter_map(|(id, src)| {
            let src = src?;
            Some(MappedElement {
                id,
                span: src.span,
                operands: src.operands,
            })
        })
        .collect()
}

/// An ST routine; returns (line, offset of the line in `code`, byte range
/// of the line in `out`) for each line.
fn st_routine(
    out: &mut String,
    name: &str,
    code: &str,
) -> Vec<(usize, usize, std::ops::Range<usize>)> {
    let _ = writeln!(
        out,
        "<Routine Name=\"{}\" Type=\"ST\">\n<STContent>",
        esc(name)
    );
    let mut lines = Vec::new();
    let mut code_offset = 0;
    for (k, line) in code.split('\n').enumerate() {
        let text = line.strip_suffix('\r').unwrap_or(line);
        let _ = write!(out, "<Line Number=\"{k}\">\n<![CDATA[");
        let start = out.len();
        out.push_str(&text.replace("]]>", "]]]]><![CDATA[>"));
        lines.push((k, code_offset, start..out.len()));
        out.push_str("]]>\n</Line>\n");
        code_offset += line.len() + 1;
    }
    out.push_str("</STContent>\n</Routine>\n");
    lines
}

fn replace_st_boxes(series: &mut [Element], routine: &str, out: &mut Vec<(String, String, Id)>) {
    for e in series.iter_mut() {
        match e {
            Element::St(s) => {
                let name = format!("{routine}_ST{}", s.id);
                out.push((name.clone(), s.code.clone(), s.id));
                *e = Element::Block(Block {
                    id: s.id,
                    name: "JSR".into(),
                    pins: vec![
                        Pin {
                            name: "Routine Name".into(),
                            value: Some(name),
                            ..Default::default()
                        },
                        Pin {
                            name: "Input Count".into(),
                            value: Some("0".into()),
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                });
            }
            Element::Branch(b) => {
                for l in &mut b.legs {
                    replace_st_boxes(l, routine, out);
                }
            }
            _ => {}
        }
    }
}

fn tag(out: &mut String, v: &Variable) {
    if v.data_type.eq_ignore_ascii_case("ALIAS") {
        let _ = writeln!(
            out,
            "<Tag Name=\"{}\" TagType=\"Alias\" AliasFor=\"{}\" ExternalAccess=\"Read/Write\"/>",
            esc(&v.name),
            esc(v.initial.as_deref().unwrap_or(""))
        );
        return;
    }
    let (base, dims) = match v.data_type.split_once('[') {
        Some((b, d)) => (b.trim(), Some(d.trim_end_matches(']').replace(',', " "))),
        None => (v.data_type.trim(), None),
    };
    let up = base.to_ascii_uppercase();
    let atomic = ATOMIC.contains(&up.as_str());
    let mut attrs = format!(
        "Name=\"{}\" TagType=\"Base\" DataType=\"{}\"",
        esc(&v.name),
        esc(base)
    );
    if let Some(d) = &dims {
        let _ = write!(attrs, " Dimensions=\"{d}\"");
    }
    if atomic {
        let radix = if up == "REAL" || up == "LREAL" {
            "Float"
        } else {
            "Decimal"
        };
        let _ = write!(attrs, " Radix=\"{radix}\"");
    }
    let _ = write!(
        attrs,
        " Constant=\"{}\" ExternalAccess=\"Read/Write\"",
        v.constant
    );
    match (&v.initial, atomic && dims.is_none()) {
        (Some(i), _) if i.starts_with('[') => {
            let _ = writeln!(
                out,
                "<Tag {attrs}>\n<Data Format=\"L5K\">\n{}\n</Data>\n</Tag>",
                cdata(i)
            );
        }
        (Some(i), false) if i.starts_with('(') && dims.is_none() => {
            let mut members = String::new();
            for m in i.trim_start_matches('(').trim_end_matches(')').split(',') {
                let Some((n, val)) = m.split_once(":=") else {
                    continue;
                };
                let (n, val) = (n.trim(), val.trim());
                let ty = if ["PRE", "ACC", "LEN", "POS"].contains(&n.to_ascii_uppercase().as_str())
                    || !matches!(up.as_str(), "TIMER" | "COUNTER" | "CONTROL")
                {
                    if val.contains('.') { "REAL" } else { "DINT" }
                } else {
                    "BOOL"
                };
                let radix = if ty == "REAL" { "Float" } else { "Decimal" };
                let _ = writeln!(
                    members,
                    "<DataValueMember Name=\"{}\" DataType=\"{ty}\" Radix=\"{radix}\" Value=\"{}\"/>",
                    esc(n),
                    esc(val)
                );
            }
            let _ = writeln!(
                out,
                "<Tag {attrs}>\n<Data Format=\"Decorated\">\n<Structure DataType=\"{}\">\n{members}</Structure>\n</Data>\n</Tag>",
                esc(base)
            );
        }
        (Some(i), true) => {
            let radix = if up == "REAL" || up == "LREAL" {
                "Float"
            } else {
                "Decimal"
            };
            let _ = writeln!(
                out,
                "<Tag {attrs}>\n<Data Format=\"Decorated\">\n<DataValue DataType=\"{up}\" Radix=\"{radix}\" Value=\"{}\"/>\n</Data>\n</Tag>",
                esc(i)
            );
        }
        _ => {
            let _ = writeln!(out, "<Tag {attrs}/>");
        }
    }
}

/// The base tag of an operand (`T1.DN` → `T1`, `a[3].x` → `a`), when it is a
/// tag reference at all (not an immediate, `?`, or a module tag `Local:1:I`).
fn base_tag(op: &str) -> Option<(&str, Option<&str>, bool)> {
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
    let indexed = op[end..].starts_with('[');
    let member = op[end..]
        .strip_prefix('.')
        .map(|m| m.split(['.', '[']).next().unwrap_or(m));
    Some((base, member, indexed))
}

/// Types of the tags a Logix model's rungs use but do not declare,
/// per program (by POU name), from how they are used (1756-RM003 operand
/// types): the structure operand of TON/TOF/RTO is a TIMER, of CTU/CTD a
/// COUNTER; a member `.DN .TT .EN .PRE .ACC` means a TIMER, `.CU .CD .OV .UN`
/// a COUNTER; a contact, coil or one-shot bit a BOOL; any other operand a
/// DINT, or a REAL when an operand of the same instruction is a REAL literal.
/// An indexed tag (`a[3]`) cannot be sized and is reported.
pub fn derive_tags(
    project: &Project,
    warnings: &mut Vec<String>,
) -> BTreeMap<String, Vec<Variable>> {
    let declared: Vec<String> = project
        .globals
        .iter()
        .map(|v| v.name.to_ascii_lowercase())
        .collect();
    let mut out = BTreeMap::new();
    for pou in &project.pous {
        let mut known: Vec<String> = declared.clone();
        known.extend(pou.variables.iter().map(|v| v.name.to_ascii_lowercase()));
        let routines: Vec<String> = pou
            .routines
            .iter()
            .map(|r| r.name.to_ascii_lowercase())
            .collect();
        // name → (type, strength): structure types beat BOOL beat DINT.
        let mut found: BTreeMap<String, (String, String, u8)> = BTreeMap::new();
        let mut note = |op: &str,
                        ty: &str,
                        strength: u8,
                        found: &mut BTreeMap<String, (String, String, u8)>| {
            let Some((base, member, indexed)) = base_tag(op) else {
                return;
            };
            let key = base.to_ascii_lowercase();
            if known.contains(&key) || routines.contains(&key) {
                return;
            }
            let (ty, strength) = match member {
                Some(m)
                    if ["DN", "TT", "EN", "PRE", "ACC"]
                        .contains(&m.to_ascii_uppercase().as_str())
                        && strength < 3 =>
                {
                    ("TIMER", 3)
                }
                Some(m)
                    if ["CU", "CD", "OV", "UN"].contains(&m.to_ascii_uppercase().as_str())
                        && strength < 3 =>
                {
                    ("COUNTER", 3)
                }
                _ => (ty, strength),
            };
            if indexed {
                warnings.push(format!(
                    "tag `{base}` is indexed but not declared: its size is unknown; declare it"
                ));
            }
            let e = found
                .entry(key)
                .or_insert((base.to_string(), ty.to_string(), strength));
            if strength > e.2 {
                *e = (base.to_string(), ty.to_string(), strength);
            }
        };
        for r in &pou.routines {
            for g in &r.rungs {
                walk(&g.elements, &mut |e| match e {
                    Element::Contact(c) => note(&c.operand, "BOOL", 2, &mut found),
                    Element::Coil(c) => note(&c.operand, "BOOL", 2, &mut found),
                    Element::Block(b) => {
                        let up = b.name.to_ascii_uppercase();
                        let real = b.pins.iter().any(|p| {
                            p.value
                                .as_deref()
                                .is_some_and(|v| v.contains('.') && v.trim().parse::<f64>().is_ok())
                        });
                        let num = if real { "REAL" } else { "DINT" };
                        for (k, p) in b.pins.iter().enumerate() {
                            let Some(v) = &p.value else { continue };
                            let (ty, s) = match (up.as_str(), k) {
                                ("TON" | "TOF" | "RTO", 0) => ("TIMER", 4),
                                ("CTU" | "CTD", 0) => ("COUNTER", 4),
                                ("RES", 0) => continue,
                                ("ONS" | "OSR" | "OSF", _) => ("BOOL", 2),
                                (
                                    "JSR" | "SBR" | "RET" | "JMP" | "LBL" | "FOR" | "EVENT" | "GSV"
                                    | "SSV",
                                    _,
                                ) => continue,
                                ("CPT" | "CMP", 1) | ("CMP", 0) => {
                                    // An expression: its tag names, as numbers.
                                    for word in v.split(|c: char| {
                                        !(c.is_ascii_alphanumeric()
                                            || c == '_'
                                            || c == '.'
                                            || c == '['
                                            || c == ']')
                                    }) {
                                        if word
                                            .chars()
                                            .next()
                                            .is_some_and(|c| c.is_ascii_alphabetic())
                                            && !is_function(word)
                                        {
                                            note(word, num, 1, &mut found);
                                        }
                                    }
                                    continue;
                                }
                                _ => (num, 1),
                            };
                            note(v, ty, s, &mut found);
                        }
                    }
                    _ => {}
                });
            }
        }
        let vars: Vec<Variable> = found
            .into_values()
            .map(|(name, ty, _)| {
                warnings.push(format!(
                    "program {}: tag `{name}` is not declared; created as {ty}",
                    pou.name
                ));
                Variable {
                    name,
                    data_type: ty,
                    ..Default::default()
                }
            })
            .collect();
        out.insert(pou.name.clone(), vars);
    }
    out
}

fn is_function(word: &str) -> bool {
    const F: &[&str] = &[
        "AND", "OR", "XOR", "NOT", "MOD", "ABS", "SQR", "SQRT", "SIN", "COS", "TAN", "ASN", "ACS",
        "ATN", "LN", "LOG", "DEG", "RAD", "TRN", "FRD", "TOD",
    ];
    F.iter().any(|f| f.eq_ignore_ascii_case(word))
}
