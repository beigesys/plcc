// SPDX-License-Identifier: MPL-2.0

//! IEC ladder model → Structured Text AST.
//!
//! The semantics are those of `docs/ladder.md` (the PLCopen LD lowering),
//! stated on the series/parallel tree:
//!
//! * The left rail is TRUE; power flows left to right through a series.
//!   A contact is `power AND var` (`AND NOT var` normally closed; an edge
//!   contact ANDs the `Q` of a hidden `R_TRIG`/`F_TRIG` on the variable).
//! * A coil writes `var := power` (negated: `NOT power`; set/reset:
//!   `IF power THEN var := TRUE/FALSE`; edge: the power through a hidden edge
//!   detector) and passes the power on.
//! * A branch latches the power at the branch point into a hidden BOOL when
//!   two or more legs read it, so every leg sees the value the rung had there
//!   even if a coil in an earlier leg writes a variable a later leg reads;
//!   the legs' results are ORed where they join.
//! * A function block call is `inst(IN := power, PT := ...)` (power on the
//!   `power_in` pin) or `IF power THEN inst(...) END_IF` (power on `EN`); the
//!   rung continues with `inst.<power_out>` (or the EN value for `ENO`).
//!   Outputs with a value are copied to it after the call. A function is
//!   inlined as an expression (IEC operators as ST operators).
//! * Jumps set a hidden `_ld_jmp`; every rung runs under `IF _ld_jmp = 0`, a
//!   labelled rung clears it, and a backward jump wraps the routine in a loop.
//! * An ST box runs its statements under `IF power THEN ... END_IF`.
//!
//! Hidden variables start with `_ld_` and are named after the element id.

use crate::model::*;
use plcc_st::Span;
use plcc_st::ast::*;
use std::collections::{HashMap, HashSet};

/// A problem with an element of the model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LowerError {
    pub element: Id,
    pub message: String,
}

impl std::fmt::Display for LowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "element {}: {}", self.element, self.message)
    }
}

/// Options for [`lower_routine`].
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Put a `rung N` comment statement before each rung.
    pub annotate: bool,
    /// The routine is a FUNCTION's body: edge elements (which need state) are
    /// errors.
    pub in_function: bool,
    /// Names (any case) already declared by the POU: FB instances not among
    /// them are declared as hidden variables.
    pub declared: HashSet<String>,
}

/// The statements of a routine and the hidden variables they use.
#[derive(Clone, Debug, Default)]
pub struct Lowered {
    pub body: Vec<Statement>,
    pub hidden: Vec<VarDecl>,
    pub errors: Vec<LowerError>,
}

const SP: Span = Span { start: 0, end: 0 };

fn ex(kind: ExpressionKind) -> Expression {
    Expression { kind, span: SP }
}

fn lit(b: bool) -> Expression {
    ex(ExpressionKind::BoolLiteral(b))
}

fn is_true(e: &Expression) -> bool {
    matches!(e.kind, ExpressionKind::BoolLiteral(true))
}

fn ident(name: &str) -> Expression {
    ex(ExpressionKind::Identifier(Ident::new(name, SP)))
}

fn member(obj: &str, m: &str) -> Expression {
    ex(ExpressionKind::MemberAccess {
        object: Box::new(ident(obj)),
        member: Ident::new(m, SP),
    })
}

fn binary(op: BinaryOp, l: Expression, r: Expression) -> Expression {
    ex(ExpressionKind::BinaryOp {
        op,
        left: Box::new(l),
        right: Box::new(r),
    })
}

fn and(l: Expression, r: Expression) -> Expression {
    if is_true(&l) {
        r
    } else if is_true(&r) {
        l
    } else {
        binary(BinaryOp::And, l, r)
    }
}

fn or(l: Expression, r: Expression) -> Expression {
    if is_true(&l) || is_true(&r) {
        lit(true)
    } else {
        binary(BinaryOp::Or, l, r)
    }
}

fn not(e: Expression) -> Expression {
    ex(ExpressionKind::UnaryOp {
        op: UnaryOp::Not,
        operand: Box::new(e),
    })
}

fn stmt(kind: StatementKind) -> Statement {
    Statement { kind, span: SP }
}

fn assign(target: Expression, value: Expression) -> Statement {
    stmt(StatementKind::Assignment { target, value })
}

fn guarded(cond: Expression, body: Vec<Statement>) -> Vec<Statement> {
    if is_true(&cond) {
        return body;
    }
    vec![stmt(StatementKind::If {
        condition: cond,
        then_body: body,
        elsif_branches: Vec::new(),
        else_body: None,
    })]
}

fn int(v: i128) -> Expression {
    ex(ExpressionKind::IntegerLiteral(v))
}

fn call_arg(name: &str, value: Expression) -> CallArg {
    CallArg {
        name: Some(Ident::new(name, SP)),
        value,
        is_output: false,
        negated: false,
        span: SP,
    }
}

pub(crate) fn var_decl(name: &str, ty: &str) -> VarDecl {
    VarDecl {
        init_args: Vec::new(),
        name: Ident::new(name, SP),
        type_spec: TypeSpec {
            kind: TypeSpecKind::Named(Ident::new(ty, SP)),
            span: SP,
        },
        at_address: None,
        edge: None,
        initializer: None,
        span: SP,
    }
}

struct L<'o> {
    opts: &'o Options,
    out: Vec<Statement>,
    hidden: Vec<VarDecl>,
    declared: HashSet<String>,
    errors: Vec<LowerError>,
    labels: HashMap<String, i128>,
}

/// Lower an IEC routine.
pub fn lower_routine(routine: &Routine, opts: &Options) -> Lowered {
    let mut l = L {
        opts,
        out: Vec::new(),
        hidden: Vec::new(),
        declared: opts
            .declared
            .iter()
            .map(|n| n.to_ascii_uppercase())
            .collect(),
        errors: Vec::new(),
        labels: HashMap::new(),
    };
    let body = l.routine(routine);
    Lowered {
        body,
        hidden: l.hidden,
        errors: l.errors,
    }
}

impl L<'_> {
    fn err(&mut self, element: Id, message: impl Into<String>) {
        self.errors.push(LowerError {
            element,
            message: message.into(),
        });
    }

    fn expr(&mut self, id: Id, text: &str) -> Option<Expression> {
        let (e, errs) = plcc_st::parse_expression(text);
        if let Some(first) = errs.first() {
            self.err(id, format!("`{text}`: {first}"));
            return None;
        }
        Some(strip_spans_expr(e))
    }

    fn hide(&mut self, name: &str, ty: &str) {
        if self.declared.insert(name.to_ascii_uppercase()) {
            self.hidden.push(var_decl(name, ty));
        }
    }

    fn latch(&mut self, name: String, v: Expression) -> Expression {
        if matches!(v.kind, ExpressionKind::BoolLiteral(_)) {
            return v;
        }
        self.hide(&name, "BOOL");
        self.out.push(assign(ident(&name), v));
        ident(&name)
    }

    fn trig(&mut self, id: Id, rising: bool, clk: Expression, suffix: &str) -> Option<Expression> {
        if self.opts.in_function {
            self.err(
                id,
                "edge detection needs state, which a FUNCTION does not have",
            );
            return None;
        }
        let (name, ty) = if rising {
            (format!("_ld_rt{id}{suffix}"), "R_TRIG")
        } else {
            (format!("_ld_ft{id}{suffix}"), "F_TRIG")
        };
        self.hide(&name, ty);
        self.out.push(stmt(StatementKind::FunctionCall {
            callee: ident(&name),
            args: vec![call_arg("CLK", clk)],
        }));
        Some(member(&name, "Q"))
    }

    fn routine(&mut self, r: &Routine) -> Vec<Statement> {
        // Labels are numbered in rung order.
        let mut label_rung = HashMap::new();
        for (ri, g) in r.rungs.iter().enumerate() {
            if let Some(lb) = &g.label {
                let k = self.labels.len() as i128 + 1;
                let key = lb.to_ascii_uppercase();
                if self.labels.insert(key.clone(), k).is_some() {
                    self.err(g.id, format!("label `{lb}` is defined twice"));
                }
                label_rung.insert(key, ri);
            }
        }
        let mut has_jumps = false;
        let mut backward = false;
        for (ri, g) in r.rungs.iter().enumerate() {
            walk(&g.elements, &mut |e| {
                if let Element::Jump(j) = e {
                    has_jumps = true;
                    if let Some(&li) = label_rung.get(&j.label.to_ascii_uppercase()) {
                        backward |= li <= ri;
                    }
                }
            });
        }
        for g in &r.rungs {
            let mut missing = Vec::new();
            walk(&g.elements, &mut |e| {
                if let Element::Jump(j) = e
                    && !self.labels.contains_key(&j.label.to_ascii_uppercase())
                {
                    missing.push((j.id, j.label.clone()));
                }
            });
            for (id, lb) in missing {
                self.err(id, format!("jump to undefined label `{lb}`"));
            }
        }
        let jmp = || ident("_ld_jmp");
        let mut body = Vec::new();
        if has_jumps {
            self.hide("_ld_jmp", "DINT");
            body.push(assign(jmp(), int(0)));
        }
        let mut pass = Vec::new();
        for (ri, g) in r.rungs.iter().enumerate() {
            if self.opts.annotate {
                let mut text = format!("rung {}", ri + 1);
                if let Some(c) = &g.comment {
                    text.push_str(": ");
                    text.push_str(c);
                }
                pass.push(stmt(StatementKind::Comment(text)));
            }
            if let Some(lb) = &g.label {
                let k = self.labels[&lb.to_ascii_uppercase()];
                pass.extend(guarded(
                    binary(BinaryOp::Equal, jmp(), int(k)),
                    vec![assign(jmp(), int(0))],
                ));
            }
            self.series(&g.elements, lit(true));
            let stmts = std::mem::take(&mut self.out);
            if stmts.is_empty() {
                continue;
            }
            if has_jumps {
                pass.extend(guarded(binary(BinaryOp::Equal, jmp(), int(0)), stmts));
            } else {
                pass.extend(stmts);
            }
        }
        if backward {
            pass.extend(guarded(
                binary(BinaryOp::Equal, jmp(), int(0)),
                vec![stmt(StatementKind::Exit)],
            ));
            body.push(stmt(StatementKind::While {
                condition: lit(true),
                body: pass,
            }));
        } else {
            body.extend(pass);
        }
        body
    }

    fn series(&mut self, elems: &[Element], mut p: Expression) -> Expression {
        for e in elems {
            p = self.element(e, p);
        }
        p
    }

    fn element(&mut self, e: &Element, p: Expression) -> Expression {
        match e {
            Element::Contact(c) => {
                let Some(var) = self.expr(c.id, &c.operand) else {
                    return p;
                };
                let term = match c.kind {
                    ContactKind::No => var,
                    ContactKind::Nc => not(var),
                    ContactKind::Rising | ContactKind::Falling => {
                        match self.trig(c.id, c.kind == ContactKind::Rising, var, "") {
                            Some(q) => q,
                            None => return p,
                        }
                    }
                };
                and(p, term)
            }
            Element::Coil(c) => {
                let Some(var) = self.expr(c.id, &c.operand) else {
                    return p;
                };
                let v = match c.kind {
                    CoilKind::Rising | CoilKind::Falling => {
                        match self.trig(c.id, c.kind == CoilKind::Rising, p.clone(), "") {
                            Some(q) => q,
                            None => return p,
                        }
                    }
                    _ => p.clone(),
                };
                match c.kind {
                    CoilKind::Set | CoilKind::Reset => {
                        let body = vec![assign(var, lit(c.kind == CoilKind::Set))];
                        let g = guarded(v, body);
                        self.out.extend(g);
                    }
                    CoilKind::Negated => self.out.push(assign(var, not(v))),
                    _ => self.out.push(assign(var, v)),
                }
                p
            }
            Element::Branch(b) if b.legs.iter().all(|l| pure(l)) => {
                // Only contacts: nothing in a leg writes, so the branch is
                // `p AND (leg OR leg ...)` and needs no latch.
                let mut acc: Option<Expression> = None;
                for leg in &b.legs {
                    let r = self.series(leg, lit(true));
                    acc = Some(match acc {
                        None => r,
                        Some(a) => or(a, r),
                    });
                }
                and(p, acc.unwrap_or_else(|| lit(true)))
            }
            Element::Branch(b) => {
                let readers = b.legs.len();
                let at = if readers > 1 {
                    self.latch(format!("_ld_p{}", b.id), p)
                } else {
                    p
                };
                let mut acc: Option<Expression> = None;
                for leg in &b.legs {
                    let r = self.series(leg, at.clone());
                    acc = Some(match acc {
                        None => r,
                        Some(a) => or(a, r),
                    });
                }
                acc.unwrap_or(at)
            }
            Element::Block(b) => self.block(b, p),
            Element::Jump(j) => {
                let k = self
                    .labels
                    .get(&j.label.to_ascii_uppercase())
                    .copied()
                    .unwrap_or(0);
                let g = guarded(p.clone(), vec![assign(ident("_ld_jmp"), int(k))]);
                self.out.extend(g);
                p
            }
            Element::Return(_) => {
                let g = guarded(p.clone(), vec![stmt(StatementKind::Return { value: None })]);
                self.out.extend(g);
                p
            }
            Element::St(s) => {
                let (stmts, errs) = plcc_st::parse_statements(&s.code);
                if let Some(first) = errs.first() {
                    self.err(s.id, format!("ST box: {first}"));
                    return p;
                }
                let stmts = stmts.into_iter().map(strip_spans_stmt).collect();
                let g = guarded(p.clone(), stmts);
                self.out.extend(g);
                p
            }
        }
    }

    /// The value arriving at an input pin (not the power pin).
    fn pin_input(&mut self, b: &Block, pin: &Pin) -> Option<Expression> {
        let v = if let Some(r) = &pin.rung {
            Some(self.series(r, lit(true)))
        } else if let Some(t) = &pin.value {
            self.expr(b.id, t)
        } else {
            None
        }?;
        Some(if pin.negated { not(v) } else { v })
    }

    fn block(&mut self, b: &Block, p: Expression) -> Expression {
        let is = |pin: &Pin, name: &Option<String>| {
            name.as_deref()
                .is_some_and(|n| pin.name.eq_ignore_ascii_case(n))
        };
        let en_pin = b
            .power_in
            .as_deref()
            .is_some_and(|n| n.eq_ignore_ascii_case("EN"));
        let mut en: Option<Expression> = None;
        // Inputs, in pin order.
        let mut args: Vec<(String, Expression)> = Vec::new();
        let mut all_inputs = true;
        for pin in b.pins.iter().filter(|p| p.dir != PinDir::Output) {
            if pin.name.eq_ignore_ascii_case("EN") {
                if is(pin, &b.power_in) {
                    en = Some(if pin.negated {
                        not(p.clone())
                    } else {
                        p.clone()
                    });
                } else {
                    en = self.pin_input(b, pin);
                }
                continue;
            }
            let v = if is(pin, &b.power_in) {
                Some(if pin.negated {
                    not(p.clone())
                } else {
                    p.clone()
                })
            } else {
                self.pin_input(b, pin)
            };
            match v {
                Some(v) => args.push((pin.name.clone(), v)),
                None => all_inputs = false,
            }
        }
        if en.is_none() && en_pin {
            en = Some(p.clone());
        }
        let eno_used = b
            .power_out
            .as_deref()
            .is_some_and(|n| n.eq_ignore_ascii_case("ENO"))
            || b.pins.iter().any(|q| {
                q.name.eq_ignore_ascii_case("ENO") && (q.value.is_some() || q.rung.is_some())
            });
        let outputs: Vec<&Pin> = b.pins.iter().filter(|p| p.dir == PinDir::Output).collect();
        let power_after = |this: &mut Self, v: Expression| -> Expression {
            if b.power_in.is_none() {
                and(p.clone(), v)
            } else {
                let _ = this;
                v
            }
        };
        match &b.instance {
            Some(inst) => {
                if !self.declared.contains(&inst.to_ascii_uppercase()) {
                    self.declared.insert(inst.to_ascii_uppercase());
                    self.hidden.push(var_decl(inst, &b.name));
                }
                let call = stmt(StatementKind::FunctionCall {
                    callee: ident(inst),
                    args: args.into_iter().map(|(n, v)| call_arg(&n, v)).collect(),
                });
                let eno = match en {
                    Some(en) => {
                        let en = if eno_used {
                            self.latch(format!("_ld_en{}", b.id), en)
                        } else {
                            en
                        };
                        let g = guarded(en.clone(), vec![call]);
                        self.out.extend(g);
                        en
                    }
                    None => {
                        self.out.push(call);
                        lit(true)
                    }
                };
                let value_of = |pin: &Pin| -> Expression {
                    let v = if pin.name.eq_ignore_ascii_case("ENO") {
                        eno.clone()
                    } else {
                        member(inst, &pin.name)
                    };
                    if pin.negated { not(v) } else { v }
                };
                for pin in &outputs {
                    let v = value_of(pin);
                    if let Some(t) = &pin.value
                        && let Some(target) = self.expr(b.id, t)
                    {
                        self.out.push(assign(target, v.clone()));
                    }
                    if let Some(r) = &pin.rung {
                        self.series(r, v);
                    }
                }
                match &b.power_out {
                    None => p,
                    Some(po) => {
                        let v = outputs
                            .iter()
                            .find(|q| q.name.eq_ignore_ascii_case(po))
                            .map(|q| value_of(q))
                            .unwrap_or_else(|| {
                                if po.eq_ignore_ascii_case("ENO") {
                                    eno.clone()
                                } else {
                                    member(inst, po)
                                }
                            });
                        power_after(self, v)
                    }
                }
            }
            None => {
                let Some(f) = self.function(b, args, all_inputs) else {
                    return p;
                };
                let eno = en.clone().unwrap_or_else(|| lit(true));
                let result_pin = outputs
                    .iter()
                    .find(|q| !q.name.eq_ignore_ascii_case("ENO"))
                    .map(|q| q.name.clone());
                let value_of = |pin: &Pin| -> Expression {
                    let v = if pin.name.eq_ignore_ascii_case("ENO") {
                        eno.clone()
                    } else {
                        f.clone()
                    };
                    if pin.negated { not(v) } else { v }
                };
                let mut consumed = false;
                for pin in &outputs {
                    let v = value_of(pin);
                    if let Some(t) = &pin.value
                        && let Some(target) = self.expr(b.id, t)
                    {
                        consumed |= !pin.name.eq_ignore_ascii_case("ENO");
                        let s = assign(target, v.clone());
                        // An output written only while EN is TRUE keeps its
                        // value otherwise (IEC 61131-3).
                        let g = match (&en, pin.name.eq_ignore_ascii_case("ENO")) {
                            (Some(e), false) => guarded(e.clone(), vec![s]),
                            _ => vec![s],
                        };
                        self.out.extend(g);
                    }
                    if let Some(r) = &pin.rung {
                        consumed |= !pin.name.eq_ignore_ascii_case("ENO");
                        self.series(r, v);
                    }
                }
                let continues_with_result = b
                    .power_out
                    .as_deref()
                    .is_some_and(|po| !po.eq_ignore_ascii_case("ENO"));
                if !consumed
                    && !continues_with_result
                    && let ExpressionKind::FunctionCall { callee, args } = &f.kind
                {
                    // Called for its effects.
                    let s = stmt(StatementKind::FunctionCall {
                        callee: (**callee).clone(),
                        args: args.clone(),
                    });
                    let g = guarded(eno.clone(), vec![s]);
                    self.out.extend(g);
                }
                match &b.power_out {
                    None => p,
                    Some(po) if po.eq_ignore_ascii_case("ENO") => power_after(self, eno),
                    Some(po) => {
                        let neg = outputs
                            .iter()
                            .any(|q| q.name.eq_ignore_ascii_case(po) && q.negated);
                        let _ = &result_pin;
                        let v = if neg { not(f) } else { f };
                        power_after(self, v)
                    }
                }
            }
        }
    }

    /// A function block as an expression: IEC operators become ST operators,
    /// anything else a call (positional when every input is connected).
    fn function(
        &mut self,
        b: &Block,
        args: Vec<(String, Expression)>,
        positional: bool,
    ) -> Option<Expression> {
        let up = b.name.to_ascii_uppercase();
        let vals: Vec<Expression> = args.iter().map(|(_, v)| v.clone()).collect();
        let op = match up.as_str() {
            "ADD" => Some((BinaryOp::Add, true)),
            "MUL" => Some((BinaryOp::Mul, true)),
            "AND" => Some((BinaryOp::And, true)),
            "OR" => Some((BinaryOp::Or, true)),
            "XOR" => Some((BinaryOp::Xor, true)),
            "SUB" => Some((BinaryOp::Sub, false)),
            "DIV" => Some((BinaryOp::Div, false)),
            "MOD" => Some((BinaryOp::Mod, false)),
            "EXPT" => Some((BinaryOp::Power, false)),
            _ => None,
        };
        let cmp = match up.as_str() {
            "GT" => Some(BinaryOp::Greater),
            "GE" => Some(BinaryOp::GreaterEqual),
            "EQ" => Some(BinaryOp::Equal),
            "LE" => Some(BinaryOp::LessEqual),
            "LT" => Some(BinaryOp::Less),
            "NE" => Some(BinaryOp::NotEqual),
            _ => None,
        };
        if let Some((bop, extensible)) = op {
            let ok = if extensible {
                vals.len() >= 2
            } else {
                vals.len() == 2
            };
            if !ok || !positional {
                self.err(b.id, format!("{} needs its inputs connected", b.name));
                return None;
            }
            return vals.into_iter().reduce(|l, r| binary(bop, l, r));
        }
        if let Some(c) = cmp {
            let ok = if c == BinaryOp::NotEqual {
                vals.len() == 2
            } else {
                vals.len() >= 2
            };
            if !ok || !positional {
                self.err(b.id, format!("{} needs its inputs connected", b.name));
                return None;
            }
            return vals
                .windows(2)
                .map(|w| binary(c, w[0].clone(), w[1].clone()))
                .reduce(|l, r| binary(BinaryOp::And, l, r));
        }
        if up == "NOT" || up == "MOVE" {
            if vals.len() != 1 {
                self.err(b.id, format!("{} needs exactly 1 connected input", b.name));
                return None;
            }
            let v = vals.into_iter().next()?;
            return Some(if up == "NOT" { not(v) } else { v });
        }
        let call_args = args
            .into_iter()
            .map(|(n, v)| CallArg {
                name: (!positional).then(|| Ident::new(n, SP)),
                value: v,
                is_output: false,
                negated: false,
                span: SP,
            })
            .collect();
        Some(ex(ExpressionKind::FunctionCall {
            callee: Box::new(ident(&b.name)),
            args: call_args,
        }))
    }
}

/// A series of contacts only (nested branches of contacts included): it
/// reads, it writes nothing.
fn pure(series: &[Element]) -> bool {
    series.iter().all(|e| match e {
        Element::Contact(_) => true,
        Element::Branch(b) => b.legs.iter().all(|l| pure(l)),
        _ => false,
    })
}

fn strip_spans_expr(e: Expression) -> Expression {
    zero(e)
}

fn strip_spans_stmt(s: Statement) -> Statement {
    zero(s)
}

/// Every span set to 0..0: operand text has no place in the file the
/// statements end up in.
fn zero<T: serde::Serialize + serde::de::DeserializeOwned>(v: T) -> T {
    fn walk(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(map) => {
                if map.len() == 2 && map.contains_key("start") && map.contains_key("end") {
                    map.insert("start".into(), 0.into());
                    map.insert("end".into(), 0.into());
                    return;
                }
                for (_, c) in map.iter_mut() {
                    walk(c);
                }
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(walk),
            _ => {}
        }
    }
    let Ok(mut j) = serde_json::to_value(&v) else {
        return v;
    };
    walk(&mut j);
    serde_json::from_value(j).unwrap_or(v)
}
