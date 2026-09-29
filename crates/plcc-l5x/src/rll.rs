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
use crate::scope::{Ctx, Dom, SpanOf, Val, conv, dom_of};
use crate::types::{Elem, StructKind, Ty, TypeEnv};
use crate::xml::Text;
use plcc_st::Span;
use std::collections::{BTreeSet, HashMap};

pub(crate) struct Shared<'a> {
    pub src: &'a str,
    pub env: &'a TypeEnv,
    pub aois: &'a HashMap<String, AoiSig>,
    /// String helpers generated on demand.
    pub strings: &'a crate::strings::Helpers,
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
    /// SBR operands and RET types of each routine (JSR parameters).
    subs: &'a Subs,
    /// This routine's method (RET return values are stored per routine).
    method: String,
}

/// Subroutine parameters of a program's routines (1756-RM003 JSR/SBR/RET): the
/// tags each routine's SBR copies inputs into, and the types of the values
/// its RET instructions return (kept in hidden `lx__ret_<method>_<k>`
/// variables of the program).
#[derive(Default)]
pub(crate) struct Subs {
    pub sbr: HashMap<String, Vec<(String, Ty)>>,
    pub ret: HashMap<String, Vec<Ty>>,
}

impl Subs {
    pub fn ret_var(method: &str, k: usize) -> String {
        format!("lx__ret_{method}_{k}")
    }
}

type Res<T> = Result<T, L5xError>;

pub(crate) fn routine(
    sh: &Shared,
    ctx: &Ctx,
    r: &RoutineDef,
    routines: &HashMap<String, String>,
    in_aoi: bool,
) -> (RoutineOut, Vec<L5xError>) {
    let empty = Subs::default();
    routine_with(sh, ctx, r, routines, in_aoi, &empty)
}

pub(crate) fn routine_with(
    sh: &Shared,
    ctx: &Ctx,
    r: &RoutineDef,
    routines: &HashMap<String, String>,
    in_aoi: bool,
    subs: &Subs,
) -> (RoutineOut, Vec<L5xError>) {
    let method = routines
        .get(&r.name.text.to_ascii_lowercase())
        .cloned()
        .unwrap_or_default();
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
        subs,
        method,
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
pub(crate) fn subroutines(
    ctx: &Ctx,
    routines: &[RoutineDef],
    names: &HashMap<String, String>,
) -> Subs {
    let mut subs = Subs {
        sbr: sbr_params(ctx, routines),
        ret: HashMap::new(),
    };
    // RET operand types: the first RET with operands in each routine.
    for r in routines {
        let Some(method) = names.get(&r.name.text.to_ascii_lowercase()) else {
            continue;
        };
        let found = if r.kind == crate::model::RoutineKind::St {
            crate::stx::ret_types(ctx, r)
        } else {
            let mut found = None;
            'rungs: for rung in &r.rungs {
                let Some(text) = &rung.text else { continue };
                let Ok(rs) = rung::parse(&text.text) else {
                    continue;
                };
                for seq in &rs {
                    if let Some(tys) = ret_in_seq(ctx, seq, text) {
                        found = Some(tys);
                        break 'rungs;
                    }
                }
            }
            found
        };
        if let Some(tys) = found {
            subs.ret.insert(method.to_ascii_lowercase(), tys);
        }
    }
    subs
}

fn ret_in_seq(ctx: &Ctx, seq: &[Element], text: &Text) -> Option<Vec<Ty>> {
    for e in seq {
        match e {
            Element::Instr(i) if i.name.eq_ignore_ascii_case("RET") && !i.operands.is_empty() => {
                let mut tys = Vec::new();
                for op in &i.operands {
                    let ex = operand::parse_expr(&text.text, op.clone()).ok()?;
                    let v = ctx.value(&ex, text).ok()?;
                    tys.push(match v.dom {
                        Dom::Bool => Ty::Elem(Elem::Bool),
                        Dom::Real => Ty::Elem(Elem::Real),
                        Dom::LReal => Ty::Elem(Elem::Lreal),
                        Dom::Int => match v.ty.elem() {
                            Some(e) if e.is_int() => Ty::Elem(e),
                            _ => Ty::Elem(Elem::Dint),
                        },
                    });
                }
                return Some(tys);
            }
            Element::Branch(legs, _) => {
                for l in legs {
                    if let Some(t) = ret_in_seq(ctx, l, text) {
                        return Some(t);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn sbr_params(ctx: &Ctx, routines: &[RoutineDef]) -> HashMap<String, Vec<(String, Ty)>> {
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

    /// `bit := value;` for OTE/OTL/OTU, including an indirect bit
    /// `tag.[n]` (1756-RM003 "Bit Addressing"), which rewrites the integer.
    fn bit_write(&self, ins: &Instr, i: usize, t: &Text, value: &str) -> Res<String> {
        let e = self.operand(ins, i, t)?;
        if let LKind::Path(p) = &e.kind
            && let Some(Seg::IndirectBit(ix)) = p.segs.last()
        {
            let base = TagPath {
                base: p.base.clone(),
                base_span: p.base_span.clone(),
                segs: p.segs[..p.segs.len() - 1].to_vec(),
            };
            let (st, ty) = self.ctx.path(&base, t)?;
            let Some(el) = ty.elem().filter(|e| e.is_int()) else {
                return Err(L5xError::new("indirect bit access needs an integer", t.span(e.span)));
            };
            let n = conv(&self.ctx.value(ix, t)?, Dom::Int);
            let whole = if el == Elem::Lint { st.clone() } else { format!("{}_TO_LINT({st})", el.st()) };
            let set = format!("lx__setbitl({whole}, {n}, {value})");
            let set = if el == Elem::Lint { set } else { format!("LINT_TO_{}({set})", el.st()) };
            return Ok(format!("{st} := {set};"));
        }
        Ok(format!("{} := {value};", self.bit_dest(ins, i, t)?))
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
                let set = self.bit_write(ins, 0, t, "lx__rc")?;
                self.line(&set);
                let clear = self.bit_write(ins, 0, t, "FALSE")?;
                self.pre_line(&clear);
            }
            "OTL" => {
                self.arity(ins, 1, t)?;
                let set = self.bit_write(ins, 0, t, "TRUE")?;
                self.when_true(&set);
            }
            "OTU" => {
                self.arity(ins, 1, t)?;
                let set = self.bit_write(ins, 0, t, "FALSE")?;
                self.when_true(&set);
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
                if let (Some((sa, ta)), Some((sb, tb))) = (
                    self.string_operand(ins, 0, t),
                    self.string_operand(ins, 1, t),
                ) {
                    // String compare (1756-RM003 EQ/NE/LT/... "String Compare").
                    let f = self
                        .sh
                        .strings
                        .compare(self.sh.env, &ta, &tb)
                        .ok_or_else(|| L5xError::new("string compare", t.span(ins.span.clone())))?;
                    let op = match up.as_str() {
                        "EQU" | "EQ" => "=",
                        "NEQ" | "NE" => "<>",
                        "LES" | "LT" => "<",
                        "LEQ" | "LE" => "<=",
                        "GRT" | "GT" => ">",
                        _ => ">=",
                    };
                    self.line(&format!("lx__rc := lx__rc AND ({f}({sa}, {sb}) {op} 0);"));
                    return Ok(());
                }
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
                if let Some((s, sty)) = self.string_operand(ins, 0, t)
                    && crate::strings::cap(self.sh.env, &dty).is_some()
                    && sty != dty
                {
                    let f = self
                        .sh
                        .strings
                        .copy(self.sh.env, &sty, &dty)
                        .unwrap_or_default();
                    self.when_true(&format!("lx__S_V := FALSE; {f}({s}, {d});"));
                    return Ok(());
                }
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
            "CONCAT" | "MID" | "DELETE" | "INSERT" | "FIND" | "UPPER" | "LOWER" | "DTOS"
            | "STOD" => {
                let stmt = self.string_instr(&up, ins, t)?;
                self.when_true(&stmt);
            }
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
                // Return parameters go to hidden per-routine variables the JSR
                // copies into its return operands.
                let mut stmts = Vec::new();
                let types = self
                    .subs
                    .ret
                    .get(&self.method.to_ascii_lowercase())
                    .cloned()
                    .unwrap_or_default();
                for k in 0..ins.operands.len() {
                    let v = self.val(ins, k, t)?;
                    let value = match types.get(k).and_then(|t| t.elem()) {
                        Some(e) => cast(&v, e),
                        None => v.st,
                    };
                    stmts.push(format!("{} := {value};", Subs::ret_var(&self.method, k)));
                }
                stmts.push("RETURN;".into());
                self.when_true(&stmts.join("\n    "));
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
                        .subs
                        .sbr
                        .get(&name.to_ascii_lowercase())
                        .cloned()
                        .unwrap_or_default();
                    // More inputs than the SBR takes: the extras are not
                    // received. Fewer: "A major fault will occur if: JSR
                    // instruction has fewer input parameters than SBR
                    // instruction" (4/31, 1756-RM003 JSR/SBR/RET).
                    if targets.len() < n {
                        self.errors.push(L5xError::warning(
                            format!(
                                "JSR passes {n} input(s) but `{name}` takes {}; the rest are ignored",
                                targets.len()
                            ),
                            t.span(ins.span.clone()),
                        ));
                    } else if targets.len() > n {
                        return Err(L5xError::new(
                            format!(
                                "JSR passes {n} input(s) but the SBR of `{name}` takes {} (a major fault 4/31 on a controller)",
                                targets.len()
                            ),
                            t.span(ins.span.clone()),
                        ));
                    }
                    let mut stmts = Vec::new();
                    for (k, (target, tty)) in targets.iter().enumerate().take(n) {
                        let value = match tty.elem() {
                            Some(e) => cast(&self.val(ins, 2 + k, t)?, e),
                            // A structure or array parameter: copied whole.
                            None => {
                                let (s, sty) = self.dest(ins, 2 + k, t)?;
                                if &sty != tty {
                                    return Err(L5xError::new(
                                        format!(
                                            "JSR passes a {} where SBR expects a {}",
                                            self.sh.env.logix(&sty),
                                            self.sh.env.logix(tty)
                                        ),
                                        t.span(ins.operands[2 + k].clone()),
                                    ));
                                }
                                s
                            }
                        };
                        stmts.push(format!("{target} := {value};"));
                    }
                    stmts.push(format!("{m}();"));
                    // Return operands, after the inputs.
                    let rtypes = self
                        .subs
                        .ret
                        .get(&m.to_ascii_lowercase())
                        .cloned()
                        .unwrap_or_default();
                    for k in 0..extra.saturating_sub(n) {
                        let (d, dty) = self.dest(ins, 2 + n + k, t)?;
                        let Some(rt) = rtypes.get(k) else {
                            return Err(L5xError::new(
                                format!("`{name}` returns fewer values than this JSR expects"),
                                t.span(ins.operands[2 + n + k].clone()),
                            ));
                        };
                        let rv = Subs::ret_var(&m, k);
                        match (dty.elem(), dom_of(rt)) {
                            (Some(de), Some(rd)) => {
                                let v = Val {
                                    st: rv,
                                    dom: rd,
                                    ty: rt.clone(),
                                };
                                let v = Val {
                                    st: conv(&v, rd),
                                    ..v
                                };
                                stmts.push(format!("{d} := {};", cast(&v, de)));
                            }
                            _ => stmts.push(format!("{d} := {rv};")),
                        }
                    }
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
            // plcc's tasks do not preempt one another (docs/process-image.md):
            // there are no user-task interrupts to disable or enable.
            "UID" | "UIE" => {}
            "SIZE" => {
                // SIZE(Source, Dimension to vary, Size): a constant, since
                // plcc arrays have fixed bounds (1756-RM003 "Size In Elements").
                self.arity(ins, 3, t)?;
                let e = self.operand(ins, 0, t)?;
                let LKind::Path(p) = &e.kind else {
                    return Err(L5xError::new("SIZE needs an array tag", t.span(e.span)));
                };
                let dims = self.ctx.size_dims(p, t)?;
                let dim = match self.operand(ins, 1, t)?.kind {
                    LKind::Int(n) => n as usize,
                    _ => 0,
                };
                let Some(n) = dims.get(dim) else {
                    return Err(L5xError::new(
                        format!("the array has no dimension {dim}"),
                        t.span(ins.operands[1].clone()),
                    ));
                };
                let (d, dty) = self.dest(ins, 2, t)?;
                let v = Val {
                    st: n.to_string(),
                    dom: Dom::Int,
                    ty: Ty::Elem(Elem::Dint),
                };
                let value = match dty.elem() {
                    Some(e) => cast(&v, e),
                    None => v.st,
                };
                self.when_true(&format!("{d} := {value};"));
            }
            "FOR" => {
                // FOR(Routine, Index, Initial, Terminal, Step): "repeatedly
                // executes the Routine until the Index value exceeds the Terminal
                // value" (below it for a negative step), adding Step each time;
                // BRK ends the loop (1756-RM003 "For (FOR)", "Break (BRK)").
                self.arity(ins, 5, t)?;
                if self.in_aoi {
                    return Err(L5xError::new(
                        "FOR inside an Add-On Instruction",
                        t.span(ins.span.clone()),
                    ));
                }
                let name = t.text[ins.operands[0].clone()].trim().to_string();
                let Some(m) = self.routines.get(&name.to_ascii_lowercase()).cloned() else {
                    return Err(L5xError::new(
                        format!("unknown routine `{name}`"),
                        t.span(ins.operands[0].clone()),
                    ));
                };
                let (idx, ity) = self.dest(ins, 1, t)?;
                let Some(ie) = ity.elem().filter(|e| e.is_int()) else {
                    return Err(L5xError::new(
                        "the FOR index must be an integer",
                        t.span(ins.operands[1].clone()),
                    ));
                };
                let init = self.val(ins, 2, t)?;
                let term = self.val(ins, 3, t)?;
                let step = self.val(ins, 4, t)?;
                self.temps.insert("lx__fstep : LINT".into());
                self.temps.insert("lx__fterm : LINT".into());
                let stmt = format!(
                    "{idx} := {init};\n    lx__fterm := {term};\n    lx__fstep := {step};\n    lx__for_depth := lx__for_depth + 1;\n    \
                     WHILE (lx__fstep >= 0 AND {idxl} <= lx__fterm) OR (lx__fstep < 0 AND {idxl} >= lx__fterm) DO\n        \
                     {m}();\n        IF lx__brk THEN lx__brk := FALSE; EXIT; END_IF;\n        \
                     {idx} := LINT_TO_{e}({idxl} + lx__fstep);\n    END_WHILE;\n    lx__for_depth := lx__for_depth - 1;",
                    init = cast(&init, ie),
                    term = conv(&term, Dom::Int),
                    step = conv(&step, Dom::Int),
                    idxl = if ie == Elem::Lint {
                        idx.clone()
                    } else {
                        format!("{}_TO_LINT({idx})", ie.st())
                    },
                    e = ie.st(),
                );
                self.when_true(&stmt);
            }
            "BRK" => {
                // "If no FOR instruction preceded this BRK instruction in its
                // execution during this scan then BRK does not initiate."
                self.when_true("IF lx__for_depth > 0 THEN lx__brk := TRUE; RETURN; END_IF;");
            }
            "GSV" | "SSV" => {
                let what = ins
                    .operands
                    .iter()
                    .take(3)
                    .map(|r| t.text[r.clone()].trim().to_string())
                    .collect::<Vec<_>>()
                    .join(".");
                self.errors.push(L5xError::warning(
                    format!(
                        "{} {what}: plcc has no controller object model; {}",
                        up,
                        if up == "GSV" {
                            "the destination keeps its value"
                        } else {
                            "nothing is set"
                        }
                    ),
                    t.span(ins.span.clone()),
                ));
            }
            "MSG" => {
                self.arity(ins, 1, t)?;
                self.structure(ins, 0, t, &["MESSAGE"])?;
                self.errors.push(L5xError::warning(
                    "MSG: plcc has no CIP messaging; the message never starts (EN, DN and ER stay FALSE)",
                    t.span(ins.span.clone()),
                ));
            }
            "BSL" | "BSR" => self.bit_shift(ins, t, up == "BSL")?,
            "FFL" | "LFL" => self.stack_load(ins, t)?,
            "FFU" | "LFU" => self.stack_unload(ins, t, up == "LFU")?,
            "SWPB" => {
                self.arity(ins, 3, t)?;
                let se = self.operand(ins, 0, t)?;
                let de = self.operand(ins, 2, t)?;
                let mode = t.text[ins.operands[1].clone()].trim();
                let stmt = swpb_code(self.ctx, &se, mode, &de, t, t.span(ins.span.clone()))?;
                self.when_true(&stmt);
            }
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
        let ok =
            matches!(&ty, Ty::Struct(i) if self.sh.env.get(*i).st.eq_ignore_ascii_case(&sig.st));
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
                    if p.alias {
                        // An alias parameter is a member of the backing tag.
                        self.line(&format!("{tag}.{} := {a};", p.st));
                    } else {
                        args.push(format!("{} := {a}", p.st));
                    }
                }
                Usage::InOut if p.alias => {
                    return Err(L5xError::new(
                        format!("InOut alias parameter `{}` is not supported", p.logix),
                        t.span(ins.operands[k + 1].clone()),
                    ));
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

    /// Operand `i` when it is a string tag: its ST path and type.
    fn string_operand(&self, ins: &Instr, i: usize, t: &Text) -> Option<(String, Ty)> {
        let e = self.operand(ins, i, t).ok()?;
        string_path(self.ctx, &e, t)
    }

    /// The ASCII string instructions (see [`crate::strings`]).
    fn string_instr(&mut self, up: &str, ins: &Instr, t: &Text) -> Res<String> {
        let mut ops = Vec::new();
        for i in 0..ins.operands.len() {
            ops.push(self.operand(ins, i, t)?);
        }
        string_code(
            self.ctx,
            self.sh.strings,
            up,
            &ops,
            t,
            t.span(ins.span.clone()),
        )
    }

    fn element_base(&self, e: &LExpr, t: &Text) -> Res<(String, Ty, String, u32)> {
        element_base(self.ctx, e, t)
    }

    /// COP/CPS (see [`cop_code`]).
    fn cop(&mut self, ins: &Instr, t: &Text) -> Res<()> {
        self.arity(ins, 3, t)?;
        let se = self.operand(ins, 0, t)?;
        let de = self.operand(ins, 1, t)?;
        let len = self.val(ins, 2, t)?;
        let stmt = cop_code(self.ctx, &se, &de, &len, t, t.span(ins.span.clone()))?;
        self.temps.insert("lx__i : LINT".into());
        self.temps.insert("lx__n : LINT".into());
        self.when_true(&stmt);
        Ok(())
    }

    /// BSL / BSR (1756-RM003 "Bit Shift Left/Right", flow charts): on the
    /// rung's false→true transition, shift .LEN bits of the DINT array (from
    /// the given element) one position, unloading the bit shifted out into
    /// .UL and loading Source Bit into the vacated position; DN := 1,
    /// POS := LEN. .LEN < 0 sets ER. A false rung clears EN, DN, ER, POS.
    fn bit_shift(&mut self, ins: &Instr, t: &Text, left: bool) -> Res<()> {
        self.arity(ins, 3, t)?;
        let ae = self.operand(ins, 0, t)?;
        let (arr, ety, start, alen) = self.element_base(&ae, t)?;
        if ety.elem() != Some(Elem::Dint) {
            return Err(L5xError::new(
                format!("{} needs a DINT array", ins.name),
                t.span(ins.operands[0].clone()),
            ));
        }
        let (c, _) = self.structure(ins, 1, t, &["CONTROL"])?;
        let src = self.bit(ins, 2, t)?;
        self.pseudo(ins, t, &c, 3, "LEN");
        self.temps.insert("lx__i : LINT".into());
        self.temps.insert("lx__b : BOOL".into());
        let word = |i: &str| {
            if alen == 1 && start == "0" {
                arr.clone()
            } else {
                format!("{arr}[LINT_TO_DINT({start} + ({i}) / 32)]")
            }
        };
        let get = |i: &str| format!("lx__getbit(DINT_TO_LINT({}), ({i}) MOD 32)", word(i));
        let set = |i: &str, v: &str| {
            format!(
                "{w} := lx__setbit_dint({w}, ({i}) MOD 32, {v});",
                w = word(i)
            )
        };
        let cap = format!("({alen} - {start}) * 32");
        let shift = if left {
            format!(
                "{c}.UL := {top};\n            FOR lx__i := {c}.LEN - 1 TO 1 BY -1 DO\n                lx__b := {prev};\n                {setb}\n            END_FOR;\n            {set0}",
                top = get(&format!("{c}.LEN - 1")),
                prev = get("lx__i - 1"),
                setb = set("lx__i", "lx__b"),
                set0 = set("0", &src),
            )
        } else {
            format!(
                "{c}.UL := {bottom};\n            FOR lx__i := 0 TO {c}.LEN - 2 DO\n                lx__b := {next};\n                {setb}\n            END_FOR;\n            {setl}",
                bottom = get("0"),
                next = get("lx__i + 1"),
                setb = set("lx__i", "lx__b"),
                setl = set(&format!("{c}.LEN - 1"), &src),
            )
        };
        self.line(&format!(
            "IF lx__rc THEN\n    IF NOT {c}.EN THEN\n        {c}.EN := TRUE; {c}.DN := FALSE; {c}.ER := FALSE; {c}.UL := FALSE;\n        \
             {c}.EU := FALSE; {c}.EM := FALSE; {c}.IN := FALSE; {c}.FD := FALSE;\n        \
             IF {c}.LEN = 0 THEN\n            {c}.DN := TRUE; {c}.POS := {c}.LEN; {c}.UL := {src};\n        \
             ELSIF {c}.LEN < 0 OR {c}.LEN > {cap} THEN\n            {c}.ER := TRUE;\n        \
             ELSE\n            {shift}\n            {c}.DN := TRUE; {c}.POS := {c}.LEN;\n        END_IF;\n    END_IF;\n\
             ELSE\n    {c}.EN := FALSE; {c}.DN := FALSE; {c}.ER := FALSE; {c}.POS := 0;\nEND_IF;"
        ));
        self.pre_line(&format!(
            "{c}.EN := FALSE; {c}.DN := FALSE; {c}.ER := FALSE; {c}.POS := 0;"
        ));
        Ok(())
    }

    /// The FIFO/LIFO operand of FFL/FFU/LFL/LFU (its first element) and an
    /// accessor for element `k` counted from there.
    fn stack_array(&self, ins: &Instr, i: usize, t: &Text) -> Res<(Ty, impl Fn(&str) -> String + use<>)> {
        let e = self.operand(ins, i, t)?;
        let indexed = match &e.kind {
            LKind::Path(p) => {
                matches!(p.segs.last(), Some(Seg::Index(..)))
                    || matches!(self.ctx.path(p, t)?.1, Ty::Array(..))
            }
            _ => false,
        };
        if !indexed {
            return Err(L5xError::new(
                format!("{} needs an array", ins.name),
                t.span(ins.operands[i].clone()),
            ));
        }
        let (arr, ety, start, _) = self.element_base(&e, t)?;
        Ok((ety, move |k: &str| format!("{arr}[LINT_TO_DINT({start} + ({k}))]")))
    }

    /// `dest := src` between a FIFO element and the Source/Destination:
    /// numbers converted without touching the status flags ("If Source and
    /// FIFO data types mismatch, the instruction converts the Source value"),
    /// strings by LEN and characters, structures of the same type as a whole.
    fn stack_move(&self, d: &str, dty: &Ty, s: &str, sty: &Ty, sp: Span) -> Res<String> {
        if let (Some(e), Some(dom)) = (dty.elem(), dom_of(sty)) {
            let v = Val { st: s.to_string(), dom, ty: sty.clone() };
            return Ok(format!("{d} := {};", cast(&v, e)));
        }
        if sty == dty {
            return Ok(format!("{d} := {s};"));
        }
        if let Some(f) = self.sh.strings.copy(self.sh.env, sty, dty) {
            return Ok(format!("{f}({s}, {d});"));
        }
        Err(L5xError::new(
            format!(
                "cannot move a {} into a {}",
                self.sh.env.logix(sty),
                self.sh.env.logix(dty)
            ),
            sp,
        ))
    }

    /// A zero of type `ty` (FFU/LFU return 0 from an empty stack, LFU clears
    /// the unloaded element): a literal, or a hidden never-written variable.
    fn zero_of(&mut self, ty: &Ty) -> String {
        match dom_of(ty) {
            Some(Dom::Bool) => "FALSE".into(),
            Some(Dom::Int) => "0".into(),
            Some(_) => "0.0".into(),
            None => {
                let st = self.sh.env.st(ty);
                let name = format!("lx__zero_{}", st.replace(|c: char| !c.is_ascii_alphanumeric(), "_"));
                self.temps.insert(format!("{name} : {st}"));
                name
            }
        }
    }

    /// FFL / LFL (1756-RM003 "FIFO Load (FFL)", "LIFO Load (LFL)", their
    /// flow charts): on the rung's false-to-true transition (EN), load Source
    /// at `.POS` and advance `.POS` unless the stack is full (DN); every
    /// execution updates DN (`.POS >= .LEN`) and EM (`.POS = 0`), both set
    /// when `.LEN <= 0` or `.POS < 0`. Prescan sets EN "to prevent a false
    /// load when scan begins".
    fn stack_load(&mut self, ins: &Instr, t: &Text) -> Res<()> {
        self.arity(ins, 5, t)?;
        let (ety, el) = self.stack_array(ins, 1, t)?;
        let (c, _) = self.structure(ins, 2, t, &["CONTROL"])?;
        self.pseudo(ins, t, &c, 3, "LEN");
        self.pseudo(ins, t, &c, 4, "POS");
        let sp = t.span(ins.span.clone());
        let numeric = ety.elem().filter(|_| dom_of(&ety).is_some());
        let load = if let Some(e) = numeric {
            let v = self.val(ins, 0, t)?;
            format!("{} := {};", el(&format!("{c}.POS - 1")), cast(&v, e))
        } else {
            let se = self.operand(ins, 0, t)?;
            let (s, sty) = match &se.kind {
                LKind::Path(p) => self.ctx.path(p, t)?,
                _ => return Err(L5xError::new("the Source must be a tag", t.span(se.span.clone()))),
            };
            self.stack_move(&el(&format!("{c}.POS - 1")), &ety, &s, &sty, sp)?
        };
        let status = stack_status(&c);
        self.line(&format!(
            "IF lx__rc THEN\n    IF {c}.LEN <= 0 OR {c}.POS < 0 THEN\n        {c}.DN := TRUE; {c}.EM := TRUE;\n    \
             ELSIF NOT {c}.EN THEN\n        {c}.DN := FALSE; {c}.EM := FALSE;\n        {c}.EN := TRUE;\n        \
             {c}.POS := {c}.POS + 1;\n        IF {c}.POS >= {c}.LEN THEN {c}.DN := TRUE; END_IF;\n        \
             IF {c}.POS > {c}.LEN THEN\n            {c}.POS := {c}.POS - 1;\n        ELSE\n            {load}\n        END_IF;\n    \
             ELSE\n        {status}\n    END_IF;\nELSE\n    {c}.EN := FALSE;\n    {status}\nEND_IF;"
        ));
        self.pre_line(&format!("{c}.EN := TRUE; {status}"));
        Ok(())
    }

    /// FFU / LFU (1756-RM003 "FIFO Unload (FFU)", "LIFO Unload (LFU)"): on
    /// the rung's false-to-true transition (EU), an empty stack (`.POS < 1`)
    /// returns 0; otherwise FFU returns element 0 and shifts elements
    /// 1..LEN-1 down one, LFU returns element `.POS - 1` and stores 0 there;
    /// `.POS` goes down by one and EM is set when the stack is now empty.
    /// Status bits as for FFL; prescan sets EU.
    fn stack_unload(&mut self, ins: &Instr, t: &Text, lifo: bool) -> Res<()> {
        self.arity(ins, 5, t)?;
        let (ety, el) = self.stack_array(ins, 0, t)?;
        let (d, dty) = self.dest(ins, 1, t)?;
        let (c, _) = self.structure(ins, 2, t, &["CONTROL"])?;
        self.pseudo(ins, t, &c, 3, "LEN");
        self.pseudo(ins, t, &c, 4, "POS");
        let sp = t.span(ins.span.clone());
        let dz = self.zero_of(&dty);
        let dzero = self.stack_move(&d, &dty, &dz, &dty, sp)?;
        let unload = if lifo {
            let ez = self.zero_of(&ety);
            format!(
                "IF {c}.POS > {c}.LEN THEN {c}.POS := {c}.LEN; END_IF;\n            {c}.POS := {c}.POS - 1;\n            \
                 {get}\n            {clear}",
                get = self.stack_move(&d, &dty, &el(&format!("{c}.POS")), &ety, sp)?,
                clear = self.stack_move(&el(&format!("{c}.POS")), &ety, &ez, &ety, sp)?,
            )
        } else {
            self.temps.insert("lx__i : LINT".into());
            format!(
                "{c}.POS := {c}.POS - 1;\n            {get}\n            \
                 FOR lx__i := 1 TO DINT_TO_LINT({c}.LEN) - 1 DO\n                {shift}\n            END_FOR;",
                get = self.stack_move(&d, &dty, &el("0"), &ety, sp)?,
                shift = self.stack_move(&el("lx__i - 1"), &ety, &el("lx__i"), &ety, sp)?,
            )
        };
        let status = stack_status(&c);
        self.line(&format!(
            "IF lx__rc THEN\n    IF {c}.LEN <= 0 OR {c}.POS < 0 THEN\n        {c}.DN := TRUE; {c}.EM := TRUE;\n    \
             ELSIF NOT {c}.EU THEN\n        {c}.DN := FALSE; {c}.EM := FALSE;\n        {c}.EU := TRUE;\n        \
             IF {c}.POS <= 1 THEN {c}.EM := TRUE; END_IF;\n        IF {c}.POS < 1 THEN\n            {dzero}\n        \
             ELSE\n            {unload}\n        END_IF;\n    ELSE\n        {status}\n    END_IF;\nELSE\n    \
             {c}.EU := FALSE;\n    {status}\nEND_IF;"
        ));
        self.pre_line(&format!("{c}.EU := TRUE; {status}"));
        Ok(())
    }

    fn fll(&mut self, ins: &Instr, t: &Text) -> Res<()> {
        self.arity(ins, 3, t)?;
        let se = self.operand(ins, 0, t)?;
        let de = self.operand(ins, 1, t)?;
        let len = self.val(ins, 2, t)?;
        let stmt = fll_code(self.ctx, &se, &de, &len, t, t.span(ins.span.clone()))?;
        self.temps.insert("lx__i : LINT".into());
        self.temps.insert("lx__n : LINT".into());
        self.when_true(&stmt);
        Ok(())
    }
}

/// DN and EM of a FIFO/LIFO CONTROL from `.POS` and `.LEN` (the status
/// branch of the FFL/FFU/LFL/LFU flow charts).
fn stack_status(c: &str) -> String {
    format!(
        "IF {c}.LEN <= 0 OR {c}.POS < 0 THEN {c}.DN := TRUE; {c}.EM := TRUE; \
         ELSE {c}.DN := {c}.POS >= {c}.LEN; {c}.EM := {c}.POS = 0; END_IF;"
    )
}

/// SWPB(Source, Order Mode, Dest) (1756-RM003 "Swap Byte (SWPB)"): an INT
/// swaps its two bytes whatever the mode (sign-extended into a DINT Dest); a
/// DINT ABCD becomes DCBA (REVERSE), CDAB (WORD) or BADC (HIGH/LOW). No
/// status flags.
pub(crate) fn swpb_code(
    ctx: &Ctx,
    se: &LExpr,
    mode: &str,
    de: &LExpr,
    t: &dyn SpanOf,
    span: Span,
) -> Res<String> {
    let LKind::Path(p) = &se.kind else {
        return Err(L5xError::new("the SWPB Source must be a tag", t.span_of(se.span.clone())));
    };
    let (s, sty) = ctx.path(p, t)?;
    let (d, dty) = ctx.dest(de, t)?;
    let m = mode.to_ascii_uppercase().replace('/', "");
    let v = match (sty.elem(), m.as_str()) {
        (Some(Elem::Int), _) => format!("WORD_TO_INT(ROL(INT_TO_WORD({s}), 8))"),
        (Some(Elem::Dint), "REVERSE") => format!("lx__swpb_reverse({s})"),
        (Some(Elem::Dint), "WORD") => format!("DWORD_TO_DINT(ROL(DINT_TO_DWORD({s}), 16))"),
        (Some(Elem::Dint), "HIGHLOW") => format!("lx__swpb_highlow({s})"),
        (Some(Elem::Dint), _) => {
            return Err(L5xError::new(
                format!("SWPB order mode `{mode}`: expected REVERSE, WORD or HIGH/LOW"),
                span,
            ));
        }
        _ => {
            return Err(L5xError::new(
                format!("SWPB needs an INT or DINT Source, found {}", ctx.env.logix(&sty)),
                span,
            ));
        }
    };
    match (sty.elem(), dty.elem()) {
        (Some(Elem::Int), Some(Elem::Int)) | (Some(Elem::Dint), Some(Elem::Dint)) => Ok(format!("{d} := {v};")),
        (Some(Elem::Int), Some(Elem::Dint)) => Ok(format!("{d} := INT_TO_DINT({v});")),
        _ => Err(L5xError::new(
            format!(
                "SWPB from {} into {}: the Dest must be an INT or DINT at least as wide",
                ctx.env.logix(&sty),
                ctx.env.logix(&dty)
            ),
            span,
        )),
    }
}

/// Split `arr[i]` into (`arr` ST, element type, index ST, length of the
/// dimension); a plain tag is (tag, type, "0", 1).
pub(crate) fn element_base(ctx: &Ctx, e: &LExpr, t: &dyn SpanOf) -> Res<(String, Ty, String, u32)> {
    let LKind::Path(p) = &e.kind else {
        return Err(L5xError::new("expected a tag", t.span_of(e.span.clone())));
    };
    if let Some(Seg::Index(ix, _)) = p.segs.last()
        && ix.len() == 1
    {
        let base = TagPath {
            base: p.base.clone(),
            base_span: p.base_span.clone(),
            segs: p.segs[..p.segs.len() - 1].to_vec(),
        };
        let (st, ty) = ctx.path(&base, t)?;
        if let Ty::Array(el, dims) = ty
            && dims.len() == 1
        {
            let i = ctx.value(&ix[0], t)?;
            return Ok((st, *el, conv(&i, Dom::Int), dims[0]));
        }
    }
    let (st, ty) = ctx.path(p, t)?;
    if let Ty::Array(el, dims) = &ty
        && dims.len() == 1
    {
        return Ok((st, (**el).clone(), "0".into(), dims[0]));
    }
    Ok((st, ty, "0".into(), 1))
}

/// COP / CPS (1756-RM003 "Copy (COP) - Synchronous Copy (CPS)"): copy Length
/// elements of the destination's type from Source to Dest, stopping at the end
/// of either array ("the instruction does not write past the end of the
/// destination"). Supported when source and destination elements have the
/// same type, and between string types (LEN and the characters that fit).
/// Other unlike types are a byte-level reinterpretation in Logix, which
/// plcc's different memory layout cannot reproduce. Uses `lx__i`, `lx__n`.
pub(crate) fn cop_code(
    ctx: &Ctx,
    se: &LExpr,
    de: &LExpr,
    len: &Val,
    t: &dyn SpanOf,
    span: Span,
) -> Res<String> {
    let (sb, sty, si, slen) = element_base(ctx, se, t)?;
    let (db, dty, di, dlen) = element_base(ctx, de, t)?;
    let whole = |b: &str, i: &str, l: u32| -> Option<String> {
        (l == 1 && i == "0").then(|| b.to_string())
    };
    let sidx = |x: &str| {
        whole(&sb, &si, slen).unwrap_or_else(|| format!("{sb}[LINT_TO_DINT({si} + {x})]"))
    };
    let didx = |x: &str| {
        whole(&db, &di, dlen).unwrap_or_else(|| format!("{db}[LINT_TO_DINT({di} + {x})]"))
    };
    let env = ctx.env;
    if sty != dty {
        if crate::scope::is_string(env, &sty) && crate::scope::is_string(env, &dty) {
            // String to string: LEN (clamped to the destination) and DATA.
            let cap = |ty: &Ty| match ty {
                Ty::Struct(i) => env.get(*i).field("DATA").and_then(|f| match &f.ty {
                    Ty::Array(_, d) => d.first().copied(),
                    _ => None,
                }),
                _ => None,
            };
            let (Some(sc), Some(dc)) = (cap(&sty), cap(&dty)) else {
                return Err(L5xError::new("COP between string types", span));
            };
            let (s, d) = (sidx("0"), didx("0"));
            return Ok(format!(
                "lx__n := DINT_TO_LINT({s}.LEN);\n    IF lx__n > {dc} THEN lx__n := {dc}; END_IF;\n    IF lx__n > {sc} THEN lx__n := {sc}; END_IF;\n    IF lx__n < 0 THEN lx__n := 0; END_IF;\n    \
                 {d}.LEN := LINT_TO_DINT(lx__n);\n    FOR lx__i := 0 TO lx__n - 1 DO\n        {d}.DATA[LINT_TO_DINT(lx__i)] := {s}.DATA[LINT_TO_DINT(lx__i)];\n    END_FOR;"
            ));
        }
        // Integer to integer (packing SINTs into a DINT and back, ...): a
        // byte copy, little-endian as on every Logix controller.
        if let (Some(se_), Some(de_)) = (sty.elem(), dty.elem())
            && se_.is_int()
            && de_.is_int()
        {
            let (ss, ds) = (se_.bits() / 8, de_.bits() / 8);
            let wide = |e: crate::types::Elem, x: String| {
                if e == Elem::Lint {
                    x
                } else {
                    format!("{}_TO_LINT({x})", e.st())
                }
            };
            let narrow = |e: crate::types::Elem, x: String| {
                if e == Elem::Lint {
                    x
                } else {
                    format!("LINT_TO_{}({x})", e.st())
                }
            };
            let sel = sidx(&format!("lx__i / {ss}"));
            let del = didx(&format!("lx__i / {ds}"));
            return Ok(format!(
                "lx__n := {len} * {ds};\n    IF lx__n > ({dlen} - {di}) * {ds} THEN lx__n := ({dlen} - {di}) * {ds}; END_IF;\n    \
                 IF lx__n > ({slen} - {si}) * {ss} THEN lx__n := ({slen} - {si}) * {ss}; END_IF;\n    \
                 FOR lx__i := 0 TO lx__n - 1 DO\n        {del} := {set};\n    END_FOR;",
                len = conv(len, Dom::Int),
                set = narrow(
                    de_,
                    format!(
                        "lx__setbyte({}, lx__i MOD {ds}, lx__getbyte({}, lx__i MOD {ss}))",
                        wide(de_, del.clone()),
                        wide(se_, sel)
                    )
                ),
            ));
        }
        return Err(L5xError::new(
            format!(
                "COP from {} to {} (unlike types) is not supported: Logix copies bytes, and plcc lays out data differently",
                env.logix(&sty),
                env.logix(&dty)
            ),
            span,
        ));
    }
    if slen == 1 && dlen == 1 && si == "0" && di == "0" {
        return Ok(format!("{db} := {sb};"));
    }
    Ok(format!(
        "lx__n := {len};\n    IF lx__n > {dlen} - {di} THEN lx__n := {dlen} - {di}; END_IF;\n    IF lx__n > {slen} - {si} THEN lx__n := {slen} - {si}; END_IF;\n    FOR lx__i := 0 TO lx__n - 1 DO\n        {} := {};\n    END_FOR;",
        didx("lx__i"),
        sidx("lx__i"),
        len = conv(len, Dom::Int),
    ))
}

/// FLL (1756-RM003 "File Fill"): Source into Length destination elements,
/// stopping at the end of the array. Uses `lx__i`, `lx__n`.
pub(crate) fn fll_code(
    ctx: &Ctx,
    se: &LExpr,
    de: &LExpr,
    len: &Val,
    t: &dyn SpanOf,
    span: Span,
) -> Res<String> {
    let (db, dty, di, dlen) = element_base(ctx, de, t)?;
    let target = if dlen == 1 && di == "0" {
        db.clone()
    } else {
        format!("{db}[LINT_TO_DINT({di} + lx__i)]")
    };
    let value = match dty.elem() {
        Some(e) => cast(&ctx.value(se, t)?, e),
        None => {
            // A structure fill: the source must be a tag of the same type.
            let LKind::Path(p) = &se.kind else {
                return Err(L5xError::new("FLL of a structure needs a tag source", span));
            };
            let (s, sty) = ctx.path(p, t)?;
            if sty != dty {
                return Err(L5xError::new(
                    format!(
                        "FLL from {} into {}",
                        ctx.env.logix(&sty),
                        ctx.env.logix(&dty)
                    ),
                    span,
                ));
            }
            s
        }
    };
    Ok(format!(
        "lx__n := {len};\n    IF lx__n > {dlen} - {di} THEN lx__n := {dlen} - {di}; END_IF;\n    FOR lx__i := 0 TO lx__n - 1 DO\n        {target} := {value};\n    END_FOR;",
        len = conv(len, Dom::Int),
    ))
}

/// A string tag operand: its ST path and type.
pub(crate) fn string_path(ctx: &Ctx, e: &LExpr, t: &dyn SpanOf) -> Option<(String, Ty)> {
    let LKind::Path(p) = &e.kind else { return None };
    let (s, ty) = ctx.path(p, t).ok()?;
    crate::strings::cap(ctx.env, &ty).map(|_| (s, ty))
}

/// CONCAT, MID, DELETE, INSERT, FIND, UPPER, LOWER, DTOS, STOD with the
/// ladder operand order (also used by ST, which has the same order).
pub(crate) fn string_code(
    ctx: &Ctx,
    h: &crate::strings::Helpers,
    up: &str,
    ops: &[LExpr],
    t: &dyn SpanOf,
    span: Span,
) -> Res<String> {
    let env = ctx.env;
    let need = match up {
        "CONCAT" => 3,
        "MID" | "DELETE" | "INSERT" | "FIND" => 4,
        _ => 2,
    };
    if ops.len() < need {
        return Err(L5xError::new(
            format!("{up} expects {need} operands, found {}", ops.len()),
            span,
        ));
    }
    let sarg = |i: usize| -> Res<(String, Ty)> {
        string_path(ctx, &ops[i], t).ok_or_else(|| {
            L5xError::new(
                format!("{up} needs a string tag here"),
                t.span_of(ops[i].span.clone()),
            )
        })
    };
    let iarg = |i: usize| -> Res<String> { Ok(cast(&ctx.value(&ops[i], t)?, Elem::Dint)) };
    let missing = || L5xError::new(format!("{up} needs string operands"), span);
    Ok(match up {
        "CONCAT" => {
            let ((a, ta), (b, tb), (d, td)) = (sarg(0)?, sarg(1)?, sarg(2)?);
            let f = h.concat(env, &ta, &tb, &td).ok_or_else(missing)?;
            format!("{f}({a}, {b}, {d});")
        }
        "MID" | "DELETE" => {
            let (s, ts) = sarg(0)?;
            let (qty, start) = (iarg(1)?, iarg(2)?);
            let (d, td) = sarg(3)?;
            let f = if up == "MID" {
                h.mid(env, &ts, &td)
            } else {
                h.delete(env, &ts, &td)
            }
            .ok_or_else(missing)?;
            format!("{f}(src := {s}, d := {d}, qty := {qty}, start := {start});")
        }
        "INSERT" => {
            let ((a, ta), (b, tb)) = (sarg(0)?, sarg(1)?);
            let start = iarg(2)?;
            let (d, td) = sarg(3)?;
            let f = h.insert(env, &ta, &tb, &td).ok_or_else(missing)?;
            format!("{f}(sa := {a}, sb := {b}, d := {d}, start := {start});")
        }
        "FIND" => {
            let ((s, ts), (f_, tf)) = (sarg(0)?, sarg(1)?);
            let start = iarg(2)?;
            let (r, rty) = ctx.dest(&ops[3], t)?;
            let f = h.find(env, &ts, &tf).ok_or_else(missing)?;
            let v = Val {
                st: format!("DINT_TO_LINT({f}(src := {s}, search := {f_}, start := {start}))"),
                dom: Dom::Int,
                ty: Ty::Elem(Elem::Dint),
            };
            format!("{r} := {};", cast(&v, rty.elem().unwrap_or(Elem::Dint)))
        }
        "UPPER" | "LOWER" => {
            let ((s, ts), (d, td)) = (sarg(0)?, sarg(1)?);
            let f = h.case(env, &ts, &td, up == "UPPER").ok_or_else(missing)?;
            format!("{f}({s}, {d});")
        }
        "DTOS" => {
            let v = ctx.value(&ops[0], t)?;
            let (d, td) = sarg(1)?;
            let f = h.dtos(env, &td).ok_or_else(missing)?;
            // "If the Source is a REAL, the instruction converts it to a DINT
            // value" (rounding as every REAL → DINT).
            let x = match v.dom {
                Dom::Real | Dom::LReal => format!("DINT_TO_LINT({})", cast(&v, Elem::Dint)),
                _ => conv(&v, Dom::Int),
            };
            format!("{f}(v := {x}, d := {d});")
        }
        "STOD" => {
            let (s, ts) = sarg(0)?;
            let (d, dty) = ctx.dest(&ops[1], t)?;
            let f = h.stod(env, &ts).ok_or_else(missing)?;
            let v = Val {
                st: format!("{f}({s})"),
                dom: Dom::Int,
                ty: Ty::Elem(Elem::Lint),
            };
            store(env, &d, &dty, &v)
                .map_err(|m| L5xError::new(m, t.span_of(ops[1].span.clone())))?
        }
        _ => return Err(L5xError::new(format!("{up} is not supported yet"), span)),
    })
}
