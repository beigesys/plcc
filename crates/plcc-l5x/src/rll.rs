// SPDX-License-Identifier: MPL-2.0

//! Ladder (RLL) routines → ST.
//!
//! Rung-condition semantics (1756-RM003, every instruction's "Execution"
//! table): each rung starts with rung-condition-in TRUE (FALSE inside a disabled
//! MCR zone) and the condition flows left to right through the instructions.
//! Input instructions (XIC, XIO, compares, ONS) change it; output instructions
//! act on it and pass it on unchanged. Every instruction executes on every scan,
//! true or false: a false rung still clears an OTE, resets a TON, clears an ONS
//! storage bit. A branch `[a, b]` starts every leg with the condition at the
//! branch and ORs the legs' results where they join.
//!
//! Generated code, per rung:
//!
//! ```text
//! lx__rc := TRUE;                       (* rung-condition *)
//! lx__rc := lx__rc AND Start;           (* XIC(Start)      *)
//! lx__bs1 := lx__rc; lx__bo1 := FALSE;  (* [             *)
//! ...                                   (* leg; lx__bo1 := lx__bo1 OR lx__rc *)
//! lx__rc := lx__bo1;                    (* ]             *)
//! Motor := lx__rc;                      (* OTE(Motor)      *)
//! ```
//!
//! Instructions that act on the controller's first scan collect "prescan"
//! statements (OTE clears its bit, ONS sets its storage bit, TON clears its
//! status, ...), run once before the program's first scan.

use crate::emit::Out;
use crate::error::L5xError;
use crate::lower::AoiSig;
use crate::model::{RoutineDef, Usage};
use crate::operand::{self, LExpr, LKind, Seg, TagPath};
use crate::rung::{self, Element, Instr};
use crate::scope::{Ctx, Dom, Val, conv, dom_of};
use crate::types::{Elem, StructKind, Ty, TypeEnv};
use crate::xml::Text;
use plcc_st::Span;
use std::collections::{BTreeSet, HashMap};

pub(crate) struct Shared<'a> {
    pub src: &'a str,
    pub env: &'a TypeEnv,
    pub aois: &'a HashMap<String, AoiSig>,
}

pub(crate) struct RoutineOut {
    /// Hidden temporaries (`name : TYPE`), declared once per POU.
    pub temps: BTreeSet<String>,
    pub body: Out,
    pub prescan: Out,
}

impl RoutineOut {
    pub fn temps_out(&self) -> Out {
        let mut o = Out::new();
        for t in &self.temps {
            o.s("    ").s(t).s(";\n");
        }
        o
    }
}

/// `lx__put_<E>_i` / `_r` for a numeric destination, or a plain BOOL store.
pub(crate) fn store(env: &TypeEnv, dest: &str, dty: &Ty, v: &Val) -> Result<String, String> {
    match dty.elem() {
        Some(Elem::Bool) => Ok(format!("{dest} := {};", conv(v, Dom::Bool))),
        Some(e) if e.is_num() => {
            let (suffix, arg) = match v.dom {
                Dom::Int | Dom::Bool => ("i", conv(v, Dom::Int)),
                Dom::Real | Dom::LReal => ("r", conv(v, Dom::LReal)),
            };
            // S:V is cleared by the instruction and set by any overflow on
            // the way to the stored value (a division by zero included).
            Ok(format!(
                "lx__S_V := FALSE; {dest} := lx__put_{}_{suffix}({arg});",
                e.st()
            ))
        }
        _ => Err(format!("cannot store a number in a {}", env.logix(dty))),
    }
}

/// Convert without touching the status flags (instruction operands passed to
/// an AOI, BTD results, ...).
pub(crate) fn cast(v: &Val, e: Elem) -> String {
    match (v.dom, e) {
        (_, Elem::Bool) => conv(v, Dom::Bool),
        (Dom::Int | Dom::Bool, Elem::Lint) => conv(v, Dom::Int),
        (Dom::Int | Dom::Bool, e) if e.is_int() => {
            format!("LINT_TO_{}({})", e.st(), conv(v, Dom::Int))
        }
        (Dom::Real | Dom::LReal, e) if e.is_int() => {
            format!(
                "LINT_TO_{}(lx__r2l(lx__round({})))",
                e.st(),
                conv(v, Dom::LReal)
            )
        }
        (_, Elem::Real) => conv(v, Dom::Real),
        (_, Elem::Lreal) => conv(v, Dom::LReal),
        _ => v.st.clone(),
    }
}

/// Widest value domain of the operands (1756-RM003 "Data conversions": the
/// operands are converted to the highest-ranked type among them).
fn widest(doms: &[Dom]) -> Dom {
    if doms.contains(&Dom::LReal) {
        Dom::LReal
    } else if doms.contains(&Dom::Real) {
        Dom::Real
    } else {
        Dom::Int
    }
}

/// Zero-fill a SINT/INT/DINT operand (logical instructions: "Logical
/// instructions use zero fill. All other instructions use sign-extension",
/// 1756-RM003 "Data conversions").
fn zero_fill(v: &Val) -> String {
    let mask = match v.ty.elem() {
        Some(Elem::Sint) => "16#FF",
        Some(Elem::Int) => "16#FFFF",
        Some(Elem::Dint) => "16#FFFFFFFF",
        _ => return conv(v, Dom::Int),
    };
    if matches!(&v.st[..], s if s.starts_with('(') || s.chars().all(|c| c.is_ascii_digit()))
        && !v.st.contains("_TO_")
    {
        // A literal: already a DINT value; zero-fill does not apply.
        return conv(v, Dom::Int);
    }
    format!("({} AND {mask})", conv(v, Dom::Int))
}

struct R<'a, 'x> {
    sh: &'a Shared<'a>,
    ctx: &'a Ctx<'x>,
    routines: &'a HashMap<String, String>,
    in_aoi: bool,
    body: Out,
    pre: Out,
    errors: Vec<L5xError>,
    temps: BTreeSet<String>,
    depth: usize,
    labels: HashMap<String, u32>,
    jmp: bool,
    mcr: bool,
    guards: usize,
    /// SBR operands of each routine (for JSR input parameters).
    sbr: &'a HashMap<String, Vec<(String, Ty)>>,
}

type Res<T> = Result<T, L5xError>;

pub(crate) fn routine(
    sh: &Shared,
    ctx: &Ctx,
    r: &RoutineDef,
    routines: &HashMap<String, String>,
    in_aoi: bool,
) -> (RoutineOut, Vec<L5xError>) {
    let empty = HashMap::new();
    routine_with(sh, ctx, r, routines, in_aoi, &empty)
}

pub(crate) fn routine_with(
    sh: &Shared,
    ctx: &Ctx,
    r: &RoutineDef,
    routines: &HashMap<String, String>,
    in_aoi: bool,
    sbr: &HashMap<String, Vec<(String, Ty)>>,
) -> (RoutineOut, Vec<L5xError>) {
    let mut w = R {
        sh,
        ctx,
        routines,
        in_aoi,
        body: Out::new(),
        pre: Out::new(),
        errors: Vec::new(),
        temps: BTreeSet::new(),
        depth: 0,
        labels: HashMap::new(),
        jmp: false,
        mcr: false,
        guards: 0,
        sbr,
    };
    w.temps.insert("lx__rc : BOOL".into());
    // Parse every rung first: labels and MCR/JMP use are routine-wide.
    let mut parsed: Vec<(Span, Option<&Text>, Vec<Vec<Element>>)> = Vec::new();
    for rung in &r.rungs {
        let Some(text) = &rung.text else {
            parsed.push((rung.span, None, Vec::new()));
            continue;
        };
        match rung::parse(&text.text) {
            Ok(rs) => {
                for seq in &rs {
                    w.scan_meta(seq, text);
                }
                parsed.push((rung.span, Some(text), rs));
            }
            Err(e) => {
                w.errors.push(L5xError::new(
                    format!("rung {}: {}", rung.number.unwrap_or(0), e.message),
                    text.span(e.span),
                ));
                parsed.push((rung.span, Some(text), Vec::new()));
            }
        }
    }
    let _ = sh.src;
    w.body.push_ctx(r.span);
    if w.mcr {
        w.temps.insert("lx__mcr : BOOL".into());
        w.body.s("lx__mcr := FALSE;\n");
    }
    if w.jmp {
        w.temps.insert("lx__jmp : DINT".into());
        w.body.s("lx__jmp := 0;\nREPEAT\n");
    }
    for (span, text, rs) in &parsed {
        let Some(text) = text else { continue };
        for seq in rs {
            w.body.push_ctx(text.whole());
            if w.jmp {
                if let Some(Element::Instr(first)) = seq.first()
                    && first.name.eq_ignore_ascii_case("LBL")
                    && let Some(op) = first.operands.first()
                {
                    let name = text.text[op.clone()].trim().to_ascii_lowercase();
                    if let Some(id) = w.labels.get(&name) {
                        w.body
                            .s(&format!("IF lx__jmp = {id} THEN lx__jmp := 0; END_IF;\n"));
                    }
                }
                w.body.s("IF lx__jmp = 0 THEN\n");
            }
            let has_mcr = seq_has(seq, "MCR");
            if w.mcr {
                w.body.s("lx__rc := NOT lx__mcr;\n");
            } else {
                w.body.s("lx__rc := TRUE;\n");
            }
            let _ = has_mcr;
            w.seq(seq, text);
            for _ in 0..w.guards {
                w.body.s("END_IF;\n");
            }
            w.guards = 0;
            if w.jmp {
                w.body.s("END_IF;\n");
            }
            w.body.pop_ctx();
        }
        let _ = span;
    }
    if w.jmp {
        w.body.s("UNTIL lx__jmp = 0 END_REPEAT;\n");
    }
    w.body.pop_ctx();
    let out = RoutineOut {
        temps: w.temps,
        body: w.body,
        prescan: w.pre,
    };
    (out, w.errors)
}

/// The SBR operands of each ladder routine whose first instruction is SBR: the
/// tags a JSR's input parameters are copied into (1756-RM003 JSR/SBR/RET).
pub(crate) fn sbr_params(ctx: &Ctx, routines: &[RoutineDef]) -> HashMap<String, Vec<(String, Ty)>> {
    let mut map = HashMap::new();
    for r in routines {
        if r.kind == crate::model::RoutineKind::St {
            crate::stx::sbr_params(ctx, r, &mut map);
            continue;
        }
        let Some(text) = r.rungs.first().and_then(|g| g.text.as_ref()) else {
            continue;
        };
        let Ok(rs) = rung::parse(&text.text) else {
            continue;
        };
        let Some(Element::Instr(first)) = rs.first().and_then(|s| s.first()) else {
            continue;
        };
        if !first.name.eq_ignore_ascii_case("SBR") {
            continue;
        }
        let mut params = Vec::new();
        for op in &first.operands {
            let Ok(e) = operand::parse_expr(&text.text, op.clone()) else {
                break;
            };
            let Ok(d) = ctx.dest(&e, text) else { break };
            params.push(d);
        }
        map.insert(r.name.text.to_ascii_lowercase(), params);
    }
    map
}

fn seq_has(seq: &[Element], name: &str) -> bool {
    seq.iter().any(|e| match e {
        Element::Instr(i) => i.name.eq_ignore_ascii_case(name),
        Element::Branch(legs, _) => legs.iter().any(|l| seq_has(l, name)),
    })
}

impl<'a, 'x> R<'a, 'x> {
    fn scan_meta(&mut self, seq: &[Element], text: &Text) {
        for e in seq {
            match e {
                Element::Instr(i) => match i.name.to_ascii_uppercase().as_str() {
                    "LBL" => {
                        if let Some(op) = i.operands.first() {
                            let name = text.text[op.clone()].trim().to_ascii_lowercase();
                            let id = self.labels.len() as u32 + 1;
                            if self.labels.insert(name, id).is_some() {
                                self.errors.push(L5xError::new(
                                    "label is defined twice in this routine",
                                    text.span(op.clone()),
                                ));
                            }
                        }
                    }
                    "JMP" => self.jmp = true,
                    "MCR" => self.mcr = true,
                    _ => {}
                },
                Element::Branch(legs, _) => {
                    for l in legs {
                        self.scan_meta(l, text);
                    }
                }
            }
        }
    }

    fn seq(&mut self, seq: &[Element], text: &Text) {
        for e in seq {
            match e {
                Element::Instr(i) => {
                    let span = text.span(i.span.clone());
                    self.body.push_ctx(span);
                    self.pre.push_ctx(span);
                    if let Err(err) = self.instr(i, text) {
                        self.errors.push(err);
                    }
                    self.pre.pop_ctx();
                    self.body.pop_ctx();
                }
                Element::Branch(legs, r) => {
                    self.depth += 1;
                    let d = self.depth;
                    self.temps.insert(format!("lx__bs{d} : BOOL"));
                    self.temps.insert(format!("lx__bo{d} : BOOL"));
                    self.body.push_ctx(text.span(r.clone()));
                    self.body
                        .s(&format!("lx__bs{d} := lx__rc;\nlx__bo{d} := FALSE;\n"));
                    for (k, leg) in legs.iter().enumerate() {
                        if k > 0 {
                            self.body.s(&format!("lx__rc := lx__bs{d};\n"));
                        }
                        self.seq(leg, text);
                        self.body.s(&format!("lx__bo{d} := lx__bo{d} OR lx__rc;\n"));
                    }
                    self.body.s(&format!("lx__rc := lx__bo{d};\n"));
                    self.body.pop_ctx();
                    self.depth -= 1;
                }
            }
        }
    }

    fn line(&mut self, s: &str) {
        self.body.s(s).s("\n");
    }

    fn pre_line(&mut self, s: &str) {
        self.pre.s(s).s("\n");
    }

    fn operand(&self, ins: &Instr, i: usize, text: &Text) -> Res<LExpr> {
        let Some(r) = ins.operands.get(i) else {
            return Err(L5xError::new(
                format!("{} is missing operand {}", ins.name, i + 1),
                text.span(ins.span.clone()),
            ));
        };
        operand::parse_expr(&text.text, r.clone())
            .map_err(|e| L5xError::new(e.message, text.span(e.span)))
    }

    fn arity(&self, ins: &Instr, n: usize, text: &Text) -> Res<()> {
        if ins.operands.len() < n {
            return Err(L5xError::new(
                format!(
                    "{} expects {n} operand(s), found {}",
                    ins.name,
                    ins.operands.len()
                ),
                text.span(ins.span.clone()),
            ));
        }
        Ok(())
    }

    fn val(&self, ins: &Instr, i: usize, text: &Text) -> Res<Val> {
        let e = self.operand(ins, i, text)?;
        self.ctx.value(&e, text)
    }

    fn bit(&self, ins: &Instr, i: usize, text: &Text) -> Res<String> {
        let v = self.val(ins, i, text)?;
        if v.dom != Dom::Bool {
            return Err(L5xError::new(
                format!("{} needs a BOOL operand", ins.name),
                text.span(ins.operands[i].clone()),
            ));
        }
        Ok(v.st)
    }

    fn dest(&self, ins: &Instr, i: usize, text: &Text) -> Res<(String, Ty)> {
        let e = self.operand(ins, i, text)?;
        self.ctx.dest(&e, text)
    }

    fn bit_dest(&self, ins: &Instr, i: usize, text: &Text) -> Res<String> {
        let (st, ty) = self.dest(ins, i, text)?;
        if !ty.is_bool() {
            return Err(L5xError::new(
                format!(
                    "{} needs a BOOL operand, found a {}",
                    ins.name,
                    self.sh.env.logix(&ty)
                ),
                text.span(ins.operands[i].clone()),
            ));
        }
        Ok(st)
    }

    /// A structure operand of the given predefined type (TIMER, COUNTER, ...).
    fn structure(
        &self,
        ins: &Instr,
        i: usize,
        text: &Text,
        want: &[&str],
    ) -> Res<(String, String)> {
        let (st, ty) = self.dest(ins, i, text)?;
        if let Ty::Struct(id) = &ty {
            let def = self.sh.env.get(*id);
            if def.kind == StructKind::Builtin
                && let Some(w) = want.iter().find(|w| def.logix.eq_ignore_ascii_case(w))
            {
                return Ok((st, w.to_string()));
            }
        }
        Err(L5xError::new(
            format!(
                "{} needs a {} operand, found a {}",
                ins.name,
                want.join(" or "),
                self.sh.env.logix(&ty)
            ),
            text.span(ins.operands[i].clone()),
        ))
    }

    fn store_to(&self, ins: &Instr, i: usize, text: &Text, v: &Val) -> Res<String> {
        let (d, dty) = self.dest(ins, i, text)?;
        store(self.sh.env, &d, &dty, v)
            .map_err(|m| L5xError::new(m, text.span(ins.operands[i].clone())))
    }

    fn dest_dom(&self, ins: &Instr, i: usize, text: &Text) -> Res<Dom> {
        let (_, dty) = self.dest(ins, i, text)?;
        Ok(dom_of(&dty).unwrap_or(Dom::Int))
    }

    /// `IF lx__rc THEN <stmt> END_IF;`
    fn when_true(&mut self, stmt: &str) {
        self.line(&format!("IF lx__rc THEN\n    {stmt}\nEND_IF;"));
    }

    fn pseudo(&mut self, ins: &Instr, text: &Text, tag: &str, idx: usize, member: &str) {
        if let Ok(e) = self.operand(ins, idx, text)
            && let LKind::Int(n) = e.kind
        {
            self.pre_line(&format!("{tag}.{member} := {n};"));
        }
    }

    fn instr(&mut self, ins: &Instr, text: &Text) -> Res<()> {
        let up = ins.name.to_ascii_uppercase();
        let t = text;
        match up.as_str() {
            // ── Bit (1756-RM003 "Bit Instructions") ──
            "XIC" => {
                self.arity(ins, 1, t)?;
                let b = self.bit(ins, 0, t)?;
                self.line(&format!("lx__rc := lx__rc AND {b};"));
            }
            "XIO" => {
                self.arity(ins, 1, t)?;
                let b = self.bit(ins, 0, t)?;
                self.line(&format!("lx__rc := lx__rc AND NOT {b};"));
            }
            "OTE" => {
                self.arity(ins, 1, t)?;
                let b = self.bit_dest(ins, 0, t)?;
                self.line(&format!("{b} := lx__rc;"));
                self.pre_line(&format!("{b} := FALSE;"));
            }
            "OTL" => {
                self.arity(ins, 1, t)?;
                let b = self.bit_dest(ins, 0, t)?;
                self.when_true(&format!("{b} := TRUE;"));
            }
            "OTU" => {
                self.arity(ins, 1, t)?;
                let b = self.bit_dest(ins, 0, t)?;
                self.when_true(&format!("{b} := FALSE;"));
            }
            "ONS" => {
                // True: rung-out true only when the storage bit was clear;
                // storage := 1. False: storage := 0. Prescan: storage := 1.
                self.arity(ins, 1, t)?;
                let sb = self.bit_dest(ins, 0, t)?;
                self.line(&format!(
                    "IF lx__rc THEN\n    lx__rc := NOT {sb};\n    {sb} := TRUE;\nELSE\n    {sb} := FALSE;\nEND_IF;"
                ));
                self.pre_line(&format!("{sb} := TRUE;"));
            }
            "OSR" => {
                self.arity(ins, 2, t)?;
                let sb = self.bit_dest(ins, 0, t)?;
                let ob = self.bit_dest(ins, 1, t)?;
                self.line(&format!(
                    "IF lx__rc THEN\n    {ob} := NOT {sb};\n    {sb} := TRUE;\nELSE\n    {sb} := FALSE;\n    {ob} := FALSE;\nEND_IF;"
                ));
                self.pre_line(&format!("{sb} := TRUE;\n{ob} := FALSE;"));
            }
            "OSF" => {
                self.arity(ins, 2, t)?;
                let sb = self.bit_dest(ins, 0, t)?;
                let ob = self.bit_dest(ins, 1, t)?;
                self.line(&format!(
                    "IF lx__rc THEN\n    {sb} := TRUE;\n    {ob} := FALSE;\nELSE\n    {ob} := {sb};\n    {sb} := FALSE;\nEND_IF;"
                ));
                self.pre_line(&format!("{sb} := FALSE;\n{ob} := FALSE;"));
            }
            // ── Timers and counters ──
            "TON" | "TOF" | "RTO" => {
                self.arity(ins, 1, t)?;
                let (tm, _) = self.structure(ins, 0, t, &["TIMER"])?;
                self.pseudo(ins, t, &tm, 1, "PRE");
                self.pseudo(ins, t, &tm, 2, "ACC");
                let f = up.to_ascii_lowercase();
                self.line(&format!("lx__{f}({tm}, lx__rc);"));
                match up.as_str() {
                    "TON" => self.pre_line(&format!(
                        "{tm}.EN := FALSE; {tm}.TT := FALSE; {tm}.DN := FALSE; {tm}.ACC := 0;"
                    )),
                    "TOF" => self.pre_line(&format!(
                        "{tm}.EN := FALSE; {tm}.TT := FALSE; {tm}.DN := FALSE; {tm}.ACC := {tm}.PRE;"
                    )),
                    _ => self.pre_line(&format!("{tm}.EN := FALSE; {tm}.TT := FALSE;")),
                }
            }
            "CTU" | "CTD" => {
                self.arity(ins, 1, t)?;
                let (c, _) = self.structure(ins, 0, t, &["COUNTER"])?;
                self.pseudo(ins, t, &c, 1, "PRE");
                self.pseudo(ins, t, &c, 2, "ACC");
                let f = up.to_ascii_lowercase();
                self.line(&format!("lx__{f}({c}, lx__rc);"));
                let bit = if up == "CTU" { "CU" } else { "CD" };
                self.pre_line(&format!("{c}.{bit} := TRUE;"));
            }
            "RES" => {
                self.arity(ins, 1, t)?;
                let (s, kind) = self.structure(ins, 0, t, &["TIMER", "COUNTER", "CONTROL"])?;
                self.when_true(&format!("lx__res_{}({s});", kind.to_ascii_lowercase()));
            }
            // ── Compare (1756-RM003 "Compare Instructions") ──
            "EQU" | "EQ" | "NEQ" | "NE" | "LES" | "LT" | "LEQ" | "LE" | "GRT" | "GT" | "GEQ"
            | "GE" => {
                self.arity(ins, 2, t)?;
                let a = self.val(ins, 0, t)?;
                let b = self.val(ins, 1, t)?;
                let op = match up.as_str() {
                    "EQU" | "EQ" => operand::BinOp::Eq,
                    "NEQ" | "NE" => operand::BinOp::Ne,
                    "LES" | "LT" => operand::BinOp::Lt,
                    "LEQ" | "LE" => operand::BinOp::Le,
                    "GRT" | "GT" => operand::BinOp::Gt,
                    _ => operand::BinOp::Ge,
                };
                let c = self.ctx.binary(op, &a, &b);
                self.line(&format!("lx__rc := lx__rc AND {};", c.st));
            }
            "LIM" | "LIMIT" => {
                // Low <= High: Low <= Test <= High. Low > High: the circular
                // range, Test >= Low OR Test <= High.
                self.arity(ins, 3, t)?;
                let lo = self.val(ins, 0, t)?;
                let te = self.val(ins, 1, t)?;
                let hi = self.val(ins, 2, t)?;
                let d = widest(&[lo.dom, te.dom, hi.dom]);
                let (lo, te, hi) = (conv(&lo, d), conv(&te, d), conv(&hi, d));
                self.line(&format!(
                    "lx__rc := lx__rc AND (({lo} <= {hi} AND {lo} <= {te} AND {te} <= {hi}) OR ({lo} > {hi} AND ({te} >= {lo} OR {te} <= {hi})));"
                ));
            }
            "MEQ" => {
                self.arity(ins, 3, t)?;
                let s = self.val(ins, 0, t)?;
                let m = self.val(ins, 1, t)?;
                let c = self.val(ins, 2, t)?;
                let (s, m, c) = (zero_fill(&s), zero_fill(&m), zero_fill(&c));
                self.line(&format!(
                    "lx__rc := lx__rc AND (({s} AND {m}) = ({c} AND {m}));"
                ));
            }
            "CMP" => {
                self.arity(ins, 1, t)?;
                let v = self.val(ins, 0, t)?;
                self.line(&format!("lx__rc := lx__rc AND {};", conv(&v, Dom::Bool)));
            }
            // ── Math (1756-RM003 "Compute/Math Instructions") ──
            "ADD" | "SUB" | "MUL" | "DIV" | "MOD" | "XPY" => {
                self.arity(ins, 3, t)?;
                let a = self.val(ins, 0, t)?;
                let b = self.val(ins, 1, t)?;
                let dd = self.dest_dom(ins, 2, t)?;
                let d = widest(&[a.dom, b.dom, dd]);
                let a = Val {
                    st: conv(&a, d),
                    dom: d,
                    ty: a.ty,
                };
                let b = Val {
                    st: conv(&b, d),
                    dom: d,
                    ty: b.ty,
                };
                let op = match up.as_str() {
                    "ADD" => operand::BinOp::Add,
                    "SUB" => operand::BinOp::Sub,
                    "MUL" => operand::BinOp::Mul,
                    "DIV" => operand::BinOp::Div,
                    "MOD" => operand::BinOp::Mod,
                    _ => operand::BinOp::Pow,
                };
                let v = self.ctx.binary(op, &a, &b);
                let s = self.store_to(ins, 2, t, &v)?;
                self.when_true(&s);
            }
            "CPT" => {
                self.arity(ins, 2, t)?;
                let v = self.val(ins, 1, t)?;
                let s = self.store_to(ins, 0, t, &v)?;
                self.when_true(&s);
            }
            "NEG" | "ABS" | "SQR" | "SQRT" | "SIN" | "COS" | "TAN" | "ASN" | "ASIN" | "ACS"
            | "ACOS" | "ATN" | "ATAN" | "LN" | "LOG" | "DEG" | "RAD" | "TRN" | "TRUNC" | "TOD"
            | "FRD" => {
                self.arity(ins, 2, t)?;
                let src = self.operand(ins, 0, t)?;
                let e = if up == "NEG" {
                    LExpr {
                        span: src.span.clone(),
                        kind: LKind::Unary(operand::UnOp::Neg, Box::new(src)),
                    }
                } else {
                    LExpr {
                        span: src.span.clone(),
                        kind: LKind::Call(up.clone(), vec![src]),
                    }
                };
                let v = self.ctx.value(&e, t)?;
                let s = self.store_to(ins, 1, t, &v)?;
                self.when_true(&s);
            }
            // ── Move/logical (1756-RM003 "Move/Logical Instructions") ──
            "MOV" | "MOVE" => {
                self.arity(ins, 2, t)?;
                let (d, dty) = self.dest(ins, 1, t)?;
                if let Ty::Struct(_) | Ty::Array(..) = dty {
                    let se = self.operand(ins, 0, t)?;
                    let (s, sty) = match &se.kind {
                        LKind::Path(p) => self.ctx.path(p, t)?,
                        _ => {
                            return Err(L5xError::new(
                                "MOV of a structure needs a tag source",
                                t.span(se.span),
                            ));
                        }
                    };
                    if sty != dty {
                        return Err(L5xError::new(
                            format!(
                                "MOV from {} to {}",
                                self.sh.env.logix(&sty),
                                self.sh.env.logix(&dty)
                            ),
                            t.span(ins.span.clone()),
                        ));
                    }
                    self.when_true(&format!("{d} := {s};"));
                } else {
                    let v = self.val(ins, 0, t)?;
                    let s = store(self.sh.env, &d, &dty, &v)
                        .map_err(|m| L5xError::new(m, t.span(ins.operands[1].clone())))?;
                    self.when_true(&s);
                }
            }
            "MVM" => {
                self.arity(ins, 3, t)?;
                let s = self.val(ins, 0, t)?;
                let m = self.val(ins, 1, t)?;
                let (d, dty) = self.dest(ins, 2, t)?;
                let dv = self.ctx.value(&self.operand(ins, 2, t)?, t)?;
                let expr = format!(
                    "(({} AND {}) OR ({} AND NOT {}))",
                    zero_fill(&s),
                    zero_fill(&m),
                    conv(&dv, Dom::Int),
                    zero_fill(&m)
                );
                let v = Val {
                    st: expr,
                    dom: Dom::Int,
                    ty: Ty::Elem(Elem::Lint),
                };
                let st = store(self.sh.env, &d, &dty, &v)
                    .map_err(|e| L5xError::new(e, t.span(ins.span.clone())))?;
                self.when_true(&st);
            }
            "CLR" => {
                self.arity(ins, 1, t)?;
                let v = Val {
                    st: "0".into(),
                    dom: Dom::Int,
                    ty: Ty::Elem(Elem::Dint),
                };
                let s = self.store_to(ins, 0, t, &v)?;
                self.when_true(&s);
            }
            "AND" | "OR" | "XOR" | "BAND" | "BOR" | "BXOR" => {
                self.arity(ins, 3, t)?;
                let a = self.val(ins, 0, t)?;
                let b = self.val(ins, 1, t)?;
                let sym = match up.trim_start_matches('B') {
                    "AND" => "AND",
                    "OR" => "OR",
                    _ => "XOR",
                };
                let v = Val {
                    st: format!("({} {sym} {})", zero_fill(&a), zero_fill(&b)),
                    dom: Dom::Int,
                    ty: Ty::Elem(Elem::Lint),
                };
                let s = self.store_to(ins, 2, t, &v)?;
                self.when_true(&s);
            }
            "NOT" => {
                self.arity(ins, 2, t)?;
                let a = self.val(ins, 0, t)?;
                let v = Val {
                    st: format!("(NOT {})", zero_fill(&a)),
                    dom: Dom::Int,
                    ty: Ty::Elem(Elem::Lint),
                };
                let s = self.store_to(ins, 1, t, &v)?;
                self.when_true(&s);
            }
            "BTD" => {
                // Copy Length bits from Source (starting at Source bit) into
                // Dest (starting at Dest bit); bits beyond the destination are
                // lost; no status flags.
                self.arity(ins, 5, t)?;
                let s = self.val(ins, 0, t)?;
                let sb = self.val(ins, 1, t)?;
                let (d, dty) = self.dest(ins, 2, t)?;
                let dv = self.ctx.value(&self.operand(ins, 2, t)?, t)?;
                let db = self.val(ins, 3, t)?;
                let len = self.val(ins, 4, t)?;
                let Some(de) = dty.elem().filter(|e| e.is_int()) else {
                    return Err(L5xError::new(
                        "BTD needs an integer destination",
                        t.span(ins.operands[2].clone()),
                    ));
                };
                let r = Val {
                    st: format!(
                        "lx__btd({}, {}, {}, {}, {})",
                        zero_fill(&s),
                        conv(&sb, Dom::Int),
                        conv(&dv, Dom::Int),
                        conv(&db, Dom::Int),
                        conv(&len, Dom::Int)
                    ),
                    dom: Dom::Int,
                    ty: Ty::Elem(Elem::Lint),
                };
                self.when_true(&format!("{d} := {};", cast(&r, de)));
            }
            "COP" | "CPS" => self.cop(ins, t)?,
            "FLL" => self.fll(ins, t)?,
            // ── Program control (1756-RM003 "Program Control Instructions") ──
            "NOP" | "SBR" => {}
            "AFI" => self.line("lx__rc := FALSE;"),
            "MCR" => {
                // "Each time the MCR instruction is executed with
                // rung-condition-in false, the override behavior is toggled."
                self.line("IF NOT lx__rc THEN lx__mcr := NOT lx__mcr; END_IF;");
            }
            "TND" => self.when_true("RETURN;"),
            "RET" => {
                if !ins.operands.is_empty() {
                    return Err(L5xError::new(
                        "RET with return parameters is not supported yet",
                        t.span(ins.span.clone()),
                    ));
                }
                self.when_true("RETURN;");
            }
            "JSR" => {
                self.arity(ins, 1, t)?;
                let name = t.text[ins.operands[0].clone()].trim().to_string();
                if self.in_aoi {
                    return Err(L5xError::new(
                        "JSR inside an Add-On Instruction",
                        t.span(ins.span.clone()),
                    ));
                }
                let Some(m) = self.routines.get(&name.to_ascii_lowercase()).cloned() else {
                    return Err(L5xError::new(
                        format!("unknown routine `{name}`"),
                        t.span(ins.operands[0].clone()),
                    ));
                };
                let extra = ins.operands.len().saturating_sub(2);
                if extra > 0 {
                    // JSR(routine, input count, inputs..., returns...).
                    let n: usize = t.text[ins.operands[1].clone()].trim().parse().unwrap_or(0);
                    let targets = self
                        .sbr
                        .get(&name.to_ascii_lowercase())
                        .cloned()
                        .unwrap_or_default();
                    if extra > n || targets.len() < n {
                        return Err(L5xError::new(
                            "JSR with return parameters, or input parameters without a matching SBR, is not supported yet",
                            t.span(ins.span.clone()),
                        ));
                    }
                    let mut stmts = Vec::new();
                    for (k, (target, tty)) in targets.iter().enumerate().take(n) {
                        let v = self.val(ins, 2 + k, t)?;
                        let value = match tty.elem() {
                            Some(e) => cast(&v, e),
                            None => v.st,
                        };
                        stmts.push(format!("{target} := {value};"));
                    }
                    stmts.push(format!("{m}();"));
                    self.when_true(&stmts.join("\n    "));
                } else {
                    self.when_true(&format!("{m}();"));
                }
            }
            "JMP" => {
                self.arity(ins, 1, t)?;
                let name = t.text[ins.operands[0].clone()].trim().to_ascii_lowercase();
                let Some(id) = self.labels.get(&name).copied() else {
                    return Err(L5xError::new(
                        "JMP to a label that is not in this routine",
                        t.span(ins.operands[0].clone()),
                    ));
                };
                self.when_true(&format!("lx__jmp := {id};"));
                // Jumped: the rest of the rung does not run.
                self.line("IF lx__jmp = 0 THEN");
                self.guards += 1;
            }
            "LBL" => {}
            "EVENT" => {
                self.arity(ins, 1, t)?;
                let name = t.text[ins.operands[0].clone()].trim();
                self.when_true(&format!("lx__event_{} := TRUE;", crate::names::ident(name)));
            }
            _ => {
                if let Some(sig) = self.sh.aois.get(&ins.name.to_ascii_lowercase()) {
                    let sig = sig.clone();
                    return self.aoi_call(&sig, ins, t);
                }
                return Err(L5xError::new(
                    format!("instruction `{}` is not supported yet", ins.name),
                    t.span(ins.name_span.clone()),
                ));
            }
        }
        Ok(())
    }

    fn aoi_call(&mut self, sig: &AoiSig, ins: &Instr, t: &Text) -> Res<()> {
        self.arity(ins, 1, t)?;
        let (tag, ty) = self.dest(ins, 0, t)?;
        let ok = matches!(&ty, Ty::Struct(i) if self.sh.env.get(*i).st == sig.st);
        if !ok {
            return Err(L5xError::new(
                format!(
                    "the backing tag of {} must be of type {}",
                    ins.name, ins.name
                ),
                t.span(ins.operands[0].clone()),
            ));
        }
        let req: Vec<_> = sig
            .params
            .iter()
            .filter(|p| {
                p.required
                    && !p.logix.eq_ignore_ascii_case("EnableIn")
                    && !p.logix.eq_ignore_ascii_case("EnableOut")
            })
            .collect();
        if ins.operands.len() != req.len() + 1 {
            return Err(L5xError::new(
                format!(
                    "{} expects {} operand(s), found {}",
                    ins.name,
                    req.len() + 1,
                    ins.operands.len()
                ),
                t.span(ins.span.clone()),
            ));
        }
        let mut args = vec!["EnableIn := lx__rc".to_string()];
        let mut inouts = Vec::new();
        let mut outs = Vec::new();
        for (k, p) in req.iter().enumerate() {
            match p.usage {
                Usage::Input => {
                    let v = self.val(ins, k + 1, t)?;
                    let a = match p.ty.elem() {
                        Some(e) => cast(&v, e),
                        None => v.st,
                    };
                    args.push(format!("{} := {a}", p.st));
                }
                Usage::InOut => {
                    let (d, _) = self.dest(ins, k + 1, t)?;
                    args.push(format!("{} := {d}", p.st));
                    inouts.push(format!("{} := {d}", p.st));
                }
                _ => {
                    let (d, dty) = self.dest(ins, k + 1, t)?;
                    outs.push((p.st.clone(), d, dty, p.ty.clone()));
                }
            }
        }
        self.line(&format!("{tag}({});", args.join(", ")));
        for (pst, d, dty, pty) in outs {
            if dty == pty {
                self.line(&format!("{d} := {tag}.{pst};"));
            } else if let (Some(de), Some(pd)) = (dty.elem(), dom_of(&pty)) {
                let v = Val {
                    st: format!("{tag}.{pst}"),
                    dom: pd,
                    ty: pty.clone(),
                };
                let v = Val {
                    st: conv(&v, if pd == Dom::Bool { Dom::Bool } else { pd }),
                    ..v
                };
                self.line(&format!("{d} := {};", cast(&v, de)));
            }
        }
        self.line(&format!("lx__rc := {tag}.EnableOut;"));
        let mut pargs = vec!["lx__prescan := TRUE".to_string()];
        pargs.extend(inouts);
        self.pre_line(&format!("{tag}({});", pargs.join(", ")));
        Ok(())
    }

    /// Split `arr[i]` into (`arr` ST, element type, index ST, length of the
    /// dimension); a plain tag is (tag, type, "0", 1).
    fn element_base(&self, e: &LExpr, t: &Text) -> Res<(String, Ty, String, u32)> {
        let LKind::Path(p) = &e.kind else {
            return Err(L5xError::new("expected a tag", t.span(e.span.clone())));
        };
        if let Some(Seg::Index(ix, _)) = p.segs.last()
            && ix.len() == 1
        {
            let base = TagPath {
                base: p.base.clone(),
                base_span: p.base_span.clone(),
                segs: p.segs[..p.segs.len() - 1].to_vec(),
            };
            let (st, ty) = self.ctx.path(&base, t)?;
            if let Ty::Array(el, dims) = ty
                && dims.len() == 1
            {
                let i = self.ctx.value(&ix[0], t)?;
                return Ok((st, *el, conv(&i, Dom::Int), dims[0]));
            }
        }
        let (st, ty) = self.ctx.path(p, t)?;
        if let Ty::Array(el, dims) = &ty
            && dims.len() == 1
        {
            return Ok((st, (**el).clone(), "0".into(), dims[0]));
        }
        Ok((st, ty, "0".into(), 1))
    }

    /// COP/CPS: copy Length elements of the destination's type. Supported when
    /// source and destination elements have the same type (the byte-level
    /// reinterpretation of unlike types is not, since plcc's layout differs).
    fn cop(&mut self, ins: &Instr, t: &Text) -> Res<()> {
        self.arity(ins, 3, t)?;
        let se = self.operand(ins, 0, t)?;
        let de = self.operand(ins, 1, t)?;
        let (sb, sty, si, slen) = self.element_base(&se, t)?;
        let (db, dty, di, dlen) = self.element_base(&de, t)?;
        let len = self.val(ins, 2, t)?;
        if sty != dty {
            return Err(L5xError::new(
                format!(
                    "COP from {} to {} (unlike types) is not supported",
                    self.sh.env.logix(&sty),
                    self.sh.env.logix(&dty)
                ),
                t.span(ins.span.clone()),
            ));
        }
        self.temps.insert("lx__i : LINT".into());
        self.temps.insert("lx__n : LINT".into());
        let single = slen == 1 && dlen == 1 && si == "0" && di == "0";
        if single {
            self.when_true(&format!("{db} := {sb};"));
            return Ok(());
        }
        let sidx = |x: &str| {
            if slen == 1 && si == "0" {
                sb.clone()
            } else {
                format!("{sb}[LINT_TO_DINT({si} + {x})]")
            }
        };
        let didx = |x: &str| {
            if dlen == 1 && di == "0" {
                db.clone()
            } else {
                format!("{db}[LINT_TO_DINT({di} + {x})]")
            }
        };
        // The copy stops at the end of either array (1756-RM003 COP: "the
        // instruction does not write past the end of the destination").
        let stmt = format!(
            "lx__n := {len};\n    IF lx__n > {dlen} - {di} THEN lx__n := {dlen} - {di}; END_IF;\n    IF lx__n > {slen} - {si} THEN lx__n := {slen} - {si}; END_IF;\n    FOR lx__i := 0 TO lx__n - 1 DO\n        {} := {};\n    END_FOR;",
            didx("lx__i"),
            sidx("lx__i"),
            len = conv(&len, Dom::Int),
        );
        self.when_true(&stmt);
        Ok(())
    }

    fn fll(&mut self, ins: &Instr, t: &Text) -> Res<()> {
        self.arity(ins, 3, t)?;
        let v = self.val(ins, 0, t)?;
        let de = self.operand(ins, 1, t)?;
        let (db, dty, di, dlen) = self.element_base(&de, t)?;
        let len = self.val(ins, 2, t)?;
        self.temps.insert("lx__i : LINT".into());
        self.temps.insert("lx__n : LINT".into());
        let target = if dlen == 1 && di == "0" {
            db.clone()
        } else {
            format!("{db}[LINT_TO_DINT({di} + lx__i)]")
        };
        let Some(e) = dty.elem() else {
            return Err(L5xError::new(
                "FLL of a structure is not supported",
                t.span(ins.span.clone()),
            ));
        };
        let stmt = format!(
            "lx__n := {len};\n    IF lx__n > {dlen} - {di} THEN lx__n := {dlen} - {di}; END_IF;\n    FOR lx__i := 0 TO lx__n - 1 DO\n        {target} := {};\n    END_FOR;",
            cast(&v, e),
            len = conv(&len, Dom::Int),
        );
        self.when_true(&stmt);
        Ok(())
    }
}
