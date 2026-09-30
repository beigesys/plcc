// SPDX-License-Identifier: MPL-2.0

//! LD and FBD bodies → ST statements.
//!
//! A graphical body is a flat list of elements wired by `connectionPointIn` /
//! `<connection refLocalId=..>`. Lowering:
//!
//! 1. **Networks.** Elements joined by connections (and connector/continuation
//!    pairs of the same name) form one network (rung). Networks run in
//!    `executionOrderId` order when every network carries one, otherwise top to
//!    bottom, left to right by `<position>`.
//! 2. **Sinks.** Inside a network the elements with an effect — coils, output
//!    variables, FB calls, jumps, returns — run in execution order (or position
//!    order). Each sink *pulls* the value of its inputs; anything a value needs
//!    first (an FB call whose output it reads, an edge detector) is emitted just
//!    before it, so the result is always a valid topological order.
//! 3. **Power flow.** Series = AND, several connections into one input
//!    (parallel branches) = OR, the left rail is TRUE. A contact is
//!    `power AND var` (`AND NOT var` negated); a coil passes its input on.
//! 4. **Hidden state.** Edge contacts and coils use hidden `R_TRIG` / `F_TRIG`
//!    instances; a power-flow value used by more than one element is latched into
//!    a hidden BOOL so every branch sees the same value; jumps use a hidden
//!    `_ld_jmp`. All are extra VARs of the POU, prefixed `_ld_`.
//!
//! Every generated statement and expression carries the span of the XML element
//! it came from.

use crate::error::PlcOpenError;
use crate::project::{Lower, PouKind};
use crate::xml::{self, XNode};
use plcc_st::Span;
use plcc_st::ast::*;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Edge {
    None,
    Rising,
    Falling,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Storage {
    None,
    Set,
    Reset,
}

#[derive(Debug)]
pub(crate) struct Conn {
    pub ref_id: u32,
    pub formal: Option<String>,
    pub span: Span,
}

#[derive(Debug)]
pub(crate) struct PinIn {
    pub formal: String,
    pub conns: Vec<Conn>,
    /// `connectionPointIn` may carry an `<expression>` instead of connections.
    pub expr: Option<Expression>,
    pub negated: bool,
    pub edge: Edge,
    pub span: Span,
}

#[derive(Debug)]
pub(crate) struct PinOut {
    pub formal: String,
    pub negated: bool,
}

#[derive(Debug)]
pub(crate) enum Kind {
    LeftRail,
    RightRail,
    Contact {
        var: Expression,
        negated: bool,
        edge: Edge,
    },
    Coil {
        var: Expression,
        negated: bool,
        storage: Storage,
        edge: Edge,
    },
    Block {
        type_name: Ident,
        instance: Option<Ident>,
        inouts: HashSet<String>,
    },
    InVar {
        expr: Expression,
        negated: bool,
    },
    OutVar {
        expr: Expression,
        negated: bool,
    },
    InOutVar {
        expr: Expression,
    },
    Connector {
        name: String,
    },
    Continuation {
        name: String,
    },
    Jump {
        label: String,
    },
    Label {
        label: String,
    },
    Return,
}

#[derive(Debug)]
pub(crate) struct Elem {
    pub id: u32,
    pub kind: Kind,
    /// Span of the element's start tag.
    pub span: Span,
    pub pos: (f64, f64),
    pub eo: Option<u32>,
    pub ins: Vec<PinIn>,
    pub outs: Vec<PinOut>,
}

impl Elem {
    fn is_sink(&self) -> bool {
        matches!(
            self.kind,
            Kind::Coil { .. }
                | Kind::OutVar { .. }
                | Kind::InOutVar { .. }
                | Kind::Block { .. }
                | Kind::Jump { .. }
                | Kind::Return
        )
    }

    fn is_fb(&self) -> bool {
        matches!(
            &self.kind,
            Kind::Block {
                instance: Some(_),
                ..
            }
        )
    }

    pub(crate) fn pin(&self, formal: &str) -> Option<usize> {
        self.ins
            .iter()
            .position(|p| p.formal.eq_ignore_ascii_case(formal))
    }

    pub(crate) fn label(&self) -> String {
        let what = match &self.kind {
            Kind::LeftRail => "left power rail".to_string(),
            Kind::RightRail => "right power rail".to_string(),
            Kind::Contact { .. } => "contact".to_string(),
            Kind::Coil { .. } => "coil".to_string(),
            Kind::Block { type_name, .. } => format!("block {}", type_name.name),
            Kind::InVar { .. } => "input variable".to_string(),
            Kind::OutVar { .. } => "output variable".to_string(),
            Kind::InOutVar { .. } => "in-out variable".to_string(),
            Kind::Connector { name } => format!("connector {name}"),
            Kind::Continuation { name } => format!("continuation {name}"),
            Kind::Jump { label } => format!("jump to {label}"),
            Kind::Label { label } => format!("label {label}"),
            Kind::Return => "return".to_string(),
        };
        format!("{what} (localId {})", self.id)
    }
}

// ── Expression / statement builders ──

fn ex(kind: ExpressionKind, span: Span) -> Expression {
    Expression { kind, span }
}

fn lit_bool(b: bool, span: Span) -> Expression {
    ex(ExpressionKind::BoolLiteral(b), span)
}

fn is_true(e: &Expression) -> bool {
    matches!(e.kind, ExpressionKind::BoolLiteral(true))
}

fn ident(name: &str, span: Span) -> Expression {
    ex(ExpressionKind::Identifier(Ident::new(name, span)), span)
}

fn member(obj: &str, m: &str, span: Span) -> Expression {
    ex(
        ExpressionKind::MemberAccess {
            object: Box::new(ident(obj, span)),
            member: Ident::new(m, span),
        },
        span,
    )
}

fn binary(op: BinaryOp, l: Expression, r: Expression, span: Span) -> Expression {
    ex(
        ExpressionKind::BinaryOp {
            op,
            left: Box::new(l),
            right: Box::new(r),
        },
        span,
    )
}

fn and(l: Expression, r: Expression, span: Span) -> Expression {
    if is_true(&l) {
        r
    } else if is_true(&r) {
        l
    } else {
        binary(BinaryOp::And, l, r, span)
    }
}

fn or(l: Expression, r: Expression, span: Span) -> Expression {
    if is_true(&l) || is_true(&r) {
        lit_bool(true, span)
    } else {
        binary(BinaryOp::Or, l, r, span)
    }
}

fn not(e: Expression, span: Span) -> Expression {
    ex(
        ExpressionKind::UnaryOp {
            op: UnaryOp::Not,
            operand: Box::new(e),
        },
        span,
    )
}

fn stmt(kind: StatementKind, span: Span) -> Statement {
    Statement { kind, span }
}

fn assign(target: Expression, value: Expression, span: Span) -> Statement {
    stmt(StatementKind::Assignment { target, value }, span)
}

/// `IF cond THEN body END_IF`, or just `body` when `cond` is literally TRUE.
fn guarded(cond: Expression, body: Vec<Statement>, span: Span) -> Vec<Statement> {
    if is_true(&cond) {
        return body;
    }
    vec![stmt(
        StatementKind::If {
            condition: cond,
            then_body: body,
            elsif_branches: Vec::new(),
            else_body: None,
        },
        span,
    )]
}

fn call_arg(name: &str, value: Expression, span: Span) -> CallArg {
    CallArg {
        name: Some(Ident::new(name, span)),
        value,
        is_output: false,
        negated: false,
        span,
    }
}

fn var_decl(name: &str, ty: &str, span: Span) -> VarDecl {
    VarDecl {
        init_args: Vec::new(),
        name: Ident::new(name, span),
        type_spec: TypeSpec {
            kind: TypeSpecKind::Named(Ident::new(ty, span)),
            span,
        },
        at_address: None,
        edge: None,
        initializer: None,
        span,
    }
}

// ── Reading the XML ──

fn parse_edge(n: XNode, lower: &mut Lower) -> Edge {
    match xml::attr(n, "edge").map(str::trim) {
        None | Some("none") | Some("") => Edge::None,
        Some("rising") => Edge::Rising,
        Some("falling") => Edge::Falling,
        Some(other) => {
            lower.err(
                format!("unknown edge {other:?} (expected none, rising or falling)"),
                xml::attr_span(lower.src, n, "edge"),
            );
            Edge::None
        }
    }
}

fn parse_storage(n: XNode, lower: &mut Lower) -> Storage {
    match xml::attr(n, "storage").map(str::trim) {
        None | Some("none") | Some("") => Storage::None,
        Some("set") => Storage::Set,
        Some("reset") => Storage::Reset,
        Some(other) => {
            lower.err(
                format!("unknown coil storage {other:?} (expected none, set or reset)"),
                xml::attr_span(lower.src, n, "storage"),
            );
            Storage::None
        }
    }
}

fn parse_pin(lower: &mut Lower, cpi: Option<XNode>, formal: &str, owner: XNode) -> PinIn {
    let mut conns = Vec::new();
    let mut expr = None;
    let span = cpi.map_or_else(
        || xml::tag_span(lower.src, owner),
        |c| xml::tag_span(lower.src, c),
    );
    if let Some(cpi) = cpi {
        for c in xml::children(cpi, "connection") {
            let cspan = xml::tag_span(lower.src, c);
            match xml::attr(c, "refLocalId").and_then(|v| v.trim().parse().ok()) {
                Some(ref_id) => conns.push(Conn {
                    ref_id,
                    formal: xml::attr(c, "formalParameter")
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                    span: cspan,
                }),
                None => lower.err("<connection> without a valid refLocalId", cspan),
            }
        }
        if let Some(e) = xml::child(cpi, "expression") {
            expr = lower.expr_content(e, "connection expression");
        }
    }
    PinIn {
        formal: formal.to_string(),
        conns,
        expr,
        negated: false,
        edge: Edge::None,
        span,
    }
}

fn required_expr(lower: &mut Lower, n: XNode, child: &str, what: &str) -> Option<Expression> {
    match xml::child(n, child) {
        Some(c) => lower.expr_content(c, what),
        None => {
            lower.err(
                format!("<{}> has no <{child}>", xml::name(n)),
                xml::tag_span(lower.src, n),
            );
            None
        }
    }
}

fn required_attr(lower: &mut Lower, n: XNode, a: &str) -> Option<String> {
    match xml::attr(n, a).map(str::trim).filter(|s| !s.is_empty()) {
        Some(v) => Some(v.to_string()),
        None => {
            lower.err(
                format!("<{}> is missing its `{a}` attribute", xml::name(n)),
                xml::tag_span(lower.src, n),
            );
            None
        }
    }
}

const SFC_ELEMENTS: &[&str] = &[
    "step",
    "macroStep",
    "transition",
    "selectionDivergence",
    "selectionConvergence",
    "simultaneousDivergence",
    "simultaneousConvergence",
    "actionBlock",
];

pub(crate) fn parse_elem(lower: &mut Lower, n: XNode) -> Option<Elem> {
    let tag = xml::name(n);
    if matches!(tag, "comment" | "documentation" | "addData" | "error") {
        return None;
    }
    let span = xml::tag_span(lower.src, n);
    if SFC_ELEMENTS.contains(&tag) {
        lower.errors.push(
            PlcOpenError::new(format!("SFC element <{tag}> is not supported yet"), span)
                .with_help("plcc supports ST, LD and FBD bodies; SFC is planned"),
        );
        return None;
    }
    let id: u32 = match xml::attr(n, "localId").and_then(|v| v.trim().parse().ok()) {
        Some(id) => id,
        None => {
            lower.err(format!("<{tag}> has no valid localId"), span);
            return None;
        }
    };
    let eo = xml::attr(n, "executionOrderId")
        .and_then(|v| v.trim().parse().ok())
        .filter(|v: &u32| *v > 0);
    let mut ins = Vec::new();
    let mut outs = Vec::new();
    let single_in = |lower: &mut Lower| parse_pin(lower, xml::child(n, "connectionPointIn"), "", n);
    let kind = match tag {
        "leftPowerRail" => Kind::LeftRail,
        "rightPowerRail" => Kind::RightRail,
        "contact" => {
            let var = required_expr(lower, n, "variable", "contact variable")?;
            ins.push(single_in(lower));
            Kind::Contact {
                var,
                negated: xml::attr_bool(n, "negated"),
                edge: parse_edge(n, lower),
            }
        }
        "coil" => {
            let var = required_expr(lower, n, "variable", "coil variable")?;
            ins.push(single_in(lower));
            Kind::Coil {
                var,
                negated: xml::attr_bool(n, "negated"),
                storage: parse_storage(n, lower),
                edge: parse_edge(n, lower),
            }
        }
        "block" => {
            let type_name = Ident::new(
                required_attr(lower, n, "typeName")?,
                xml::attr_span(lower.src, n, "typeName"),
            );
            let instance = xml::attr(n, "instanceName")
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| Ident::new(s, xml::attr_span(lower.src, n, "instanceName")));
            let mut inouts = HashSet::new();
            for (section, is_inout) in [("inputVariables", false), ("inOutVariables", true)] {
                if let Some(sec) = xml::child(n, section) {
                    for v in xml::children(sec, "variable") {
                        let Some(formal) = required_attr(lower, v, "formalParameter") else {
                            continue;
                        };
                        let mut pin =
                            parse_pin(lower, xml::child(v, "connectionPointIn"), &formal, v);
                        pin.negated = xml::attr_bool(v, "negated");
                        pin.edge = parse_edge(v, lower);
                        if is_inout {
                            inouts.insert(formal.to_ascii_uppercase());
                        }
                        ins.push(pin);
                    }
                }
            }
            if let Some(sec) = xml::child(n, "outputVariables") {
                for v in xml::children(sec, "variable") {
                    let Some(formal) = required_attr(lower, v, "formalParameter") else {
                        continue;
                    };
                    outs.push(PinOut {
                        formal,
                        negated: xml::attr_bool(v, "negated"),
                    });
                }
            }
            Kind::Block {
                type_name,
                instance,
                inouts,
            }
        }
        "inVariable" => Kind::InVar {
            expr: required_expr(lower, n, "expression", "input variable")?,
            negated: xml::attr_bool(n, "negated"),
        },
        "outVariable" => {
            let expr = required_expr(lower, n, "expression", "output variable")?;
            ins.push(single_in(lower));
            Kind::OutVar {
                expr,
                negated: xml::attr_bool(n, "negated"),
            }
        }
        "inOutVariable" => {
            let expr = required_expr(lower, n, "expression", "in-out variable")?;
            ins.push(single_in(lower));
            Kind::InOutVar { expr }
        }
        "connector" => {
            ins.push(single_in(lower));
            Kind::Connector {
                name: required_attr(lower, n, "name")?,
            }
        }
        "continuation" => Kind::Continuation {
            name: required_attr(lower, n, "name")?,
        },
        "jump" => {
            ins.push(single_in(lower));
            Kind::Jump {
                label: required_attr(lower, n, "label")?,
            }
        }
        "label" => Kind::Label {
            label: required_attr(lower, n, "label")?,
        },
        "return" => {
            ins.push(single_in(lower));
            Kind::Return
        }
        other => {
            lower.errors.push(PlcOpenError::new(
                format!("unsupported graphical element <{other}>"),
                span,
            ));
            return None;
        }
    };
    if matches!(kind, Kind::RightRail) {
        // Its inputs only terminate the rungs; keep them for network grouping.
        for cpi in xml::children(n, "connectionPointIn") {
            ins.push(parse_pin(lower, Some(cpi), "", n));
        }
    }
    Some(Elem {
        id,
        kind,
        span,
        pos: xml::position(n),
        eo,
        ins,
        outs,
    })
}

// ── Lowering ──

pub(crate) struct Graph<'l, 's> {
    lower: &'l mut Lower<'s>,
    pou: PouKind,
    pub elems: Vec<Elem>,
    pub by_id: HashMap<u32, usize>,
    connectors: HashMap<String, usize>,
    /// Number of connections reading each (element, output formal).
    fanout: HashMap<(usize, String), usize>,
    memo: HashMap<(usize, String), Expression>,
    in_progress: HashSet<usize>,
    emitted: HashSet<usize>,
    /// Statements of the network being lowered.
    out: Vec<Statement>,
    /// Hidden variables, and names already declared by the POU.
    pub extra: Vec<VarDecl>,
    declared: HashSet<String>,
    /// ENO values of FB blocks that have been called.
    eno: HashMap<usize, Expression>,
    labels: HashMap<String, i128>,
    /// `rung` (LD) or `network` (FBD), for annotations.
    unit_word: &'static str,
}

pub(crate) fn lower_body(
    lower: &mut Lower,
    body: XNode,
    pou: PouKind,
    var_blocks: &mut Vec<VarBlock>,
) -> Vec<Statement> {
    let Some(mut g) = Graph::build(lower, body, pou, var_blocks) else {
        return Vec::new();
    };
    let body = g.lower_networks();
    if !g.extra.is_empty() {
        let span = g.extra[0].span;
        var_blocks.push(VarBlock {
            list_name: None,
            kind: VarBlockKind::Var,
            is_constant: false,
            is_retain: false,
            is_non_retain: false,
            declarations: std::mem::take(&mut g.extra),
            span,
        });
    }
    body
}

impl<'l, 's> Graph<'l, 's> {
/// Read a body's elements and check their wiring; `None` after a structural
/// error (reported).
pub(crate) fn build(
    lower: &'l mut Lower<'s>,
    body: XNode,
    pou: PouKind,
    var_blocks: &[VarBlock],
) -> Option<Graph<'l, 's>> {
    let before = lower.errors.len();
    let elems: Vec<Elem> = xml::elements(body)
        .filter_map(|n| parse_elem(lower, n))
        .collect();
    let mut by_id = HashMap::new();
    for (i, e) in elems.iter().enumerate() {
        if by_id.insert(e.id, i).is_some() {
            lower.err(format!("duplicate localId {}", e.id), e.span);
        }
    }
    let mut connectors = HashMap::new();
    for (i, e) in elems.iter().enumerate() {
        if let Kind::Connector { name } = &e.kind
            && connectors.insert(name.to_ascii_uppercase(), i).is_some()
        {
            lower.err(format!("connector `{name}` is defined twice"), e.span);
        }
    }
    let declared = var_blocks
        .iter()
        .flat_map(|b| b.declarations.iter())
        .map(|d| d.name.name.to_ascii_uppercase())
        .collect();
    let mut g = Graph {
        lower,
        pou,
        elems,
        by_id,
        connectors,
        fanout: HashMap::new(),
        memo: HashMap::new(),
        in_progress: HashSet::new(),
        emitted: HashSet::new(),
        out: Vec::new(),
        extra: Vec::new(),
        declared,
        eno: HashMap::new(),
        labels: HashMap::new(),
        unit_word: if xml::name(body) == "LD" { "rung" } else { "network" },
    };
    g.count_fanout();
    if g.lower.errors.len() > before {
        return None;
    }
    Some(g)
}
}

impl Graph<'_, '_> {
    fn err(&mut self, message: impl Into<String>, span: Span) {
        self.lower.err(message, span);
    }

    /// Resolve a connection to (element index, output formal, upper-cased).
    fn source(&mut self, c: &ConnKey) -> Option<(usize, String)> {
        let Some(&idx) = self.by_id.get(&c.ref_id) else {
            self.err(
                format!("connection refers to unknown localId {}", c.ref_id),
                c.span,
            );
            return None;
        };
        let e = &self.elems[idx];
        if !matches!(e.kind, Kind::Block { .. }) {
            return Some((idx, String::new()));
        }
        let formal = match &c.formal {
            Some(f) => f.clone(),
            None => match e
                .outs
                .iter()
                .find(|o| !o.formal.eq_ignore_ascii_case("ENO"))
            {
                Some(o) => o.formal.clone(),
                None => {
                    let l = e.label();
                    self.err(format!("{l} has no output to connect to"), c.span);
                    return None;
                }
            },
        };
        let known = e.outs.iter().any(|o| o.formal.eq_ignore_ascii_case(&formal))
            || e.pin(&formal).is_some_and(|p| {
                matches!(&e.kind, Kind::Block { inouts, .. } if inouts.contains(&e.ins[p].formal.to_ascii_uppercase()))
            });
        if !known {
            let l = e.label();
            self.err(format!("{l} has no output `{formal}`"), c.span);
            return None;
        }
        Some((idx, formal.to_ascii_uppercase()))
    }

    fn conn_keys(&self, idx: usize, pin: usize) -> Vec<ConnKey> {
        self.elems[idx].ins[pin]
            .conns
            .iter()
            .map(|c| ConnKey {
                ref_id: c.ref_id,
                formal: c.formal.clone(),
                span: c.span,
            })
            .collect()
    }

    fn count_fanout(&mut self) {
        for i in 0..self.elems.len() {
            if matches!(self.elems[i].kind, Kind::RightRail) {
                continue;
            }
            for p in 0..self.elems[i].ins.len() {
                for c in self.conn_keys(i, p) {
                    if let Some(k) = self.source(&c) {
                        *self.fanout.entry(k).or_default() += 1;
                    }
                }
            }
        }
    }

    // ── Networks and ordering ──

    fn networks(&mut self) -> Vec<Vec<usize>> {
        let n = self.elems.len();
        let mut parent: Vec<usize> = (0..n).collect();
        fn find(p: &mut [usize], mut x: usize) -> usize {
            while p[x] != x {
                p[x] = p[p[x]];
                x = p[x];
            }
            x
        }
        let union = |p: &mut Vec<usize>, a: usize, b: usize| {
            let (ra, rb) = (find(p, a), find(p, b));
            if ra != rb {
                p[ra] = rb;
            }
        };
        for i in 0..n {
            for pin in &self.elems[i].ins {
                for c in &pin.conns {
                    if let Some(&j) = self.by_id.get(&c.ref_id) {
                        union(&mut parent, i, j);
                    }
                }
            }
            if let Kind::Continuation { name } = &self.elems[i].kind
                && let Some(&j) = self.connectors.get(&name.to_ascii_uppercase())
            {
                union(&mut parent, i, j);
            }
        }
        let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
        for i in 0..n {
            let r = find(&mut parent, i);
            groups.entry(r).or_default().push(i);
        }
        let mut nets: Vec<Vec<usize>> = groups.into_values().collect();
        let all_ordered = nets
            .iter()
            .all(|g| g.iter().any(|&i| self.elems[i].eo.is_some()));
        let key = |g: &Vec<usize>| -> (u32, f64, f64) {
            let eo = if all_ordered {
                g.iter()
                    .filter_map(|&i| self.elems[i].eo)
                    .min()
                    .unwrap_or(u32::MAX)
            } else {
                0
            };
            let y = g
                .iter()
                .map(|&i| self.elems[i].pos.1)
                .fold(f64::INFINITY, f64::min);
            let x = g
                .iter()
                .map(|&i| self.elems[i].pos.0)
                .fold(f64::INFINITY, f64::min);
            (eo, y, x)
        };
        nets.sort_by(|a, b| {
            key(a)
                .partial_cmp(&key(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        nets
    }

    fn lower_networks(&mut self) -> Vec<Statement> {
        let nets = self.networks();
        // Labels, numbered in network order.
        let mut label_net = HashMap::new();
        for (ni, net) in nets.iter().enumerate() {
            for &i in net {
                if let Kind::Label { label } = &self.elems[i].kind {
                    let k = self.labels.len() as i128 + 1;
                    let key = label.to_ascii_uppercase();
                    if self.labels.insert(key.clone(), k).is_some() {
                        let span = self.elems[i].span;
                        self.err(format!("label `{label}` is defined twice"), span);
                    }
                    label_net.insert(key, ni);
                }
            }
        }
        let mut has_jumps = false;
        let mut backward = false;
        for (ni, net) in nets.iter().enumerate() {
            for &i in net {
                if let Kind::Jump { label } = &self.elems[i].kind {
                    has_jumps = true;
                    match label_net.get(&label.to_ascii_uppercase()) {
                        Some(&li) => backward |= li <= ni,
                        None => {
                            let span = self.elems[i].span;
                            self.err(format!("jump to undefined label `{label}`"), span);
                        }
                    }
                }
            }
        }
        let body_span = nets
            .first()
            .and_then(|n| n.first())
            .map_or(Span::empty(), |&i| self.elems[i].span);
        let jmp = |span| ident("_ld_jmp", span);
        let mut body = Vec::new();
        if has_jumps {
            self.extra.push(var_decl("_ld_jmp", "DINT", body_span));
            body.push(assign(
                jmp(body_span),
                ex(ExpressionKind::IntegerLiteral(0), body_span),
                body_span,
            ));
        }
        let mut pass = Vec::new();
        for (ni, net) in nets.iter().enumerate() {
            if self.lower.annotate {
                let span = net.first().map_or(body_span, |&i| self.elems[i].span);
                pass.push(stmt(
                    StatementKind::Comment(format!("{} {}", self.unit_word, ni + 1)),
                    span,
                ));
            }
            if let [only] = net.as_slice()
                && let Kind::Label { label } = &self.elems[*only].kind
            {
                let span = self.elems[*only].span;
                let k = self.labels[&label.to_ascii_uppercase()];
                let hit = binary(
                    BinaryOp::Equal,
                    jmp(span),
                    ex(ExpressionKind::IntegerLiteral(k), span),
                    span,
                );
                pass.extend(guarded(
                    hit,
                    vec![assign(
                        jmp(span),
                        ex(ExpressionKind::IntegerLiteral(0), span),
                        span,
                    )],
                    span,
                ));
                continue;
            }
            let stmts = self.lower_network(net);
            if stmts.is_empty() {
                continue;
            }
            if has_jumps {
                let span = stmts[0].span;
                let not_jumping = binary(
                    BinaryOp::Equal,
                    jmp(span),
                    ex(ExpressionKind::IntegerLiteral(0), span),
                    span,
                );
                pass.extend(guarded(not_jumping, stmts, span));
            } else {
                pass.extend(stmts);
            }
        }
        if backward {
            // A backward jump re-runs the pass from the top; networks before the
            // label are skipped (`_ld_jmp <> 0`) until the label clears it.
            let done = binary(
                BinaryOp::Equal,
                jmp(body_span),
                ex(ExpressionKind::IntegerLiteral(0), body_span),
                body_span,
            );
            pass.extend(guarded(
                done,
                vec![stmt(StatementKind::Exit, body_span)],
                body_span,
            ));
            body.push(stmt(
                StatementKind::While {
                    condition: lit_bool(true, body_span),
                    body: pass,
                },
                body_span,
            ));
        } else {
            body.extend(pass);
        }
        body
    }

    pub(crate) fn lower_network(&mut self, net: &[usize]) -> Vec<Statement> {
        let mut sinks: Vec<usize> = net
            .iter()
            .copied()
            .filter(|&i| self.elems[i].is_sink())
            .collect();
        sinks.sort_by(|&a, &b| {
            let k = |i: usize| {
                let e = &self.elems[i];
                (e.eo.unwrap_or(u32::MAX), e.pos.1, e.pos.0)
            };
            k(a).partial_cmp(&k(b)).unwrap_or(std::cmp::Ordering::Equal)
        });
        for s in sinks {
            self.emit_sink(s);
        }
        std::mem::take(&mut self.out)
    }

    // ── Sinks ──

    fn emit_sink(&mut self, idx: usize) {
        if self.emitted.contains(&idx) {
            return;
        }
        let span = self.elems[idx].span;
        match &self.elems[idx].kind {
            Kind::Block { .. } => {
                if self.elems[idx].is_fb() {
                    self.emit_fb_call(idx);
                } else if !self.has_consumers(idx) {
                    // A function whose result nobody reads: call it for its effects.
                    self.emitted.insert(idx);
                    if let Some(call) = self.function_call(idx, span) {
                        let en = self.en_value(idx);
                        let s = match call.kind {
                            ExpressionKind::FunctionCall { callee, args } => stmt(
                                StatementKind::FunctionCall {
                                    callee: *callee,
                                    args,
                                },
                                span,
                            ),
                            _ => {
                                // Operators have no effect when their value is unused.
                                return;
                            }
                        };
                        let body =
                            guarded(en.unwrap_or_else(|| lit_bool(true, span)), vec![s], span);
                        self.out.extend(body);
                    }
                }
            }
            Kind::Coil { .. } => self.emit_coil(idx),
            Kind::OutVar { expr, negated } => {
                let (target, negated) = (expr.clone(), *negated);
                self.emitted.insert(idx);
                // An output fed straight by a function with EN is only written
                // when EN is TRUE (IEC 61131-3: outputs keep their value).
                if let Some((f, en)) = self.enabled_function_feeding(idx) {
                    let Some(v) = self.function_call(f, span) else {
                        return;
                    };
                    let v = if negated { not(v, span) } else { v };
                    let body = guarded(en, vec![assign(target, v, span)], span);
                    self.out.extend(body);
                    return;
                }
                let Some(v) = self.required_input(idx, 0) else {
                    return;
                };
                let v = if negated { not(v, span) } else { v };
                self.out.push(assign(target, v, span));
            }
            Kind::InOutVar { expr } => {
                let target = expr.clone();
                self.emitted.insert(idx);
                let Some(v) = self.required_input(idx, 0) else {
                    return;
                };
                self.out.push(assign(target, v, span));
            }
            Kind::Jump { label } => {
                let k = self
                    .labels
                    .get(&label.to_ascii_uppercase())
                    .copied()
                    .unwrap_or(0);
                self.emitted.insert(idx);
                let Some(p) = self.required_input(idx, 0) else {
                    return;
                };
                let set = assign(
                    ident("_ld_jmp", span),
                    ex(ExpressionKind::IntegerLiteral(k), span),
                    span,
                );
                let body = guarded(p, vec![set], span);
                self.out.extend(body);
            }
            Kind::Return => {
                self.emitted.insert(idx);
                let Some(p) = self.required_input(idx, 0) else {
                    return;
                };
                let body = guarded(
                    p,
                    vec![stmt(StatementKind::Return { value: None }, span)],
                    span,
                );
                self.out.extend(body);
            }
            _ => {}
        }
    }

    fn emit_coil(&mut self, idx: usize) {
        self.emitted.insert(idx);
        let span = self.elems[idx].span;
        let Kind::Coil {
            var,
            negated,
            storage,
            edge,
        } = &self.elems[idx].kind
        else {
            return;
        };
        let (var, negated, storage, edge) = (var.clone(), *negated, *storage, *edge);
        let Some(p) = self.required_input(idx, 0) else {
            return;
        };
        if negated && (storage != Storage::None || edge != Edge::None) {
            self.err(
                "a negated coil cannot also be a set/reset or edge coil",
                span,
            );
            return;
        }
        let p = match edge {
            Edge::None => p,
            e => match self.trig(
                e == Edge::Rising,
                p,
                &format!("{}", self.elems[idx].id),
                span,
            ) {
                Some(q) => q,
                None => return,
            },
        };
        match storage {
            Storage::None => {
                let v = if negated { not(p, span) } else { p };
                self.out.push(assign(var, v, span));
            }
            Storage::Set | Storage::Reset => {
                let body = guarded(
                    p,
                    vec![assign(var, lit_bool(storage == Storage::Set, span), span)],
                    span,
                );
                self.out.extend(body);
            }
        }
    }

    fn has_consumers(&self, idx: usize) -> bool {
        self.fanout.iter().any(|((i, _), n)| *i == idx && *n > 0)
    }

    /// `(function block index, EN value)` when output variable `idx` is fed by
    /// exactly one connection from a function block with a connected EN.
    fn enabled_function_feeding(&mut self, idx: usize) -> Option<(usize, Expression)> {
        let [c] = self.elems[idx].ins[0].conns.as_slice() else {
            return None;
        };
        let key = ConnKey {
            ref_id: c.ref_id,
            formal: c.formal.clone(),
            span: c.span,
        };
        let &f = self.by_id.get(&key.ref_id)?;
        if !matches!(self.elems[f].kind, Kind::Block { instance: None, .. }) {
            return None;
        }
        self.elems[f].pin("EN")?;
        let (_, formal) = self.source(&key)?;
        if formal == "ENO" {
            return None;
        }
        let en = self.en_value(f)?;
        Some((f, en))
    }

    // ── FB calls ──

    fn emit_fb_call(&mut self, idx: usize) {
        if self.emitted.contains(&idx) || self.in_progress.contains(&idx) {
            return;
        }
        self.in_progress.insert(idx);
        let span = self.elems[idx].span;
        let Kind::Block {
            type_name,
            instance: Some(inst),
            ..
        } = &self.elems[idx].kind
        else {
            return;
        };
        let (type_name, inst) = (type_name.clone(), inst.clone());
        if !self.declared.contains(&inst.name.to_ascii_uppercase()) {
            // Not in the interface: declare it (Beremiz-style exports always do,
            // hand-written files often don't).
            self.declared.insert(inst.name.to_ascii_uppercase());
            let mut d = var_decl(&inst.name, &type_name.name, inst.span);
            d.type_spec.span = type_name.span;
            self.extra.push(d);
        }
        let mut args = Vec::new();
        let mut en = None;
        for p in 0..self.elems[idx].ins.len() {
            let formal = self.elems[idx].ins[p].formal.clone();
            let pspan = self.elems[idx].ins[p].span;
            let Some(v) = self.input(idx, p) else {
                continue;
            };
            if formal.eq_ignore_ascii_case("EN") {
                en = Some(v);
            } else {
                args.push(call_arg(&formal, v, pspan));
            }
        }
        let call = stmt(
            StatementKind::FunctionCall {
                callee: ident(&inst.name, inst.span),
                args,
            },
            span,
        );
        let eno = match en {
            Some(en) => {
                let en = if self.fanout.get(&(idx, "ENO".into())).copied().unwrap_or(0) > 0 {
                    self.latch(&format!("en{}", self.elems[idx].id), en, span)
                } else {
                    en
                };
                let body = guarded(en.clone(), vec![call], span);
                self.out.extend(body);
                en
            }
            None => {
                self.out.push(call);
                lit_bool(true, span)
            }
        };
        self.eno.insert(idx, eno);
        self.in_progress.remove(&idx);
        self.emitted.insert(idx);
    }

    /// Latch `value` into a hidden BOOL `_ld_<suffix>`; return the variable.
    fn latch(&mut self, suffix: &str, value: Expression, span: Span) -> Expression {
        // Even a plain variable is latched: a coil earlier in the rung may write
        // it before a later branch reads it.
        if matches!(value.kind, ExpressionKind::BoolLiteral(_)) {
            return value;
        }
        let name = format!("_ld_{suffix}");
        self.extra.push(var_decl(&name, "BOOL", span));
        self.out.push(assign(ident(&name, span), value, span));
        ident(&name, span)
    }

    /// Emit an edge detector on `clk`; return its `Q`.
    fn trig(
        &mut self,
        rising: bool,
        clk: Expression,
        suffix: &str,
        span: Span,
    ) -> Option<Expression> {
        if self.pou == PouKind::Function {
            self.lower.errors.push(
                PlcOpenError::new(
                    "edge detection needs state, which a FUNCTION does not have",
                    span,
                )
                .with_help("make the POU a FUNCTION_BLOCK or a PROGRAM"),
            );
            return None;
        }
        let (name, ty) = if rising {
            (format!("_ld_rt{suffix}"), "R_TRIG")
        } else {
            (format!("_ld_ft{suffix}"), "F_TRIG")
        };
        self.extra.push(var_decl(&name, ty, span));
        self.out.push(stmt(
            StatementKind::FunctionCall {
                callee: ident(&name, span),
                args: vec![call_arg("CLK", clk, span)],
            },
            span,
        ));
        Some(member(&name, "Q", span))
    }

    // ── Values ──

    fn required_input(&mut self, idx: usize, pin: usize) -> Option<Expression> {
        let v = self.input(idx, pin);
        if v.is_none() && !self.elems[idx].ins[pin].conns.is_empty() {
            return None; // already reported
        }
        if v.is_none() {
            let l = self.elems[idx].label();
            let span = self.elems[idx].span;
            self.err(format!("{l}: input is not connected"), span);
        }
        v
    }

    /// The value arriving at input `pin` of element `idx`: OR of its
    /// connections, then the pin's negation and edge. `None` when unconnected
    /// (or on an error, which is reported).
    fn input(&mut self, idx: usize, pin: usize) -> Option<Expression> {
        let span = self.elems[idx].ins[pin].span;
        let mut acc: Option<Expression> = self.elems[idx].ins[pin].expr.clone();
        for c in self.conn_keys(idx, pin) {
            let (src, formal) = self.source(&c)?;
            let v = self.output(src, &formal)?;
            acc = Some(match acc {
                None => v,
                Some(a) => or(a, v, span),
            });
        }
        let mut v = acc?;
        let (negated, edge) = (
            self.elems[idx].ins[pin].negated,
            self.elems[idx].ins[pin].edge,
        );
        if negated {
            v = not(v, span);
        }
        if edge != Edge::None {
            let suffix = format!("{}_{}", self.elems[idx].id, self.elems[idx].ins[pin].formal);
            v = self.trig(edge == Edge::Rising, v, &suffix, span)?;
        }
        Some(v)
    }

    fn en_value(&mut self, idx: usize) -> Option<Expression> {
        let p = self.elems[idx].pin("EN")?;
        self.input(idx, p)
    }

    /// The value at output `formal` (upper-cased; empty for single-output
    /// elements) of element `idx`.
    fn output(&mut self, idx: usize, formal: &str) -> Option<Expression> {
        let key = (idx, formal.to_string());
        if let Some(v) = self.memo.get(&key) {
            return Some(v.clone());
        }
        let span = self.elems[idx].span;
        let is_fb = self.elems[idx].is_fb();
        if self.in_progress.contains(&idx) && !is_fb {
            let l = self.elems[idx].label();
            self.err(format!("{l} feeds back into itself"), span);
            return None;
        }
        let v = match &self.elems[idx].kind {
            Kind::LeftRail => lit_bool(true, span),
            Kind::Contact { var, negated, edge } => {
                let (var, negated, edge) = (var.clone(), *negated, *edge);
                self.in_progress.insert(idx);
                let p = self.required_input(idx, 0);
                self.in_progress.remove(&idx);
                let p = p?;
                let term = match (edge, negated) {
                    (Edge::None, false) => var,
                    (Edge::None, true) => not(var, span),
                    (e, false) => self.trig(
                        e == Edge::Rising,
                        var,
                        &format!("{}", self.elems[idx].id),
                        span,
                    )?,
                    (_, true) => {
                        self.err("a contact cannot be both negated and edge-sensing", span);
                        return None;
                    }
                };
                let v = and(p, term, span);
                self.shared(idx, v, span)
            }
            Kind::Coil { .. } => {
                // A coil passes its input power on to the right.
                self.in_progress.insert(idx);
                let p = self.required_input(idx, 0);
                self.in_progress.remove(&idx);
                let p = p?;
                self.shared(idx, p, span)
            }
            Kind::InVar { expr, negated } => {
                if *negated {
                    not(expr.clone(), span)
                } else {
                    expr.clone()
                }
            }
            Kind::InOutVar { expr } => {
                // Its output is the variable, read after it was written.
                let e = expr.clone();
                self.emit_sink(idx);
                e
            }
            Kind::Continuation { name } => {
                let Some(&c) = self.connectors.get(&name.to_ascii_uppercase()) else {
                    self.err(
                        format!("continuation `{name}` has no matching connector"),
                        span,
                    );
                    return None;
                };
                self.in_progress.insert(idx);
                let v = self.required_input(c, 0);
                self.in_progress.remove(&idx);
                v?
            }
            Kind::Block { .. } => {
                let v = self.block_output(idx, formal, span)?;
                // Don't memoize FB outputs read during the FB's own call
                // (feedback): later readers must see the new value.
                if self.in_progress.contains(&idx) {
                    return Some(v);
                }
                v
            }
            _ => {
                let l = self.elems[idx].label();
                self.err(format!("{l} has no output to connect from"), span);
                return None;
            }
        };
        self.memo.insert(key, v.clone());
        Some(v)
    }

    /// Latch a power-flow value that fans out to several elements, so every
    /// branch sees the value this element had when the rung reached it.
    fn shared(&mut self, idx: usize, v: Expression, span: Span) -> Expression {
        let n = self.fanout.get(&(idx, String::new())).copied().unwrap_or(0);
        if n > 1 {
            self.latch(&format!("p{}", self.elems[idx].id), v, span)
        } else {
            v
        }
    }

    fn block_output(&mut self, idx: usize, formal: &str, span: Span) -> Option<Expression> {
        let Kind::Block {
            instance, inouts, ..
        } = &self.elems[idx].kind
        else {
            return None;
        };
        let negated = self.elems[idx]
            .outs
            .iter()
            .any(|o| o.formal.eq_ignore_ascii_case(formal) && o.negated);
        let is_inout = inouts.contains(formal);
        let v = match instance.clone() {
            Some(inst) => {
                if formal == "ENO" {
                    self.emit_fb_call(idx);
                    // In a feedback loop through ENO, use TRUE (the call has not
                    // happened yet); otherwise the latched EN.
                    self.eno
                        .get(&idx)
                        .cloned()
                        .unwrap_or_else(|| lit_bool(true, span))
                } else if is_inout {
                    let p = self.elems[idx].pin(formal)?;
                    self.emit_fb_call(idx);
                    self.input(idx, p)?
                } else {
                    // Feedback (reading an FB's output while its own inputs are
                    // being evaluated) reads the previous scan's value, as IEC
                    // 61131-3 specifies for FBD feedback paths.
                    self.emit_fb_call(idx);
                    member(&inst.name, formal, span)
                }
            }
            None => {
                if formal == "ENO" {
                    self.en_value(idx).unwrap_or_else(|| lit_bool(true, span))
                } else if is_inout {
                    let p = self.elems[idx].pin(formal)?;
                    self.input(idx, p)?
                } else {
                    let ret = self.elems[idx]
                        .outs
                        .iter()
                        .find(|o| !o.formal.eq_ignore_ascii_case("ENO"))
                        .map(|o| o.formal.to_ascii_uppercase());
                    if ret.as_deref() != Some(formal) {
                        let l = self.elems[idx].label();
                        self.err(
                            format!(
                                "{l}: reading function output `{formal}` other than the \
                                 result is not supported yet"
                            ),
                            span,
                        );
                        return None;
                    }
                    self.in_progress.insert(idx);
                    let v = self.function_call(idx, span);
                    self.in_progress.remove(&idx);
                    v?
                }
            }
        };
        Some(if negated { not(v, span) } else { v })
    }

    /// A function block as an expression: IEC operators become ST operators,
    /// anything else a call (positional when every input is connected).
    fn function_call(&mut self, idx: usize, span: Span) -> Option<Expression> {
        let Kind::Block { type_name, .. } = &self.elems[idx].kind else {
            return None;
        };
        let type_name = type_name.clone();
        let mut args: Vec<(String, Option<Expression>, Span)> = Vec::new();
        for p in 0..self.elems[idx].ins.len() {
            let formal = self.elems[idx].ins[p].formal.clone();
            if formal.eq_ignore_ascii_case("EN") {
                continue;
            }
            let pspan = self.elems[idx].ins[p].span;
            let has_source =
                !self.elems[idx].ins[p].conns.is_empty() || self.elems[idx].ins[p].expr.is_some();
            let v = self.input(idx, p);
            if has_source && v.is_none() {
                return None;
            }
            args.push((formal, v, pspan));
        }
        let upper = type_name.name.to_ascii_uppercase();
        let all: Option<Vec<Expression>> = args.iter().map(|(_, v, _)| v.clone()).collect();
        let fold = |op: BinaryOp, vals: Vec<Expression>| {
            vals.into_iter().reduce(|l, r| binary(op, l, r, span))
        };
        let op = match upper.as_str() {
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
        let cmp = match upper.as_str() {
            "GT" => Some(BinaryOp::Greater),
            "GE" => Some(BinaryOp::GreaterEqual),
            "EQ" => Some(BinaryOp::Equal),
            "LE" => Some(BinaryOp::LessEqual),
            "LT" => Some(BinaryOp::Less),
            "NE" => Some(BinaryOp::NotEqual),
            _ => None,
        };
        let arity = |g: &mut Self, want: &str, ok: bool| {
            if !ok {
                g.err(
                    format!("{} needs {want} connected input(s)", type_name.name),
                    span,
                );
            }
            ok
        };
        if let Some((bop, extensible)) = op {
            let vals = all.unwrap_or_default();
            let ok = if extensible {
                vals.len() >= 2
            } else {
                vals.len() == 2
            };
            if !arity(self, if extensible { "2 or more" } else { "exactly 2" }, ok) {
                return None;
            }
            return fold(bop, vals);
        }
        if let Some(c) = cmp {
            let vals = all.unwrap_or_default();
            let ok = if c == BinaryOp::NotEqual {
                vals.len() == 2
            } else {
                vals.len() >= 2
            };
            if !arity(self, "2 or more", ok) {
                return None;
            }
            // GT(a, b, c) = a > b AND b > c
            return vals
                .windows(2)
                .map(|w| binary(c, w[0].clone(), w[1].clone(), span))
                .reduce(|l, r| binary(BinaryOp::And, l, r, span));
        }
        match upper.as_str() {
            "NOT" | "MOVE" => {
                let vals = all.unwrap_or_default();
                if !arity(self, "exactly 1", vals.len() == 1) {
                    return None;
                }
                let v = vals.into_iter().next()?;
                return Some(if upper == "NOT" { not(v, span) } else { v });
            }
            _ => {}
        }
        let positional = all.is_some();
        let call_args = args
            .into_iter()
            .filter_map(|(formal, v, pspan)| {
                let v = v?;
                Some(CallArg {
                    name: (!positional).then(|| Ident::new(formal, pspan)),
                    value: v,
                    is_output: false,
                    negated: false,
                    span: pspan,
                })
            })
            .collect();
        Some(ex(
            ExpressionKind::FunctionCall {
                callee: Box::new(ident(&type_name.name, type_name.span)),
                args: call_args,
            },
            span,
        ))
    }
}

struct ConnKey {
    ref_id: u32,
    formal: Option<String>,
    span: Span,
}
