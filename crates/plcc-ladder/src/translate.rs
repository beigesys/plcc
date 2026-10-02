// SPDX-License-Identifier: MPL-2.0

//! IEC ladder ↔ Logix ladder, element by element, with a warning for every
//! element whose behaviour differs between the dialects.
//!
//! The mapping table, its sources and the reasoning behind each choice are in
//! `docs/ladder-translation.md`. In short: contacts, coils, set/reset coils,
//! branches, jumps, labels and returns map one to one; IEC function blocks
//! (TON, TOF, CTU, CTD, R_TRIG, the plcc RTO) become Logix TIMER/COUNTER
//! instructions and back, with `Q`/`ET`/`PT` ↔ `.DN`/`.ACC`/`.PRE` and TIME ↔
//! milliseconds; comparisons, arithmetic, MOVE and the math functions map to
//! their instruction; what has no counterpart in the other dialect becomes an
//! ST box (IEC) or is reported as not translated. Structure that a dialect
//! cannot draw in place (an IEC edge contact, a counter's reset input, an
//! output copied to a variable) becomes helper rungs just before or after the
//! rung.

use crate::model::*;
use plcc_st::ast::{BinaryOp, Expression, ExpressionKind, UnaryOp};
use std::collections::HashMap;

/// Translate `p` to the dialect `to`. The warnings name each element whose
/// behaviour differs and say how; they are also kept in the element's notes.
/// Logix operands are converted as text ([`TextOperands`]); `plcc-l5x`
/// provides a converter that parses Logix expressions
/// (`plcc_l5x::ladder::LogixOperands`), for [`translate_with`].
pub fn translate(p: &Project, to: Dialect) -> (Project, Vec<String>) {
    translate_with(p, to, &TextOperands)
}

/// [`translate`] with a converter for Logix operands.
pub fn translate_with(p: &Project, to: Dialect, ops: &dyn Operands) -> (Project, Vec<String>) {
    match (p.dialect, to) {
        (a, b) if a == b => (p.clone(), Vec::new()),
        (Dialect::Iec, Dialect::Logix) => {
            let mut t = Tr::new(p, ops);
            let out = t.iec_to_logix(p);
            (out, t.warnings)
        }
        _ => {
            let mut t = Tr::new(p, ops);
            let out = t.logix_to_iec(p);
            (out, t.warnings)
        }
    }
}

/// Converts Logix operand text (a tag path, an immediate, a CPT/CMP
/// expression) to IEC ST expression text. Tag paths keep their members
/// (`T1.DN`); the translator renames TIMER/COUNTER members afterwards.
pub trait Operands {
    /// `write`: the operand is written (a coil, a destination).
    fn logix_to_iec(&self, text: &str, write: bool) -> Result<String, String>;
}

/// Logix operands as text: `&&`/`||` become AND/OR and `S:FS` the first-scan
/// flag `S_FS`; anything else is kept as written.
pub struct TextOperands;

impl Operands for TextOperands {
    fn logix_to_iec(&self, text: &str, _write: bool) -> Result<String, String> {
        let t = text.trim();
        if t == "?" {
            return Err("an unset operand `?`".into());
        }
        if t.eq_ignore_ascii_case("S:FS") {
            return Ok("S_FS".into());
        }
        if t.contains(':') {
            return Err(format!("{t} is a Logix module or system tag"));
        }
        Ok(t.replace("&&", " AND ").replace("||", " OR "))
    }
}

// ── Shared helpers ──

/// Nanoseconds of an IEC duration literal (`T#1h2m3s`, `TIME#1.5s`, `T#-5ms`).
pub fn duration_ns(text: &str) -> Option<i128> {
    let (prefix, rest) = text.trim().split_once('#')?;
    if !matches!(
        prefix.to_ascii_uppercase().as_str(),
        "T" | "TIME" | "LT" | "LTIME"
    ) {
        return None;
    }
    let mut s = rest.replace('_', "").to_ascii_lowercase();
    let neg = s.starts_with('-');
    if neg {
        s.remove(0);
    }
    let mut total: f64 = 0.0;
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut any = false;
    while i < bytes.len() {
        let start = i;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
            i += 1;
        }
        let num: f64 = s[start..i].parse().ok()?;
        let ustart = i;
        while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
            i += 1;
        }
        let unit = match &s[ustart..i] {
            "d" => 86_400e9,
            "h" => 3_600e9,
            "m" => 60e9,
            "s" => 1e9,
            "ms" => 1e6,
            "us" => 1e3,
            "ns" => 1.0,
            _ => return None,
        };
        total += num * unit;
        any = true;
    }
    if !any {
        return None;
    }
    let ns = total.round() as i128;
    Some(if neg { -ns } else { ns })
}

/// `base` and `.member` of an operand path (`T1.DN` → (`T1`, `DN`)).
fn base_member(op: &str) -> (&str, Option<&str>) {
    let op = op.trim();
    let end = op.find(['.', '[']).unwrap_or(op.len());
    let member = op[end..]
        .strip_prefix('.')
        .map(|m| m.split(['.', '[']).next().unwrap_or(m));
    (&op[..end], member)
}

fn element_name(e: &Element) -> String {
    match e {
        Element::Contact(c) => format!("contact {} ({:?})", c.operand, c.kind),
        Element::Coil(c) => format!("coil {} ({:?})", c.operand, c.kind),
        Element::Block(b) => match &b.instance {
            Some(i) => format!("{} {i}", b.name),
            None => {
                let ops: Vec<&str> = b.pins.iter().filter_map(|p| p.value.as_deref()).collect();
                format!("{}({})", b.name, ops.join(","))
            }
        },
        Element::Branch(_) => "branch".into(),
        Element::Jump(j) => format!("jump {}", j.label),
        Element::Return(_) => "return".into(),
        Element::St(_) => "ST box".into(),
    }
}

fn pin<'a>(b: &'a Block, name: &str) -> Option<&'a Pin> {
    b.pins.iter().find(|p| p.name.eq_ignore_ascii_case(name))
}

fn pin_value(b: &Block, name: &str) -> Option<String> {
    pin(b, name).and_then(|p| p.value.clone())
}

fn logix_op(b: &Block, k: usize) -> Option<String> {
    b.pins
        .get(k)
        .and_then(|p| p.value.clone())
        .filter(|v| v.trim() != "?")
}

fn contact(id: Id, operand: impl Into<String>, kind: ContactKind) -> Element {
    Element::Contact(Contact {
        id,
        operand: operand.into(),
        kind,
        ..Default::default()
    })
}

fn coil(id: Id, operand: impl Into<String>, kind: CoilKind) -> Element {
    Element::Coil(Coil {
        id,
        operand: operand.into(),
        kind,
        ..Default::default()
    })
}

fn instr(id: Id, name: &str, ops: &[(&str, String)]) -> Element {
    Element::Block(Block {
        id,
        name: name.into(),
        pins: ops
            .iter()
            .enumerate()
            .map(|(k, (n, v))| Pin {
                name: if n.is_empty() {
                    crate::catalog::logix_operand_name(name, k)
                } else {
                    n.to_string()
                },
                dir: crate::catalog::logix_operand_dir(name, k),
                value: Some(v.clone()),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    })
}

fn st_box(id: Id, code: impl Into<String>, notes: Vec<String>) -> Element {
    Element::St(StBox {
        id,
        code: code.into(),
        notes,
    })
}

fn add_note(e: &mut Element, note: &str) {
    let notes = match e {
        Element::Contact(c) => &mut c.notes,
        Element::Coil(c) => &mut c.notes,
        Element::Block(b) => &mut b.notes,
        Element::St(s) => &mut s.notes,
        _ => return,
    };
    notes.push(note.to_string());
}

/// Helper rungs a translated rung needs around it.
#[derive(Default)]
struct Around {
    before: Vec<Vec<Element>>,
    after: Vec<Vec<Element>>,
}

/// What an IEC FB instance / Logix structure tag is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fb {
    Ton,
    Tof,
    Tp,
    Rto,
    Ctu,
    Ctd,
    Ctud,
    RTrig,
    FTrig,
    Other,
}

impl Fb {
    fn of(ty: &str) -> Fb {
        match ty.trim().to_ascii_uppercase().as_str() {
            "TON" => Fb::Ton,
            "TOF" => Fb::Tof,
            "TP" => Fb::Tp,
            "RTO" => Fb::Rto,
            "CTU" => Fb::Ctu,
            "CTD" => Fb::Ctd,
            "CTUD" => Fb::Ctud,
            "R_TRIG" => Fb::RTrig,
            "F_TRIG" => Fb::FTrig,
            _ => Fb::Other,
        }
    }

    fn iec_name(self) -> &'static str {
        match self {
            Fb::Ton => "TON",
            Fb::Tof => "TOF",
            Fb::Tp => "TP",
            Fb::Rto => "RTO",
            Fb::Ctu => "CTU",
            Fb::Ctd => "CTD",
            Fb::Ctud => "CTUD",
            Fb::RTrig => "R_TRIG",
            Fb::FTrig => "F_TRIG",
            Fb::Other => "",
        }
    }

    fn is_timer(self) -> bool {
        matches!(self, Fb::Ton | Fb::Tof | Fb::Rto | Fb::Tp)
    }

    fn is_counter(self) -> bool {
        matches!(self, Fb::Ctu | Fb::Ctd | Fb::Ctud)
    }
}

struct Tr<'o> {
    ops: &'o dyn Operands,
    /// The POU being translated reads `S:FS`.
    first_scan: bool,
    /// Why the last operand could not be converted.
    why: String,
    /// A structure member without an IEC counterpart met while converting.
    member_fail: Option<String>,
    /// Variables the POU being translated needs (one-shot rung latches).
    extra_vars: Vec<Variable>,
    /// Storage bits that start TRUE (Logix prescan sets them).
    init_true: Vec<String>,
    /// Initial data of Logix tags (lower-case name → `(PRE := 50, ...)` or
    /// L5K `[0,50,0]`), for `?` preset operands.
    tag_data: HashMap<String, String>,
    warnings: Vec<String>,
    next: Id,
    /// Instance / structure tag (lower case) → kind.
    fbs: HashMap<String, Fb>,
    /// Where we are, for warnings.
    place: String,
    /// Routine names of the POU being translated (lower case).
    routines: Vec<String>,
    /// Series nesting while translating to Logix (1: the rung itself).
    depth: usize,
    /// A reset rung to split the current rung at (see `series_to_logix`).
    split: Option<Vec<Element>>,
}

impl<'o> Tr<'o> {
    fn new(p: &Project, ops: &'o dyn Operands) -> Self {
        Tr {
            ops,
            first_scan: false,
            why: String::new(),
            member_fail: None,
            extra_vars: Vec::new(),
            init_true: Vec::new(),
            tag_data: HashMap::new(),
            warnings: Vec::new(),
            next: p.max_id(),
            fbs: HashMap::new(),
            place: String::new(),
            routines: Vec::new(),
            depth: 0,
            split: None,
        }
    }

    fn id(&mut self) -> Id {
        self.next += 1;
        self.next
    }

    /// Declare a helper BOOL the translated rungs use.
    fn helper_bool(&mut self, name: &str) {
        if !self
            .extra_vars
            .iter()
            .any(|v| v.name.eq_ignore_ascii_case(name))
        {
            self.extra_vars.push(Variable {
                name: name.to_string(),
                data_type: "BOOL".into(),
                ..Default::default()
            });
        }
    }

    fn warn(&mut self, e: &mut Element, what: &str, msg: String) {
        add_note(e, &msg);
        self.warnings
            .push(format!("{}: {what} (id {}): {msg}", self.place, e.id()));
    }

    fn warn_at(&mut self, id: Id, what: &str, msg: String) {
        self.warnings
            .push(format!("{}: {what} (id {id}): {msg}", self.place));
    }

    fn translate_rungs(
        &mut self,
        pou: &Pou,
        f: &mut dyn FnMut(&mut Self, &[Element], &mut Around) -> Vec<Element>,
    ) -> Vec<Routine> {
        self.routines = pou
            .routines
            .iter()
            .map(|r| r.name.to_ascii_lowercase())
            .collect();
        let mut out = Vec::new();
        for r in &pou.routines {
            let mut rungs = Vec::new();
            for (k, g) in r.rungs.iter().enumerate() {
                self.place = format!("{}/{} rung {k}", pou.name, r.name);
                let mut around = Around::default();
                let elements = f(self, &g.elements, &mut around);
                let mut label = g.label.clone();
                for before in around.before {
                    rungs.push(Rung {
                        id: self.id(),
                        label: label.take(),
                        elements: before,
                        part_of: Some(g.id),
                        ..Default::default()
                    });
                }
                rungs.push(Rung {
                    id: g.id,
                    comment: g.comment.clone(),
                    label,
                    elements,
                    label_src: None,
                    part_of: None,
                });
                for after in around.after {
                    rungs.push(Rung {
                        id: self.id(),
                        elements: after,
                        part_of: Some(g.id),
                        ..Default::default()
                    });
                }
            }
            out.push(Routine {
                id: r.id,
                name: r.name.clone(),
                rungs,
            });
        }
        out
    }

    // ══ IEC → Logix ══

    fn iec_to_logix(&mut self, p: &Project) -> Project {
        let mut out = Project {
            dialect: Dialect::Logix,
            name: p.name.clone(),
            globals: Vec::new(),
            pous: Vec::new(),
            declarations: p.declarations.clone(),
            tasks: p.tasks.clone(),
        };
        // Instances: declared, and the ones blocks name.
        for v in p
            .globals
            .iter()
            .chain(p.pous.iter().flat_map(|q| &q.variables))
        {
            let k = Fb::of(&v.data_type);
            if k != Fb::Other {
                self.fbs.insert(v.name.to_ascii_lowercase(), k);
            }
        }
        for q in &p.pous {
            for r in &q.routines {
                for g in &r.rungs {
                    walk(&g.elements, &mut |e| {
                        if let Element::Block(b) = e
                            && let Some(i) = &b.instance
                        {
                            let k = Fb::of(&b.name);
                            self.fbs.entry(i.to_ascii_lowercase()).or_insert(k);
                        }
                    });
                }
            }
        }
        self.place = "globals".into();
        out.globals = p
            .globals
            .iter()
            .filter_map(|v| self.var_to_logix(v))
            .collect();
        for q in &p.pous {
            self.place = q.name.clone();
            if q.kind != PouKind::Program {
                self.warnings.push(format!(
                    "{}: a {:?} becomes a Logix program (Logix has no FUNCTION_BLOCK / FUNCTION in ladder; Add-On Instructions are not generated): its interface is program tags and nothing calls it",
                    q.name, q.kind
                ));
            }
            if q.variables.iter().any(|v| {
                matches!(
                    v.section,
                    VarSection::Input | VarSection::Output | VarSection::InOut
                )
            }) {
                self.warnings.push(format!(
                    "{}: VAR_INPUT / VAR_OUTPUT variables become ordinary program tags",
                    q.name
                ));
            }
            let mut variables: Vec<Variable> = q
                .variables
                .iter()
                .filter_map(|v| self.var_to_logix(v))
                .collect();
            let routines = self.translate_rungs(q, &mut |t, s, a| t.series_to_logix(s, a));
            variables.append(&mut self.extra_vars);
            out.pous.push(Pou {
                id: q.id,
                name: q.name.clone(),
                kind: PouKind::Program,
                return_type: None,
                variables,
                routines,
                members: q.members.clone(),
            });
        }
        out
    }

    fn var_to_logix(&mut self, v: &Variable) -> Option<Variable> {
        let place = self.place.clone();
        let mut w = |msg: String| {
            self.warnings
                .push(format!("{place}: variable {}: {msg}", v.name))
        };
        let (ty, initial) = match type_to_logix(&v.data_type) {
            Ok(Some(t)) => (t, v.initial.clone()),
            Ok(None) => return None, // an edge detector: its storage is a BOOL tag
            Err(msg) => {
                w(msg);
                return None;
            }
        };
        if matches!(ty.as_str(), "DINT")
            && Fb::of(&v.data_type) == Fb::Other
            && v.data_type.to_ascii_uppercase().contains("TIME")
        {
            w("TIME becomes a DINT of milliseconds".into());
        }
        let initial = initial.map(|i| match duration_ns(&i) {
            Some(ns) => (ns / 1_000_000).to_string(),
            None => i,
        });
        if v.address.is_some() {
            w(format!(
                "the direct address {} is dropped: Logix I/O comes from module tags (bind them with plcc's --io-map, docs/l5x.md)",
                v.address.as_deref().unwrap_or("")
            ));
        }
        let initial = if ty == "TIMER" || ty == "COUNTER" {
            None
        } else {
            initial
                .filter(|i| {
                    i.parse::<f64>().is_ok()
                        || i.eq_ignore_ascii_case("TRUE")
                        || i.eq_ignore_ascii_case("FALSE")
                })
                .map(|i| match i.to_ascii_uppercase().as_str() {
                    "TRUE" => "1".into(),
                    "FALSE" => "0".into(),
                    _ => i,
                })
        };
        Some(Variable {
            name: v.name.clone(),
            data_type: ty,
            section: if v.section == VarSection::Global {
                VarSection::Global
            } else {
                VarSection::Local
            },
            initial,
            address: None,
            comment: v.comment.clone(),
            constant: v.constant,
            retain: false,
        })
    }

    /// An IEC operand as a Logix operand: FB members renamed (`T1.Q` →
    /// `T1.DN`), durations in milliseconds, TRUE/FALSE as 1/0.
    fn op_to_logix(&mut self, text: &str, id: Id) -> String {
        let (e, errs) = plcc_st::parse_expression(text);
        if !errs.is_empty() {
            return text.to_string();
        }
        match self.expr_to_logix(&e) {
            Some(s) => s,
            None => {
                self.warn_at(
                    id,
                    "operand",
                    format!("`{text}` is not a Logix tag or immediate; left as written"),
                );
                text.to_string()
            }
        }
    }

    fn expr_to_logix(&mut self, e: &Expression) -> Option<String> {
        Some(match &e.kind {
            ExpressionKind::Identifier(i) => i.name.clone(),
            ExpressionKind::IntegerLiteral(v) => v.to_string(),
            ExpressionKind::RealLiteral(_) => plcc_st::print_expression(e),
            ExpressionKind::BoolLiteral(b) => (if *b { "1" } else { "0" }).into(),
            ExpressionKind::TimeLiteral(t) => (duration_ns(t)? / 1_000_000).to_string(),
            ExpressionKind::TypedLiteral { value, .. } => self.expr_to_logix(value)?,
            ExpressionKind::Parenthesized(x) => self.expr_to_logix(x)?,
            ExpressionKind::UnaryOp {
                op: UnaryOp::Neg,
                operand,
            } if matches!(
                operand.kind,
                ExpressionKind::IntegerLiteral(_) | ExpressionKind::RealLiteral(_)
            ) =>
            {
                format!("-{}", self.expr_to_logix(operand)?)
            }
            ExpressionKind::MemberAccess { object, member } => {
                let obj = self.expr_to_logix(object)?;
                let kind = match &object.kind {
                    ExpressionKind::Identifier(i) => {
                        self.fbs.get(&i.name.to_ascii_lowercase()).copied()
                    }
                    _ => None,
                };
                let m = member.name.to_ascii_uppercase();
                let renamed = match (kind, m.as_str()) {
                    (Some(k), "Q") if k.is_timer() || k == Fb::Ctu => "DN",
                    (Some(k), "ET") if k.is_timer() => "ACC",
                    (Some(k), "PT") if k.is_timer() => "PRE",
                    (Some(k), "IN") if k.is_timer() => "EN",
                    (Some(k), "CV") if k.is_counter() => "ACC",
                    (Some(k), "PV") if k.is_counter() => "PRE",
                    _ => &member.name,
                };
                format!("{obj}.{renamed}")
            }
            ExpressionKind::ArrayIndex { array, indices } => {
                let a = self.expr_to_logix(array)?;
                let ix: Option<Vec<String>> =
                    indices.iter().map(|i| self.expr_to_logix(i)).collect();
                format!("{a}[{}]", ix?.join(","))
            }
            _ => return None,
        })
    }

    fn series_to_logix(&mut self, elems: &[Element], around: &mut Around) -> Vec<Element> {
        self.depth += 1;
        let mut out = Vec::new();
        for e in elems {
            self.element_to_logix(e, around, &mut out);
            // A counter or RTO with a reset input, in the rung itself: the
            // rung up to it, then the reset, then the rest of the rung from
            // its output (IEC: reset has priority within the call).
            if self.depth == 1
                && let Some(reset) = self.split.take()
            {
                let tail = out.pop();
                around.before.push(std::mem::take(&mut out));
                if !reset.is_empty() {
                    around.before.push(reset);
                }
                out.extend(tail);
            }
        }
        self.depth -= 1;
        out
    }

    fn element_to_logix(&mut self, e: &Element, around: &mut Around, out: &mut Vec<Element>) {
        let what = element_name(e);
        match e {
            Element::Contact(c) => {
                let op = self.op_to_logix(&c.operand, c.id);
                match c.kind {
                    ContactKind::No | ContactKind::Nc => {
                        out.push(contact(c.id, op, c.kind));
                    }
                    ContactKind::Rising | ContactKind::Falling => {
                        let (ins, word) = if c.kind == ContactKind::Rising {
                            ("OSR", "rising")
                        } else {
                            ("OSF", "falling")
                        };
                        let q = format!("edge{}", c.id);
                        let sb = format!("edge{}_sb", c.id);
                        self.helper_bool(&q);
                        self.helper_bool(&sb);
                        let helper = vec![
                            contact(self.id(), op.clone(), ContactKind::No),
                            instr(self.id(), ins, &[("", sb.clone()), ("", q.clone())]),
                        ];
                        around.before.push(helper);
                        let mut x = contact(c.id, q.clone(), ContactKind::No);
                        self.warn(
                            &mut x,
                            &what,
                            format!(
                                "Logix has no {word}-edge contact: a rung `XIC({op}){ins}({sb},{q})` before this one computes the edge of {op} into {q}, read here by XIC({q}); {ins} stores the last state in {sb}, and at the first scan it does not fire for {op} already {} (IEC {} does)",
                                if c.kind == ContactKind::Rising { "TRUE" } else { "FALSE" },
                                if c.kind == ContactKind::Rising { "R_TRIG" } else { "F_TRIG" }
                            ),
                        );
                        out.push(x);
                    }
                }
            }
            Element::Coil(c) => {
                let op = self.op_to_logix(&c.operand, c.id);
                match c.kind {
                    CoilKind::Normal | CoilKind::Set | CoilKind::Reset => {
                        out.push(coil(c.id, op, c.kind));
                    }
                    CoilKind::Negated => {
                        let tmp = format!("neg{}", c.id);
                        self.helper_bool(&tmp);
                        let mut x = coil(c.id, tmp.clone(), CoilKind::Normal);
                        around.after.push(vec![
                            contact(self.id(), tmp.clone(), ContactKind::Nc),
                            coil(self.id(), op.clone(), CoilKind::Normal),
                        ]);
                        self.warn(
                            &mut x,
                            &what,
                            format!(
                                "Logix has no negated coil: OTE({tmp}) here and a rung `XIO({tmp})OTE({op})` after this one; {op} is written after the whole rung instead of at this point"
                            ),
                        );
                        out.push(x);
                    }
                    CoilKind::Rising | CoilKind::Falling => {
                        let ins = if c.kind == CoilKind::Rising {
                            "OSR"
                        } else {
                            "OSF"
                        };
                        let sb = format!("{}_sb{}", base_member(&op).0, c.id);
                        self.helper_bool(&sb);
                        let mut x = instr(c.id, ins, &[("", sb.clone()), ("", op.clone())]);
                        self.warn(
                            &mut x,
                            &what,
                            format!(
                                "{ins}({sb},{op}): the last rung state is kept in the storage bit {sb}; at the first scan {ins} does not fire (Logix prescan sets/clears {sb}), IEC's edge coil can"
                            ),
                        );
                        out.push(x);
                    }
                }
            }
            Element::Branch(b) => {
                let legs = b
                    .legs
                    .iter()
                    .map(|l| self.series_to_logix(l, around))
                    .collect();
                out.push(Element::Branch(Branch {
                    id: b.id,
                    legs,
                    src: None,
                }));
            }
            Element::Jump(j) => out.push(Element::Jump(j.clone())),
            Element::Return(r) => out.push(Element::Return(r.clone())),
            Element::St(s) => {
                let mut x = Element::St(s.clone());
                self.warn(
                    &mut x,
                    &what,
                    "written as a Logix ST routine called by JSR in its place: IEC ST and Logix ST differ (FB calls, TIME literals, standard functions); check the code".into(),
                );
                out.push(x);
            }
            Element::Block(b) => self.block_to_logix(b, around, out),
        }
    }

    fn block_to_logix(&mut self, b: &Block, around: &mut Around, out: &mut Vec<Element>) {
        let what = element_name(&Element::Block(b.clone()));
        let up = b.name.to_ascii_uppercase();
        let power_in = b.power_in.as_deref().unwrap_or("").to_ascii_uppercase();
        let power_out = b.power_out.as_deref().unwrap_or("").to_ascii_uppercase();
        // The value of a (non-power) input pin as a Logix operand, or its path.
        let input = |t: &mut Self, name: &str| -> Option<String> {
            let p = pin(b, name)?;
            if p.rung.is_some() {
                return None;
            }
            let v = p.value.as_ref()?;
            Some(t.op_to_logix(v, b.id))
        };
        let not_translated = |t: &mut Self, out: &mut Vec<Element>, why: &str| {
            let mut x = st_box(
                b.id,
                format!("(* not translated from IEC ladder: {what} *)"),
                Vec::new(),
            );
            t.warn(
                &mut x,
                &what,
                format!("NOT TRANSLATED: {why}; an empty ST box stands in its place"),
            );
            out.push(x);
        };
        match (&b.instance, up.as_str()) {
            (Some(inst), "TON" | "TOF" | "RTO") => {
                let tm = inst.clone();
                // Power on EN: the timer runs on a leg of its own, EN AND IN,
                // and the rung continues with EN (ENO) through an empty leg.
                let gated = power_in != "IN";
                let mut leg = Vec::new();
                if gated {
                    self.warn_at(
                        b.id,
                        &what,
                        format!(
                            "power enters on {power_in}: the Logix timer's rung condition is {power_in} AND IN (a leg of its own), so {power_in} FALSE resets it where the IEC timer, not called, would freeze"
                        ),
                    );
                    if let Some(p) = pin(b, "IN") {
                        if let Some(r) = &p.rung {
                            leg.extend(self.series_to_logix(r, around));
                        } else if let Some(v) = &p.value {
                            let op = self.op_to_logix(v, b.id);
                            leg.push(contact(self.id(), op, ContactKind::No));
                        }
                    }
                }
                let out_main = out;
                let out: &mut Vec<Element> = if gated { &mut leg } else { out_main };
                let preset = match pin_value(b, "PT") {
                    Some(pt) => match duration_ns(&pt) {
                        Some(ns) => (ns / 1_000_000).to_string(),
                        None => {
                            let src = self.op_to_logix(&pt, b.id);
                            around.before.push(vec![instr(
                                self.id(),
                                "MOV",
                                &[("", src.clone()), ("", format!("{tm}.PRE"))],
                            )]);
                            self.warn_at(
                                b.id,
                                &what,
                                format!("PT is not a constant: a rung `MOV({src},{tm}.PRE)` before this one sets the preset every scan ({src} must hold milliseconds)"),
                            );
                            "0".into()
                        }
                    },
                    None => "0".into(),
                };
                if up == "RTO"
                    && let Some(r) = pin(b, "R")
                {
                    let mut reset = if let Some(path) = &r.rung {
                        self.series_to_logix(path, around)
                    } else if let Some(v) = &r.value {
                        vec![contact(
                            self.id(),
                            self.op_to_logix(v, b.id),
                            ContactKind::No,
                        )]
                    } else {
                        Vec::new()
                    };
                    if !reset.is_empty() {
                        reset.push(instr(self.id(), "RES", &[("", tm.clone())]));
                        if self.depth == 1 && !gated && power_out == "Q" {
                            self.split = Some(reset);
                        } else {
                            around.before.push(reset);
                            self.warn_at(b.id, &what, format!("R is a `RES({tm})` rung before this one: a reset in the same scan as a count still lets that scan's time accumulate"));
                        }
                    }
                }
                let mut x = instr(
                    b.id,
                    &up,
                    &[("", tm.clone()), ("", preset), ("", "0".into())],
                );
                self.warn(
                    &mut x,
                    &what,
                    format!(
                        "the IEC {up} instance {tm} becomes the TIMER tag {tm}: Q is .DN, ET is .ACC and PT is .PRE, in whole milliseconds (DINT, not TIME); .ACC may pass .PRE by up to one scan where IEC ET stops at PT{}",
                        if up == "RTO" { "; R becomes RES(…)" } else { "" }
                    ),
                );
                out.push(x);
                self.fb_outputs_to_logix(b, &tm, around, out, &power_out, |m| match m {
                    "Q" => Some(format!("{tm}.DN")),
                    "ET" => Some(format!("{tm}.ACC")),
                    _ => None,
                });
                self.continue_from_output(b.id, &what, &up, &tm, gated, &power_out);
                if gated {
                    let mut leg = std::mem::take(out);
                    // A power output other than ENO continues the rung after
                    // the branch.
                    let tail = (!power_out.is_empty() && power_out != "ENO")
                        .then(|| leg.pop())
                        .flatten();
                    let id = self.id();
                    out_main.push(Element::Branch(Branch {
                        id,
                        legs: vec![leg, Vec::new()],
                        src: None,
                    }));
                    out_main.extend(tail);
                }
            }
            (Some(inst), "CTU" | "CTD") => {
                let c = inst.clone();
                let pv = pin_value(b, "PV").unwrap_or_else(|| "0".into());
                let preset = if pv.trim().parse::<i64>().is_ok() {
                    pv.trim().to_string()
                } else {
                    let src = self.op_to_logix(&pv, b.id);
                    around.before.push(vec![instr(
                        self.id(),
                        "MOV",
                        &[("", src.clone()), ("", format!("{c}.PRE"))],
                    )]);
                    "0".into()
                };
                let edge_pin = if up == "CTU" { "CU" } else { "CD" };
                let gated = !power_in.eq_ignore_ascii_case(edge_pin);
                let reset_pin = if up == "CTU" { "R" } else { "LD" };
                if let Some(r) = pin(b, reset_pin) {
                    let mut path = if let Some(p) = &r.rung {
                        self.series_to_logix(p, around)
                    } else if let Some(v) = &r.value {
                        vec![contact(
                            self.id(),
                            self.op_to_logix(v, b.id),
                            ContactKind::No,
                        )]
                    } else {
                        Vec::new()
                    };
                    if !path.is_empty() {
                        if up == "CTU" {
                            // Not RES: RES also clears .CU, the counter's edge
                            // memory, so a count input still TRUE would count
                            // again; IEC R leaves the edge memory alone.
                            let id = self.id();
                            let clear = [
                                instr(self.id(), "CLR", &[("", format!("{c}.ACC"))]),
                                coil(self.id(), format!("{c}.DN"), CoilKind::Reset),
                                coil(self.id(), format!("{c}.OV"), CoilKind::Reset),
                                coil(self.id(), format!("{c}.UN"), CoilKind::Reset),
                            ];
                            path.push(Element::Branch(Branch {
                                id,
                                legs: clear.into_iter().map(|e| vec![e]).collect(),
                                src: None,
                            }));
                        } else {
                            path.push(instr(
                                self.id(),
                                "MOV",
                                &[("", format!("{c}.PRE")), ("", format!("{c}.ACC"))],
                            ));
                        }
                        if self.depth == 1 && !gated && power_out == "Q" {
                            self.split = Some(path);
                            self.warn_at(b.id, &what, format!("the rung is split after the counter: the {reset_pin} rung follows it, then the rest of the rung from {c}.DN, so {reset_pin} wins over a count in the same scan as in IEC"));
                        } else {
                            around.before.push(path);
                            self.warn_at(b.id, &what, format!("{reset_pin} is a rung before the counter's: a count in the same scan as the reset still happens after it (IEC: {reset_pin} wins)"));
                        }
                    }
                }
                let mut leg = Vec::new();
                if gated {
                    self.warn_at(
                        b.id,
                        &what,
                        format!(
                            "power enters on {power_in}: the Logix counter's rung condition is {power_in} AND {edge_pin} (a leg of its own), so while {power_in} is FALSE the counter still sees {edge_pin} fall, and counts when both are TRUE again (IEC: not called, it keeps its edge memory)"
                        ),
                    );
                    if let Some(p) = pin(b, edge_pin) {
                        if let Some(r) = &p.rung {
                            leg.extend(self.series_to_logix(r, around));
                        } else if let Some(v) = &p.value {
                            let op = self.op_to_logix(v, b.id);
                            leg.push(contact(self.id(), op, ContactKind::No));
                        }
                    }
                }
                let out_main = out;
                let out: &mut Vec<Element> = if gated { &mut leg } else { out_main };
                let mut x = instr(
                    b.id,
                    &up,
                    &[("", c.clone()), ("", preset), ("", "0".into())],
                );
                let msg = if up == "CTU" {
                    format!(
                        "the IEC CTU instance {c} becomes the COUNTER tag {c}: Q is .DN, CV is .ACC, PV is .PRE; R clears .ACC, .DN, .OV and .UN (not RES, which would also clear .CU and count again); .ACC is a DINT that wraps past 2,147,483,647 and sets .OV, IEC CV is an INT that stops at 32,767"
                    )
                } else {
                    format!(
                        "the IEC CTD instance {c} becomes the COUNTER tag {c}: CV is .ACC, PV is .PRE, LD is MOV({c}.PRE,{c}.ACC); Logix .DN is .ACC >= .PRE, not IEC Q (CV <= 0), so Q is read as LEQ({c}.ACC,0)"
                    )
                };
                self.warn(&mut x, &what, msg);
                out.push(x);
                let is_ctu = up == "CTU";
                self.continue_from_output(b.id, &what, &up, &c, gated, &power_out);
                self.fb_outputs_to_logix(b, &c, around, out, &power_out, |m| match m {
                    "Q" if is_ctu => Some(format!("{c}.DN")),
                    "CV" => Some(format!("{c}.ACC")),
                    _ => None,
                });
                if !is_ctu && power_out == "Q" {
                    out.push(instr(
                        self.id(),
                        "LEQ",
                        &[("", format!("{c}.ACC")), ("", "0".into())],
                    ));
                }
                if gated {
                    let mut leg = std::mem::take(out);
                    // A power output other than ENO continues the rung after
                    // the branch.
                    let tail = (!power_out.is_empty() && power_out != "ENO")
                        .then(|| leg.pop())
                        .flatten();
                    let id = self.id();
                    out_main.push(Element::Branch(Branch {
                        id,
                        legs: vec![leg, Vec::new()],
                        src: None,
                    }));
                    out_main.extend(tail);
                }
            }
            (Some(inst), "R_TRIG") if power_in == "CLK" => {
                let sb = format!("{inst}_sb");
                self.helper_bool(&sb);
                let mut x = instr(b.id, "ONS", &[("", sb.clone())]);
                self.warn(
                    &mut x,
                    &what,
                    format!("R_TRIG {inst} becomes ONS({sb}): the storage bit {sb} holds the last rung state; at the first scan ONS does not fire for a rung already TRUE (Logix prescan sets {sb}), R_TRIG does"),
                );
                out.push(x);
                if power_out != "Q" {
                    self.warn_at(
                        b.id,
                        &what,
                        "R_TRIG's Q is only available as the rung (power_out Q)".into(),
                    );
                }
            }
            (Some(_), _) => not_translated(
                self,
                out,
                &format!(
                    "the function block {} has no Logix instruction (Add-On Instructions are not generated)",
                    b.name
                ),
            ),
            (None, "GT" | "GE" | "EQ" | "LE" | "LT" | "NE") => {
                let ins = match up.as_str() {
                    "GT" => "GRT",
                    "GE" => "GEQ",
                    "EQ" => "EQU",
                    "LE" => "LEQ",
                    "LT" => "LES",
                    _ => "NEQ",
                };
                let (Some(a), Some(c)) = (input(self, "IN1"), input(self, "IN2")) else {
                    return not_translated(self, out, "a compare needs two inputs with values");
                };
                if power_out != "OUT"
                    || pin(b, "OUT").is_some_and(|p| p.value.is_some() || p.rung.is_some())
                {
                    return not_translated(
                        self,
                        out,
                        "the compare result is not the rung (it goes to a variable)",
                    );
                }
                let mut x = instr(b.id, ins, &[("", a), ("", c)]);
                if !power_in.is_empty() {
                    self.warn(
                        &mut x,
                        &what,
                        format!("{ins} is an input instruction: the rung continues with (rung AND result); the IEC box's result ignored {power_in}"),
                    );
                }
                out.push(x);
            }
            (None, "ADD" | "SUB" | "MUL" | "DIV" | "MOD" | "EXPT" | "AND" | "OR" | "XOR") => {
                let ins = match up.as_str() {
                    "EXPT" => "XPY",
                    x => x,
                };
                let inputs: Vec<&Pin> = b
                    .pins
                    .iter()
                    .filter(|p| p.dir != PinDir::Output && !p.name.eq_ignore_ascii_case("EN"))
                    .collect();
                let dest = pin(b, "OUT").and_then(|p| p.value.clone());
                let Some(dest) = dest else {
                    return not_translated(self, out, "the result is not written to a variable");
                };
                let dest = self.op_to_logix(&dest, b.id);
                let vals: Option<Vec<String>> = inputs
                    .iter()
                    .map(|p| p.value.as_ref().map(|v| self.op_to_logix(v, b.id)))
                    .collect();
                let Some(vals) = vals else {
                    return not_translated(self, out, "an input is not a variable or constant");
                };
                let mut x = if vals.len() == 2 {
                    instr(
                        b.id,
                        ins,
                        &[("", vals[0].clone()), ("", vals[1].clone()), ("", dest)],
                    )
                } else {
                    let sym = match up.as_str() {
                        "ADD" => "+",
                        "MUL" => "*",
                        "AND" => " AND ",
                        "OR" => " OR ",
                        _ => " XOR ",
                    };
                    instr(b.id, "CPT", &[("", dest), ("", vals.join(sym))])
                };
                self.warn(
                    &mut x,
                    &what,
                    "Logix computes in the widest type of the operands and the destination, stores with truncation (integers) or round-half-even (REAL to integer), sets S:V on overflow and does not fault on division by zero the way IEC does; logical instructions zero-fill SINT/INT operands".into(),
                );
                if !(power_in == "EN" || power_in.is_empty()) || power_out == "OUT" {
                    self.warn_at(
                        b.id,
                        &what,
                        "the result is not the rung in Logix: the rung continues unchanged".into(),
                    );
                }
                out.push(x);
            }
            (
                None,
                "MOVE" | "NOT" | "ABS" | "SQRT" | "SIN" | "COS" | "TAN" | "ASIN" | "ACOS" | "ATAN"
                | "LN" | "LOG" | "TRUNC",
            ) => {
                let ins = match up.as_str() {
                    "MOVE" => "MOV",
                    "SQRT" => "SQR",
                    "ASIN" => "ASN",
                    "ACOS" => "ACS",
                    "ATAN" => "ATN",
                    "TRUNC" => "TRN",
                    x => x,
                };
                let src = b
                    .pins
                    .iter()
                    .find(|p| p.dir != PinDir::Output && !p.name.eq_ignore_ascii_case("EN"))
                    .and_then(|p| p.value.clone());
                let dest = b
                    .pins
                    .iter()
                    .find(|p| p.dir == PinDir::Output && !p.name.eq_ignore_ascii_case("ENO"))
                    .and_then(|p| p.value.clone());
                let (Some(src), Some(dest)) = (src, dest) else {
                    return not_translated(self, out, "the input or the result is not a variable");
                };
                let src = self.op_to_logix(&src, b.id);
                let dest = self.op_to_logix(&dest, b.id);
                let mut x = instr(b.id, ins, &[("", src), ("", dest)]);
                if ins != "MOV" {
                    self.warn(&mut x, &what, "Logix evaluates in REAL/LREAL and stores into the destination's type with its rounding and status flags".into());
                }
                out.push(x);
            }
            (None, name) if self.routines.contains(&name.to_ascii_lowercase()) => {
                // A call of another routine of this POU (an action).
                out.push(instr(
                    b.id,
                    "JSR",
                    &[("", b.name.clone()), ("", "0".into())],
                ));
            }
            (None, _) => not_translated(
                self,
                out,
                &format!("the function {} has no Logix instruction", b.name),
            ),
        }
    }

    /// A TOF, RTO or counter whose Q continues the rung: its .DN can be TRUE
    /// on a FALSE rung (TOF timing out, a held RTO or count), so the rest of
    /// the rung must start from .DN alone. In the rung itself the rung is
    /// split there; inside a branch it cannot be, which is warned.
    fn continue_from_output(
        &mut self,
        id: Id,
        what: &str,
        up: &str,
        tag: &str,
        gated: bool,
        power_out: &str,
    ) {
        if up == "TON" || power_out != "Q" {
            return;
        }
        if self.depth == 1 && !gated {
            if self.split.is_none() {
                self.split = Some(Vec::new());
                self.warn_at(id, what, format!("the rung is split after {up}({tag}): the rest of it continues from {tag}.DN in a rung of its own, as IEC continues with Q whatever the rung before it"));
            }
        } else {
            self.warn_at(id, what, format!("the rung continues as (rung AND {tag}.DN); IEC continues with Q alone, which can be TRUE on a FALSE rung"));
        }
    }

    /// Output pins of an FB translated to Logix: values copied after the rung,
    /// pin paths continued from the member, and the rung continued with the
    /// power-out member.
    fn fb_outputs_to_logix(
        &mut self,
        b: &Block,
        tag: &str,
        around: &mut Around,
        out: &mut Vec<Element>,
        power_out: &str,
        member: impl Fn(&str) -> Option<String>,
    ) {
        let what = element_name(&Element::Block(b.clone()));
        for p in b.pins.iter().filter(|p| p.dir == PinDir::Output) {
            let up = p.name.to_ascii_uppercase();
            let Some(m) = member(&up) else {
                if p.value.is_some() || p.rung.is_some() || power_out == up && up != "ENO" {
                    self.warn_at(
                        b.id,
                        &what,
                        format!("output {up} of {tag} has no Logix counterpart; not translated"),
                    );
                }
                continue;
            };
            if let Some(v) = &p.value {
                let dest = self.op_to_logix(v, b.id);
                if up == "Q" {
                    around.after.push(vec![
                        contact(self.id(), m.clone(), ContactKind::No),
                        coil(self.id(), dest.clone(), CoilKind::Normal),
                    ]);
                } else {
                    around.after.push(vec![instr(
                        self.id(),
                        "MOV",
                        &[("", m.clone()), ("", dest.clone())],
                    )]);
                    let unit = if up == "ET" {
                        "in milliseconds (DINT, not TIME)"
                    } else {
                        "as a DINT"
                    };
                    self.warn_at(
                        b.id,
                        &what,
                        format!("{up} → {m} is copied to {dest} by a rung after this one, {unit}"),
                    );
                }
            }
            if let Some(r) = &p.rung {
                let mut path = vec![contact(self.id(), m.clone(), ContactKind::No)];
                path.extend(self.series_to_logix(r, around));
                around.after.push(path);
            }
            if power_out == up && up != "ENO" {
                out.push(contact(self.id(), m, ContactKind::No));
            }
        }
    }

    // ══ Logix → IEC ══

    fn logix_to_iec(&mut self, p: &Project) -> Project {
        let mut out = Project {
            dialect: Dialect::Iec,
            name: p.name.clone(),
            globals: Vec::new(),
            pous: Vec::new(),
            declarations: p.declarations.clone(),
            tasks: p.tasks.clone(),
        };
        // TIMER / COUNTER tags: which instruction uses them.
        for q in &p.pous {
            for r in &q.routines {
                for g in &r.rungs {
                    walk(&g.elements, &mut |e| {
                        if let Element::Block(b) = e
                            && let Some(tag) = logix_op(b, 0)
                        {
                            let k = match b.name.to_ascii_uppercase().as_str() {
                                "TON" => Fb::Ton,
                                "TOF" => Fb::Tof,
                                "RTO" => Fb::Rto,
                                "CTU" => Fb::Ctu,
                                "CTD" => Fb::Ctd,
                                _ => return,
                            };
                            let key = base_member(&tag).0.to_ascii_lowercase();
                            // CTU and CTD on one COUNTER: an up/down counter.
                            let k = match self.fbs.get(&key) {
                                Some(Fb::Ctu | Fb::Ctd | Fb::Ctud)
                                    if matches!(k, Fb::Ctu | Fb::Ctd)
                                        && self.fbs.get(&key) != Some(&k) =>
                                {
                                    Fb::Ctud
                                }
                                _ => k,
                            };
                            if let Some(old) = self.fbs.insert(key, k)
                                && old != k
                                && k != Fb::Ctud
                            {
                                self.warnings.push(format!(
                                    "{}: tag {tag} is used by both {} and {}: it becomes one IEC {} instance",
                                    q.name,
                                    old.iec_name(),
                                    k.iec_name(),
                                    k.iec_name()
                                ));
                            }
                        }
                    });
                }
            }
        }
        for v in p
            .globals
            .iter()
            .chain(p.pous.iter().flat_map(|q| &q.variables))
        {
            if let Some(i) = &v.initial {
                self.tag_data.insert(v.name.to_ascii_lowercase(), i.clone());
            }
        }
        self.warnings.push(
            "prescan: a Logix controller clears OTE outputs and timer/counter status bits (and sets one-shot storage bits) on the transition to Run; IEC variables start from their initial values — the same at a cold start, not after a warm restart".into(),
        );
        self.place = "controller tags".into();
        out.globals = p
            .globals
            .iter()
            .filter_map(|v| self.var_to_iec(v, VarSection::Global))
            .collect();
        for q in &p.pous {
            self.place = q.name.clone();
            self.first_scan = false;
            let mut variables: Vec<Variable> = q
                .variables
                .iter()
                .filter_map(|v| self.var_to_iec(v, VarSection::Local))
                .collect();
            let mut routines = self.translate_rungs(q, &mut |t, s, a| t.series_to_iec(s, a));
            variables.append(&mut self.extra_vars);
            if self.first_scan {
                // S:FS: TRUE during the program's first scan.
                variables.push(Variable {
                    name: "S_FS".into(),
                    data_type: "BOOL".into(),
                    initial: Some("TRUE".into()),
                    ..Default::default()
                });
                if let Some(main) = routines.first_mut() {
                    let (rid, bid) = (self.id(), self.id());
                    main.rungs.push(Rung {
                        id: rid,
                        comment: Some("S:FS (first scan) ends with the first scan".into()),
                        elements: vec![st_box(bid, "S_FS := FALSE;", Vec::new())],
                        ..Default::default()
                    });
                }
                self.warnings.push(format!("{}: S:FS becomes the program variable S_FS, TRUE until the end of the first scan of this program", q.name));
            }
            for v in &mut variables {
                if self
                    .init_true
                    .iter()
                    .any(|n| n.eq_ignore_ascii_case(&v.name))
                {
                    v.initial = Some("TRUE".into());
                }
            }
            out.pous.push(Pou {
                id: q.id,
                name: q.name.clone(),
                kind: PouKind::Program,
                return_type: None,
                variables,
                routines,
                members: q.members.clone(),
            });
        }
        for v in &mut out.globals {
            if self
                .init_true
                .iter()
                .any(|n| n.eq_ignore_ascii_case(&v.name))
            {
                v.initial = Some("TRUE".into());
            }
        }
        out
    }

    fn var_to_iec(&mut self, v: &Variable, section: VarSection) -> Option<Variable> {
        let place = self.place.clone();
        let (base, dims) = match v.data_type.split_once('[') {
            Some((b, d)) => (
                b.trim().to_string(),
                Some(d.trim_end_matches(']').to_string()),
            ),
            None => (v.data_type.trim().to_string(), None),
        };
        let up = base.to_ascii_uppercase();
        let ty = match up.as_str() {
            "BOOL" | "SINT" | "INT" | "DINT" | "LINT" | "USINT" | "UINT" | "UDINT" | "ULINT"
            | "REAL" | "LREAL" => up.clone(),
            "TIMER" => self
                .fbs
                .get(&v.name.to_ascii_lowercase())
                .map(|k| k.iec_name().to_string())
                .unwrap_or_else(|| "TON".into()),
            "COUNTER" => self
                .fbs
                .get(&v.name.to_ascii_lowercase())
                .map(|k| k.iec_name().to_string())
                .unwrap_or_else(|| "CTU".into()),
            "STRING" => "STRING[82]".into(),
            "ALIAS" => {
                self.warnings.push(format!(
                    "{place}: alias tag {} (for {}) is not translated; use the target",
                    v.name,
                    v.initial.as_deref().unwrap_or("?")
                ));
                return None;
            }
            _ => {
                self.warnings.push(format!(
                    "{place}: tag {} of type {} has no IEC counterpart here (UDTs and CONTROL are not in the ladder model); declared as that type name",
                    v.name, v.data_type
                ));
                base.clone()
            }
        };
        let ty = match dims {
            Some(d) => {
                let ranges: Vec<String> = d
                    .split([',', ' '])
                    .filter(|x| !x.is_empty())
                    .map(|n| format!("0..{}", n.trim().parse::<i64>().unwrap_or(1) - 1))
                    .collect();
                format!("ARRAY[{}] OF {ty}", ranges.join(", "))
            }
            None => ty,
        };
        let initial = v
            .initial
            .clone()
            .filter(|i| !(i.starts_with('(') || i.starts_with('[')) && Fb::of(&ty) == Fb::Other);
        let initial = match (up.as_str(), initial) {
            ("BOOL", Some(i)) => Some(if i.trim() == "0" { "FALSE" } else { "TRUE" }.to_string()),
            (_, i) => i,
        };
        Some(Variable {
            name: v.name.clone(),
            data_type: ty,
            section,
            initial,
            address: None,
            comment: v.comment.clone(),
            constant: v.constant,
            retain: false,
        })
    }

    /// A Logix operand as an IEC expression: converted by the operand
    /// converter, then TIMER/COUNTER members renamed (`T1.DN` → `T1.Q`,
    /// `T1.ACC` → `TIME_TO_DINT(T1.ET)`).
    fn op_to_iec(&mut self, text: &str, id: Id) -> Option<String> {
        self.op_to_iec_rw(text, id, false)
    }

    /// `None` when the operand has no IEC form (the reason is kept for the
    /// element's NOT TRANSLATED warning).
    fn op_to_iec_rw(&mut self, text: &str, id: Id, write: bool) -> Option<String> {
        let t = text.trim();
        let iec = match self.ops.logix_to_iec(t, write) {
            Ok(s) => s,
            Err(m) => {
                self.why = format!("operand {t}: {m}");
                return None;
            }
        };
        let (e, errs) = plcc_st::parse_expression(&iec);
        if !errs.is_empty() {
            self.why = format!("operand {t} is not an IEC expression ({iec})");
            return None;
        }
        self.member_fail = None;
        let e = self.rewrite_members(e, id);
        if let Some(why) = self.member_fail.take() {
            self.why = why;
            return None;
        }
        let mut uses_fs = false;
        mentions(&e, "S_FS", &mut uses_fs);
        self.first_scan |= uses_fs;
        Some(plcc_st::print_expression(&e))
    }

    /// Rename the members of TIMER / COUNTER tags to those of the IEC
    /// instance they become.
    fn rewrite_members(&mut self, e: Expression, id: Id) -> Expression {
        let Expression { kind, span } = e;
        let kind = match kind {
            ExpressionKind::MemberAccess { object, member } => {
                if let ExpressionKind::Identifier(base) = &object.kind
                    && let Some(k) = self.fbs.get(&base.name.to_ascii_lowercase()).copied()
                {
                    let b = &base.name;
                    let m_up = member.name.to_ascii_uppercase();
                    let mapped = if k.is_timer() {
                        match m_up.as_str() {
                            "DN" => Some(format!("{b}.Q")),
                            "ACC" => Some(format!("TIME_TO_DINT({b}.ET)")),
                            "PRE" => Some(format!("TIME_TO_DINT({b}.PT)")),
                            "EN" => Some(format!("{b}.IN")),
                            "TT" => Some(format!("({b}.IN AND NOT {b}.Q)")),
                            _ => None,
                        }
                    } else {
                        match (k, m_up.as_str()) {
                            (Fb::Ctu, "DN") => Some(format!("{b}.Q")),
                            (Fb::Ctd, "DN") => Some(format!("({b}.CV >= {b}.PV)")),
                            (Fb::Ctud, "DN") => Some(format!("{b}.QU")),
                            (_, "ACC") => Some(format!("{b}.CV")),
                            (_, "PRE") => Some(format!("{b}.PV")),
                            (Fb::Ctu, "CU") => Some(format!("{b}.CU")),
                            (Fb::Ctd, "CD") => Some(format!("{b}.CD")),
                            _ => None,
                        }
                    };
                    let _ = id;
                    match mapped.map(|m| plcc_st::parse_expression(&m)) {
                        Some((x, errs)) if errs.is_empty() => return x,
                        _ => {
                            self.member_fail = Some(format!(
                                "{b}.{}: member {} of the {} {b} has no IEC counterpart",
                                member.name,
                                member.name,
                                k.iec_name()
                            ));
                        }
                    }
                }
                ExpressionKind::MemberAccess {
                    object: Box::new(self.rewrite_members(*object, id)),
                    member,
                }
            }
            ExpressionKind::BinaryOp { op, left, right } => ExpressionKind::BinaryOp {
                op,
                left: Box::new(self.rewrite_members(*left, id)),
                right: Box::new(self.rewrite_members(*right, id)),
            },
            ExpressionKind::UnaryOp { op, operand } => ExpressionKind::UnaryOp {
                op,
                operand: Box::new(self.rewrite_members(*operand, id)),
            },
            ExpressionKind::Parenthesized(x) => {
                ExpressionKind::Parenthesized(Box::new(self.rewrite_members(*x, id)))
            }
            ExpressionKind::FunctionCall { callee, args } => ExpressionKind::FunctionCall {
                callee,
                args: args
                    .into_iter()
                    .map(|mut a| {
                        a.value = self.rewrite_members(a.value, id);
                        a
                    })
                    .collect(),
            },
            ExpressionKind::ArrayIndex { array, indices } => ExpressionKind::ArrayIndex {
                array: Box::new(self.rewrite_members(*array, id)),
                indices: indices
                    .into_iter()
                    .map(|i| self.rewrite_members(i, id))
                    .collect(),
            },
            other => other,
        };
        Expression { kind, span }
    }

    /// The counter a branch of `CLR(C.ACC)` / `OTU(C.DN|OV|UN)` legs clears
    /// (what the IEC → Logix translation makes of a CTU's R input).
    fn counter_clear(&self, b: &Branch) -> Option<String> {
        let mut tag: Option<String> = None;
        let mut clr = false;
        for leg in &b.legs {
            let [e] = leg.as_slice() else { return None };
            let (base, member) = match e {
                Element::Block(x) if x.name.eq_ignore_ascii_case("CLR") => {
                    let v = x.pins.first()?.value.clone()?;
                    let (bse, m) = base_member(&v);
                    if !m?.eq_ignore_ascii_case("ACC") {
                        return None;
                    }
                    clr = true;
                    (bse.to_string(), "ACC".to_string())
                }
                Element::Coil(c) if c.kind == CoilKind::Reset => {
                    let (bse, m) = base_member(&c.operand);
                    (bse.to_string(), m?.to_ascii_uppercase())
                }
                _ => return None,
            };
            if !["ACC", "DN", "OV", "UN"].contains(&member.as_str()) {
                return None;
            }
            match &tag {
                None => tag = Some(base),
                Some(t) if t.eq_ignore_ascii_case(&base) => {}
                _ => return None,
            }
        }
        let t = tag?;
        (clr && matches!(
            self.fbs.get(&t.to_ascii_lowercase()),
            Some(Fb::Ctu | Fb::Ctud)
        ))
        .then_some(t)
    }

    /// An element whose operand has no IEC form: an ST box in its place.
    fn untranslatable(&mut self, id: Id, what: &str, out: &mut Vec<Element>) {
        let why = std::mem::take(&mut self.why);
        let mut x = st_box(
            id,
            format!("(* not translated from Logix: {what} *)"),
            Vec::new(),
        );
        self.warn(
            &mut x,
            what,
            format!("NOT TRANSLATED: {why}; an empty ST box stands in its place"),
        );
        out.push(x);
    }

    fn series_to_iec(&mut self, elems: &[Element], around: &mut Around) -> Vec<Element> {
        let mut out = Vec::new();
        for e in elems {
            self.element_to_iec(e, around, &mut out);
        }
        out
    }

    fn element_to_iec(&mut self, e: &Element, around: &mut Around, out: &mut Vec<Element>) {
        let what = element_name(e);
        match e {
            Element::Contact(c) => match self.op_to_iec(&c.operand, c.id) {
                Some(op) => out.push(contact(c.id, op, c.kind)),
                None => self.untranslatable(c.id, &what, out),
            },
            Element::Coil(c) => match self.op_to_iec_rw(&c.operand, c.id, true) {
                Some(op) => out.push(coil(c.id, op, c.kind)),
                None => self.untranslatable(c.id, &what, out),
            },
            Element::Branch(b) if self.counter_clear(b).is_some() => {
                // `[CLR(C.ACC) ,OTU(C.DN) ,OTU(C.OV) ,OTU(C.UN) ]`, the reset
                // an IEC CTU's R becomes in Logix: the instance's R again.
                let c = self.counter_clear(b).unwrap_or_default();
                out.push(st_box(
                    b.id,
                    format!("{c}(R := TRUE);\n{c}(R := FALSE);"),
                    Vec::new(),
                ));
            }
            Element::Branch(b) => {
                let legs = b
                    .legs
                    .iter()
                    .map(|l| self.series_to_iec(l, around))
                    .collect();
                out.push(Element::Branch(Branch {
                    id: b.id,
                    legs,
                    src: None,
                }));
            }
            Element::Jump(j) => out.push(Element::Jump(Jump {
                id: j.id,
                label: j.label.clone(),
                src: None,
            })),
            Element::Return(r) => out.push(Element::Return(Return {
                id: r.id,
                src: None,
            })),
            Element::St(s) => {
                let mut x = Element::St(s.clone());
                self.warn(&mut x, &what, "Logix ST taken over as IEC ST: the syntax is close but not the same (Logix instructions in ST, SIZE, string functions); check the code".into());
                out.push(x);
            }
            Element::Block(b) => self.block_to_iec(b, around, out),
        }
    }

    fn block_to_iec(&mut self, b: &Block, _around: &mut Around, out: &mut Vec<Element>) {
        let what = element_name(&Element::Block(b.clone()));
        let up = b.name.to_ascii_uppercase();
        let ops: Vec<Option<String>> = (0..b.pins.len()).map(|k| logix_op(b, k)).collect();
        let op = |t: &mut Self, k: usize| -> Option<String> {
            ops.get(k)
                .cloned()
                .flatten()
                .and_then(|v| t.op_to_iec(&v, b.id))
        };
        let fb =
            |name: &str, inst: String, pins: Vec<Pin>, power_in: &str, power_out: Option<&str>| {
                Element::Block(Block {
                    id: b.id,
                    name: name.into(),
                    instance: Some(inst),
                    pins,
                    power_in: Some(power_in.into()),
                    power_out: power_out.map(str::to_string),
                    notes: Vec::new(),
                    src: None,
                })
            };
        let pin_in = |n: &str, v: Option<String>| Pin {
            name: n.into(),
            dir: PinDir::Input,
            value: v,
            ..Default::default()
        };
        let pin_out = |n: &str, v: Option<String>| Pin {
            name: n.into(),
            dir: PinDir::Output,
            value: v,
            ..Default::default()
        };
        let func = |name: &str, ins: Vec<(&str, String)>, dest: Option<String>| {
            let mut pins = vec![pin_in("EN", None)];
            for (n, v) in ins {
                pins.push(pin_in(n, Some(v)));
            }
            pins.push(pin_out("ENO", None));
            pins.push(pin_out("OUT", dest));
            Element::Block(Block {
                id: b.id,
                name: name.into(),
                pins,
                power_in: Some("EN".into()),
                power_out: Some("ENO".into()),
                ..Default::default()
            })
        };
        let not_translated = |t: &mut Self, out: &mut Vec<Element>, why: String| {
            let why = if t.why.is_empty() {
                why
            } else {
                format!("{why} ({})", std::mem::take(&mut t.why))
            };
            let text = format!(
                "{}({})",
                b.name,
                b.pins
                    .iter()
                    .map(|p| p.value.clone().unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join(",")
            );
            let mut x = st_box(
                b.id,
                format!("(* not translated from Logix: {text} *)"),
                Vec::new(),
            );
            t.warn(
                &mut x,
                &what,
                format!("NOT TRANSLATED: {why}; an empty ST box stands in its place"),
            );
            out.push(x);
        };
        match up.as_str() {
            "TON" | "TOF" | "RTO" => {
                let Some(tag) = op(self, 0) else {
                    return not_translated(self, out, "no timer operand".into());
                };
                let pre = ops
                    .get(1)
                    .cloned()
                    .flatten()
                    .or_else(|| {
                        self.tag_data
                            .get(&tag.to_ascii_lowercase())
                            .and_then(|d| preset_of(d))
                    })
                    .unwrap_or_else(|| "0".into());
                let pt = match pre.trim().parse::<i64>() {
                    Ok(ms) => format!("T#{ms}ms"),
                    Err(_) => format!(
                        "DINT_TO_TIME({})",
                        self.op_to_iec(&pre, b.id).unwrap_or_else(|| pre.clone())
                    ),
                };
                let mut pins = vec![pin_in("IN", None)];
                if up == "RTO" {
                    // Released on every call; RES sets it for one call.
                    pins.push(pin_in("R", Some("FALSE".into())));
                }
                pins.push(pin_in("PT", Some(pt)));
                pins.push(pin_out("Q", None));
                pins.push(pin_out("ET", None));
                let mut x = fb(&up, tag.clone(), pins, "IN", None);
                let acc = ops.get(2).cloned().flatten().unwrap_or_default();
                let mut msg = format!(
                    "the TIMER tag {tag} becomes an IEC {up} instance: .DN is Q, .ACC is TIME_TO_DINT(ET) (ms), .PRE is PT; ET stops at PT where .ACC can pass .PRE by one scan; .TT and .EN are derived from IN and Q"
                );
                if up == "RTO" {
                    msg.push_str("; RTO is plcc's retentive timer FB (not IEC standard), reset by RES through its R input");
                }
                if !acc.is_empty() && acc.trim() != "0" {
                    msg.push_str(&format!("; the initial Accum {acc} is not carried over"));
                }
                self.warn(&mut x, &what, msg);
                out.push(x);
            }
            "CTU" | "CTD" => {
                let Some(tag) = op(self, 0) else {
                    return not_translated(self, out, "no counter operand".into());
                };
                let pre = ops
                    .get(1)
                    .cloned()
                    .flatten()
                    .or_else(|| {
                        self.tag_data
                            .get(&tag.to_ascii_lowercase())
                            .and_then(|d| preset_of(d))
                    })
                    .unwrap_or_else(|| "0".into());
                let pv = self.op_to_iec(&pre, b.id).unwrap_or_else(|| pre.clone());
                let (edge, reset) = if up == "CTU" {
                    ("CU", "R")
                } else {
                    ("CD", "LD")
                };
                if self.fbs.get(&tag.to_ascii_lowercase()) == Some(&Fb::Ctud) {
                    // CTU and CTD on one COUNTER: one CTUD instance; each call
                    // names only its own count input (the other keeps its value).
                    let pins = vec![
                        pin_in(edge, None),
                        pin_in("PV", Some(pv)),
                        pin_out("QU", None),
                        pin_out("QD", None),
                        pin_out("CV", None),
                    ];
                    let mut x = fb("CTUD", tag.clone(), pins, edge, None);
                    self.warn(&mut x, &what, format!("the COUNTER tag {tag}, counted up and down, becomes an IEC CTUD instance: .ACC is CV, .PRE is PV, .DN is QU (CV >= PV); IEC CV is an INT (−32,768..32,767) where .ACC is a DINT that wraps and sets .OV/.UN (not translated)"));
                    out.push(x);
                    return;
                }
                let pins = vec![
                    pin_in(edge, None),
                    pin_in(reset, None),
                    pin_in("PV", Some(pv)),
                    pin_out("Q", None),
                    pin_out("CV", None),
                ];
                let mut x = fb(&up, tag.clone(), pins, edge, None);
                let msg = if up == "CTU" {
                    format!(
                        "the COUNTER tag {tag} becomes an IEC CTU instance: .DN is Q, .ACC is CV, .PRE is PV; IEC CV is an INT that stops at 32,767 where .ACC wraps past 2,147,483,647 and sets .OV (not translated); RES is a call with R := TRUE"
                    )
                } else {
                    format!(
                        "the COUNTER tag {tag} becomes an IEC CTD instance: .ACC is CV, .PRE is PV, .DN is (CV >= PV); IEC CTD counts down from PV after LD, Logix CTD from .ACC; underflow (.UN) is not translated"
                    )
                };
                self.warn(&mut x, &what, msg);
                out.push(x);
            }
            "RES" => {
                let Some(tag) = op(self, 0) else {
                    return not_translated(self, out, "no operand".into());
                };
                let kind = self.fbs.get(&tag.to_ascii_lowercase()).copied();
                let code = match kind {
                    Some(Fb::Ton) => format!("{tag}(IN := FALSE);"),
                    Some(Fb::Tof) => {
                        self.warn_at(b.id, &what, format!("RES of the TOF {tag}: an IEC TOF has no reset; IN := FALSE starts its off-delay instead of clearing it"));
                        format!("{tag}(IN := FALSE);")
                    }
                    // The RTO's own call passes R := FALSE every scan; a
                    // second call here would restart its timing early.
                    Some(Fb::Rto) => format!("{tag}(R := TRUE);"),
                    Some(Fb::Ctu) | Some(Fb::Ctud) => {
                        format!("{tag}(R := TRUE);\n{tag}(R := FALSE);")
                    }
                    Some(Fb::Ctd) => {
                        self.warn_at(
                            b.id,
                            &what,
                            format!("RES of the CTD {tag}: an IEC CTD loads PV rather than 0 (LD)"),
                        );
                        format!("{tag}(LD := TRUE);\n{tag}(LD := FALSE);")
                    }
                    _ => {
                        return not_translated(
                            self,
                            out,
                            format!(
                                "RES of {tag}, which is not a timer or counter used by TON/TOF/RTO/CTU/CTD"
                            ),
                        );
                    }
                };
                let mut x = st_box(b.id, code, Vec::new());
                self.warn(&mut x, &what, format!("RES({tag}) becomes an ST box calling the instance with its reset input (inputs not named keep their value, so no count or timing starts)"));
                out.push(x);
            }
            "ONS" | "OSR" | "OSF" => {
                // The storage bit stays a tag, written as Logix writes it:
                // tmp := rung; out := rung AND NOT sb (OSF: sb AND NOT rung);
                // sb := tmp in a rung after this one. The storage bit starts
                // TRUE for ONS/OSR, as the Logix prescan sets it.
                let Some(sb) = op(self, 0) else {
                    return not_translated(self, out, "no storage bit".into());
                };
                let tmp = format!("os{}_rung", b.id);
                self.extra_vars.push(Variable {
                    name: tmp.clone(),
                    data_type: "BOOL".into(),
                    ..Default::default()
                });
                if up != "OSF" {
                    self.init_true.push(sb.clone());
                }
                let mut x = coil(b.id, tmp.clone(), CoilKind::Normal);
                _around.after.push(vec![
                    contact(self.id(), tmp.clone(), ContactKind::No),
                    coil(self.id(), sb.clone(), CoilKind::Normal),
                ]);
                match up.as_str() {
                    "ONS" => {
                        self.warn(&mut x, &what, format!("ONS({sb}): the rung is latched in {tmp}, continues as rung AND NOT {sb}, and {sb} := {tmp} in a rung after this one ({sb} is updated after the rung, not at this point; it starts TRUE as after the Logix prescan)"));
                        out.push(x);
                        out.push(contact(self.id(), sb, ContactKind::Nc));
                    }
                    "OSR" => {
                        let Some(q) = op(self, 1) else {
                            return not_translated(self, out, "no output bit".into());
                        };
                        self.warn(&mut x, &what, format!("OSR({sb},{q}): {q} := rung AND NOT {sb} on a leg of its own, {sb} := rung in a rung after this one ({sb} starts TRUE as after the Logix prescan)"));
                        out.push(x);
                        let id = self.id();
                        let leg = vec![
                            contact(self.id(), sb, ContactKind::Nc),
                            coil(self.id(), q, CoilKind::Normal),
                        ];
                        out.push(Element::Branch(Branch {
                            id,
                            legs: vec![leg, Vec::new()],
                            src: None,
                        }));
                    }
                    _ => {
                        let Some(q) = op(self, 1) else {
                            return not_translated(self, out, "no output bit".into());
                        };
                        self.warn(&mut x, &what, format!("OSF({sb},{q}): {q} := {sb} AND NOT rung and {sb} := rung in rungs after this one ({q} is written after the rung, not at this point)"));
                        out.push(x);
                        let last = _around.after.pop();
                        _around.after.push(vec![
                            contact(self.id(), sb.clone(), ContactKind::No),
                            contact(self.id(), tmp.clone(), ContactKind::Nc),
                            coil(self.id(), q, CoilKind::Normal),
                        ]);
                        _around.after.extend(last);
                    }
                }
            }
            "EQU" | "EQ" | "NEQ" | "NE" | "LES" | "LT" | "LEQ" | "LE" | "GRT" | "GT" | "GEQ"
            | "GE" => {
                let name = match up.as_str() {
                    "EQU" | "EQ" => "EQ",
                    "NEQ" | "NE" => "NE",
                    "LES" | "LT" => "LT",
                    "LEQ" | "LE" => "LE",
                    "GRT" | "GT" => "GT",
                    _ => "GE",
                };
                let (Some(a), Some(c)) = (op(self, 0), op(self, 1)) else {
                    return not_translated(self, out, "operands missing".into());
                };
                let x = Element::Block(Block {
                    id: b.id,
                    name: name.into(),
                    pins: vec![
                        pin_in("IN1", Some(a)),
                        pin_in("IN2", Some(c)),
                        pin_out("OUT", None),
                    ],
                    power_in: None,
                    power_out: Some("OUT".into()),
                    ..Default::default()
                });
                out.push(x);
            }
            "LIM" | "LIMIT" => {
                let (Some(lo), Some(t), Some(hi)) = (op(self, 0), op(self, 1), op(self, 2)) else {
                    return not_translated(self, out, "operands missing".into());
                };
                out.push(contact(
                    b.id,
                    format!("(({lo} <= {hi}) AND ({lo} <= {t}) AND ({t} <= {hi})) OR (({lo} > {hi}) AND (({t} >= {lo}) OR ({t} <= {hi})))"),
                    ContactKind::No,
                ));
            }
            "MEQ" => {
                let (Some(s), Some(m), Some(c)) = (op(self, 0), op(self, 1), op(self, 2)) else {
                    return not_translated(self, out, "operands missing".into());
                };
                let mut x = contact(
                    b.id,
                    format!("(({s} AND {m}) = ({c} AND {m}))"),
                    ContactKind::No,
                );
                self.warn(
                    &mut x,
                    &what,
                    "Logix zero-fills SINT/INT operands of MEQ; IEC ANDs them in their own types"
                        .into(),
                );
                out.push(x);
            }
            "CMP" => {
                let Some(ex) = op(self, 0) else {
                    return not_translated(self, out, "no expression".into());
                };
                let mut x = contact(b.id, format!("({ex})"), ContactKind::No);
                if precedence_differs(&ex) {
                    self.warn(&mut x, &what, "the expression mixes comparisons and AND/OR/XOR: in Logix CMP/CPT AND/OR/XOR bind tighter than comparisons, in IEC ST looser; parenthesize it".into());
                }
                out.push(x);
            }
            "CPT" => {
                let (Some(dest), Some(ex)) = (op(self, 0), op(self, 1)) else {
                    return not_translated(self, out, "operands missing".into());
                };
                let mut x = st_box(b.id, format!("{dest} := {ex};"), Vec::new());
                let mut msg = "CPT becomes an ST assignment run when the rung is true; Logix stores into the destination's type with its rounding and status flags".to_string();
                if precedence_differs(&ex) {
                    msg.push_str("; the expression mixes comparisons and AND/OR/XOR, which bind differently in Logix (tighter) and IEC ST (looser): parenthesize it");
                }
                self.warn(&mut x, &what, msg);
                out.push(x);
            }
            "ADD" | "SUB" | "MUL" | "DIV" | "MOD" | "XPY" | "AND" | "OR" | "XOR" | "BAND"
            | "BOR" | "BXOR" => {
                let name = match up.as_str() {
                    "XPY" => "EXPT",
                    "BAND" => "AND",
                    "BOR" => "OR",
                    "BXOR" => "XOR",
                    x => x,
                };
                let (Some(a), Some(c), Some(d)) = (op(self, 0), op(self, 1), op(self, 2)) else {
                    return not_translated(self, out, "operands missing".into());
                };
                let mut x = func(name, vec![("IN1", a), ("IN2", c)], Some(d));
                self.warn(&mut x, &what, "IEC evaluates in the operands' types: no widening to the destination's type, no S:V/S:Z/S:N, no round-half-even store, and integer division by zero is a fault instead of Source A".into());
                out.push(x);
            }
            "MOV" | "MOVE" => {
                let (Some(s), Some(d)) = (op(self, 0), op(self, 1)) else {
                    return not_translated(self, out, "operands missing".into());
                };
                out.push(func("MOVE", vec![("IN", s)], Some(d)));
            }
            "CLR" => {
                let Some(d) = op(self, 0) else {
                    return not_translated(self, out, "no destination".into());
                };
                out.push(func("MOVE", vec![("IN", "0".into())], Some(d)));
            }
            "NOT" | "ABS" | "SQR" | "SQRT" | "SIN" | "COS" | "TAN" | "ASN" | "ASIN" | "ACS"
            | "ACOS" | "ATN" | "ATAN" | "LN" | "LOG" | "TRN" | "TRUNC" => {
                let name = match up.as_str() {
                    "SQR" => "SQRT",
                    "ASN" => "ASIN",
                    "ACS" => "ACOS",
                    "ATN" => "ATAN",
                    "TRN" => "TRUNC",
                    x => x,
                };
                let (Some(s), Some(d)) = (op(self, 0), op(self, 1)) else {
                    return not_translated(self, out, "operands missing".into());
                };
                let mut x = func(name, vec![("IN", s)], Some(d));
                if name != "NOT" {
                    self.warn(&mut x, &what, "Logix evaluates in REAL and rounds into the destination's type; IEC keeps the operand's type".into());
                }
                out.push(x);
            }
            "NEG" => {
                let (Some(s), Some(d)) = (op(self, 0), op(self, 1)) else {
                    return not_translated(self, out, "operands missing".into());
                };
                out.push(st_box(b.id, format!("{d} := -{s};"), Vec::new()));
            }
            "AFI" => out.push(contact(b.id, "FALSE", ContactKind::No)),
            "NOP" => {
                self.warn_at(b.id, &what, "NOP does nothing and is left out".into());
            }
            "TND" => {
                let mut x = Element::Return(Return {
                    id: b.id,
                    src: None,
                });
                self.warn(
                    &mut x,
                    &what,
                    "TND (end of the task's scan) becomes RETURN, which ends only this POU".into(),
                );
                out.push(x);
            }
            "JSR"
                if b.pins.len() <= 2
                    || ops
                        .get(1)
                        .cloned()
                        .flatten()
                        .is_some_and(|n| n.trim() == "0")
                        && b.pins.len() == 2 =>
            {
                let Some(r) = ops.first().cloned().flatten() else {
                    return not_translated(self, out, "no routine".into());
                };
                let mut x = Element::Block(Block {
                    id: b.id,
                    name: r.clone(),
                    pins: vec![pin_in("EN", None), pin_out("ENO", None)],
                    power_in: Some("EN".into()),
                    power_out: Some("ENO".into()),
                    ..Default::default()
                });
                if !self.routines.contains(&r.to_ascii_lowercase()) {
                    self.warn(
                        &mut x,
                        &what,
                        format!("JSR to {r}, which is not a routine of this program"),
                    );
                }
                out.push(x);
            }
            _ => not_translated(
                self,
                out,
                format!("{} has no IEC ladder counterpart", b.name),
            ),
        }
    }
}

/// The `.PRE` in a TIMER / COUNTER tag's initial data: decorated
/// `(PRE := 50, ACC := 0)` or L5K `[control, PRE, ACC]`.
fn preset_of(data: &str) -> Option<String> {
    let d = data.trim();
    if let Some(inner) = d.strip_prefix('(').and_then(|x| x.strip_suffix(')')) {
        for m in inner.split(',') {
            if let Some((n, v)) = m.split_once(":=")
                && n.trim().eq_ignore_ascii_case("PRE")
            {
                return Some(v.trim().to_string());
            }
        }
        return None;
    }
    let inner = d.strip_prefix('[')?.strip_suffix(']')?;
    inner.split(',').nth(1).map(|x| x.trim().to_string())
}

/// Whether `e` reads the identifier `name`.
fn mentions(e: &Expression, name: &str, found: &mut bool) {
    match &e.kind {
        ExpressionKind::Identifier(i) if i.name.eq_ignore_ascii_case(name) => *found = true,
        ExpressionKind::BinaryOp { left, right, .. } => {
            mentions(left, name, found);
            mentions(right, name, found);
        }
        ExpressionKind::UnaryOp { operand, .. } | ExpressionKind::Parenthesized(operand) => {
            mentions(operand, name, found)
        }
        ExpressionKind::MemberAccess { object, .. } => mentions(object, name, found),
        ExpressionKind::ArrayIndex { array, indices } => {
            mentions(array, name, found);
            for i in indices {
                mentions(i, name, found);
            }
        }
        ExpressionKind::FunctionCall { args, .. } => {
            for a in args {
                mentions(&a.value, name, found);
            }
        }
        _ => {}
    }
}

/// Whether a Logix expression mixes comparisons with AND/OR/XOR (which bind
/// differently in Logix expressions and IEC ST).
fn precedence_differs(e: &str) -> bool {
    let up = e.to_ascii_uppercase();
    let logical = [" AND ", " OR ", " XOR "].iter().any(|w| up.contains(w));
    let cmp = ["<", ">", "="].iter().any(|c| up.contains(c));
    logical && cmp
}

/// An IEC type as a Logix data type; `Ok(None)` for an edge detector (its
/// state becomes a BOOL storage tag); `Err` for what has no counterpart.
fn type_to_logix(ty: &str) -> Result<Option<String>, String> {
    let (u, errs) = plcc_st::parse(&format!("TYPE x : {ty}; END_TYPE"));
    let Some(plcc_st::Declaration::TypeDecl(t)) = u.declarations.into_iter().next() else {
        return Err(format!("type {ty} is not understood"));
    };
    if !errs.is_empty() {
        return Err(format!("type {ty} is not understood"));
    }
    spec_to_logix(&t.type_spec).map_err(|m| format!("{m} (type {ty})"))
}

fn spec_to_logix(t: &plcc_st::TypeSpec) -> Result<Option<String>, String> {
    use plcc_st::TypeSpecKind as K;
    Ok(Some(match &t.kind {
        K::Named(i) => match i.name.to_ascii_uppercase().as_str() {
            n @ ("BOOL" | "SINT" | "INT" | "DINT" | "LINT" | "USINT" | "UINT" | "UDINT"
            | "ULINT" | "REAL" | "LREAL") => n.to_string(),
            "BYTE" => "SINT".into(),
            "WORD" => "INT".into(),
            "DWORD" => "DINT".into(),
            "LWORD" => "LINT".into(),
            "TIME" | "LTIME" => "DINT".into(),
            "TON" | "TOF" | "RTO" => "TIMER".into(),
            "CTU" | "CTD" => "COUNTER".into(),
            "R_TRIG" | "F_TRIG" => return Ok(None),
            other => return Err(format!("{other} has no Logix counterpart in ladder")),
        },
        K::StringType { .. } => "STRING".into(),
        K::Array { ranges, base } => {
            let b = spec_to_logix(base)?.ok_or("an array of edge detectors")?;
            let mut dims = Vec::new();
            for r in ranges {
                let lo = int_of(&r.low).ok_or("array bounds must be constants")?;
                let hi = int_of(&r.high).ok_or("array bounds must be constants")?;
                if lo != 0 {
                    return Err(format!("the array starts at {lo}; Logix arrays start at 0"));
                }
                dims.push((hi - lo + 1).to_string());
            }
            format!("{b}[{}]", dims.join(","))
        }
        _ => return Err("no Logix counterpart".into()),
    }))
}

fn int_of(e: &Expression) -> Option<i128> {
    match &e.kind {
        ExpressionKind::IntegerLiteral(v) => Some(*v),
        ExpressionKind::UnaryOp {
            op: UnaryOp::Neg,
            operand,
        } => int_of(operand).map(|v| -v),
        ExpressionKind::Parenthesized(x) => int_of(x),
        ExpressionKind::BinaryOp {
            op: BinaryOp::Sub,
            left,
            right,
        } => Some(int_of(left)? - int_of(right)?),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(duration_ns("T#100ms"), Some(100_000_000));
        assert_eq!(duration_ns("TIME#1h2m3s4ms"), Some(3_723_004_000_000));
        assert_eq!(duration_ns("t#1.5s"), Some(1_500_000_000));
        assert_eq!(duration_ns("T#-5ms"), Some(-5_000_000));
        assert_eq!(duration_ns("5"), None);
    }

    #[test]
    fn types() {
        assert_eq!(
            type_to_logix("ARRAY[0..9] OF INT"),
            Ok(Some("INT[10]".into()))
        );
        assert_eq!(type_to_logix("TON"), Ok(Some("TIMER".into())));
        assert_eq!(type_to_logix("R_TRIG"), Ok(None));
        assert!(type_to_logix("ARRAY[1..9] OF INT").is_err());
    }
}
