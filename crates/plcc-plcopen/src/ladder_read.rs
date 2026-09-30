// SPDX-License-Identifier: MPL-2.0

//! PLCopen XML → the ladder model (`plcc-ladder`, IEC dialect).
//!
//! An LD body is a graph; the model is a series/parallel tree. Each network
//! (elements joined by connections, rails left out, so a rail shared by every
//! rung does not merge them) is reduced:
//!
//! 1. Wires are nodes: an element's input is the node of everything wired
//!    into it (several connections = one joined wire); the left rail is the
//!    source `S`; the right rail, and every output nothing reads, the sink `T`.
//! 2. Elements are edges between nodes. A block is an edge from its power
//!    input (the first input pin fed by power flow; `S` when none is) to its
//!    power output (the first output pin read by power flow). Pins fed by
//!    `inVariable`s / expressions or read by `outVariable`s are data pins
//!    (`Pin::value`). A second power-fed input, or a second output read by
//!    power flow, becomes a pin path (`Pin::rung`): it must reduce on its own.
//! 3. Series (a node with one edge in and one out) and parallel (edges with
//!    the same ends) reductions repeat until one edge `S → T` is left: the
//!    rung. Parallel legs are ordered top to bottom by position.
//!
//! A network that does not reduce (a bridge, a contact whose output feeds two
//! joins, a block output wired into another block's data pin, an edge on a
//! block pin) becomes a rung holding one ST box with the Structured Text the
//! network lowers to (the ordinary LD lowering), so nothing is dropped; the
//! reason is kept in the box's notes.
//!
//! Ids: an element keeps its `localId` (when it is not taken already); a rung
//! takes the `localId` of its left rail when the rail serves that rung only.
//! Ids of POUs, routines, branches and other rungs are numbered after the
//! largest of those, in document order, so reading the same XML twice, or the
//! XML [`crate::ladder_write`] made from a model, gives the same ids.

use crate::error::PlcOpenError;
use crate::graph::{Edge, Elem, Graph, Kind, Storage};
use crate::project::{Lower, PouKind};
use crate::xml::{self, XNode};
use plcc_ladder::model::{self as m, Element, Id, Ids};
use plcc_st::ast::*;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Name prefix of the actions that hold ST boxes (see `ladder_write`).
pub(crate) const ST_BOX_ACTION: &str = "LD_ST_";

/// Read every POU of a PLCopen project into the ladder model. LD bodies
/// become rungs; ST bodies a routine with one ST box; FBD bodies an ST box
/// with the Structured Text they lower to. Data types and configurations are
/// not part of the model.
pub fn read(source: &str) -> (Option<m::Project>, Vec<PlcOpenError>) {
    let doc = match roxmltree::Document::parse(source) {
        Ok(d) => d,
        Err(e) => {
            return (
                None,
                vec![PlcOpenError::new(
                    format!("malformed XML: {e}"),
                    plcc_st::Span::new(0, 0),
                )],
            );
        }
    };
    let root = doc.root_element();
    if xml::name(root) != "project" {
        return (
            None,
            vec![PlcOpenError::new(
                "not a PLCopen XML project",
                xml::tag_span(source, root),
            )],
        );
    }
    let mut lower = Lower::new(source);
    let mut ids = Ids::new();
    // Every localId is kept for the element that has it.
    for n in root.descendants().filter(|n| n.is_element()) {
        if let Some(id) = xml::attr(n, "localId").and_then(|v| v.trim().parse::<Id>().ok()) {
            ids.reserve(id);
        }
    }
    let mut project = m::Project {
        dialect: m::Dialect::Iec,
        name: xml::child(root, "contentHeader")
            .and_then(|h| xml::attr(h, "name"))
            .unwrap_or("")
            .to_string(),
        globals: Vec::new(),
        pous: Vec::new(),
        declarations: Vec::new(),
    };
    if let Some(types) = xml::child(root, "types")
        && let Some(pous) = xml::child(types, "pous")
    {
        for pou in xml::children(pous, "pou") {
            if let Some(p) = read_pou(&mut lower, pou, &mut ids) {
                project.pous.push(p);
            }
        }
    }
    if let Some(inst) = xml::child(root, "instances")
        && let Some(cfgs) = xml::child(inst, "configurations")
    {
        for cfg in xml::children(cfgs, "configuration") {
            if let Some(g) = xml::child(cfg, "globalVars") {
                let b = lower.var_block(g, VarBlockKind::VarGlobal);
                project.globals.extend(variables(&[b]));
            }
        }
    }
    number_structure(&mut project);
    (Some(project), lower.errors)
}

/// POU, routine, branch and unnumbered rung ids (0 while reading), after the
/// largest element / rung id, in document order.
fn number_structure(p: &mut m::Project) {
    let mut next = p.max_id();
    let mut fresh = || {
        next += 1;
        next
    };
    fn series(s: &mut [Element], fresh: &mut dyn FnMut() -> Id) {
        for e in s {
            match e {
                Element::Branch(b) => {
                    if b.id == 0 {
                        b.id = fresh();
                    }
                    for l in &mut b.legs {
                        series(l, fresh);
                    }
                }
                Element::Block(b) => {
                    for pin in &mut b.pins {
                        if let Some(r) = &mut pin.rung {
                            series(r, fresh);
                        }
                    }
                }
                Element::St(s) if s.id == 0 => s.id = fresh(),
                _ => {}
            }
        }
    }
    for pou in &mut p.pous {
        pou.id = fresh();
        for r in &mut pou.routines {
            r.id = fresh();
            for g in &mut r.rungs {
                if g.id == 0 {
                    g.id = fresh();
                }
                series(&mut g.elements, &mut fresh);
            }
        }
    }
}

pub(crate) fn variables(blocks: &[VarBlock]) -> Vec<m::Variable> {
    let mut out = Vec::new();
    for b in blocks {
        let section = match b.kind {
            VarBlockKind::VarInput => m::VarSection::Input,
            VarBlockKind::VarOutput => m::VarSection::Output,
            VarBlockKind::VarInOut => m::VarSection::InOut,
            VarBlockKind::VarExternal => m::VarSection::External,
            VarBlockKind::VarTemp => m::VarSection::Temp,
            VarBlockKind::VarGlobal => m::VarSection::Global,
            _ => m::VarSection::Local,
        };
        for d in &b.declarations {
            out.push(m::Variable {
                name: d.name.name.clone(),
                data_type: plcc_st::printer::print_type_spec(&d.type_spec),
                section,
                initial: d.initializer.as_ref().map(plcc_st::print_expression),
                address: d.at_address.as_ref().map(|a| a.repr.clone()),
                comment: None,
                constant: b.is_constant,
                retain: b.is_retain,
            });
        }
    }
    out
}

fn read_pou(lower: &mut Lower, pou: XNode, ids: &mut Ids) -> Option<m::Pou> {
    let name = xml::attr(pou, "name")?.trim().to_string();
    let (kind, pk) = match xml::attr(pou, "pouType").map(str::trim) {
        Some("program") => (m::PouKind::Program, PouKind::Program),
        Some("functionBlock") => (m::PouKind::FunctionBlock, PouKind::FunctionBlock),
        Some("function") => (m::PouKind::Function, PouKind::Function),
        _ => return None,
    };
    let (mut blocks, ret) = match xml::child(pou, "interface") {
        Some(i) => lower.interface(i),
        None => (Vec::new(), None),
    };
    let mut out = m::Pou {
        id: 0,
        name: name.clone(),
        kind,
        return_type: ret.as_ref().map(plcc_st::printer::print_type_spec),
        variables: Vec::new(),
        routines: Vec::new(),
        members: String::new(),
    };
    // ST boxes written as actions `LD_ST_<id>` (ladder_write).
    let mut st_actions: HashMap<String, (String, Vec<String>)> = HashMap::new();
    if let Some(acts) = xml::child(pou, "actions") {
        for a in xml::children(acts, "action") {
            let an = xml::attr(a, "name").unwrap_or("").trim().to_string();
            if let Some(b) = xml::child(a, "body")
                && let Some(st) = xml::child(b, "ST")
            {
                let code = crate::xml::content_fragment(lower.src, st).text;
                let notes = xml::child(a, "documentation")
                    .map(|d| {
                        comment_text(d)
                            .lines()
                            .map(str::to_string)
                            .filter(|l| !l.is_empty())
                            .collect()
                    })
                    .unwrap_or_default();
                st_actions.insert(an.to_ascii_uppercase(), (code.trim().to_string(), notes));
            }
        }
    }
    let body = xml::child(pou, "body")?;
    let lang =
        xml::elements(body).find(|e| !matches!(xml::name(*e), "documentation" | "addData"))?;
    // LD actions (other than ST boxes) are further routines.
    let mut extra_routines = Vec::new();
    if let Some(acts) = xml::child(pou, "actions") {
        for a in xml::children(acts, "action") {
            let an = xml::attr(a, "name").unwrap_or("").trim().to_string();
            if an.to_ascii_uppercase().starts_with(ST_BOX_ACTION) {
                continue;
            }
            let Some(b) = xml::child(a, "body") else {
                continue;
            };
            let Some(l) =
                xml::elements(b).find(|e| !matches!(xml::name(*e), "documentation" | "addData"))
            else {
                continue;
            };
            let mut r = match xml::name(l) {
                "LD" => read_ld(lower, l, pk, &mut blocks, ids, &st_actions),
                "ST" => {
                    let code = crate::xml::content_fragment(lower.src, l).text;
                    st_routine(&an, code.trim().to_string(), "an ST action")
                }
                _ => continue,
            };
            r.name = an;
            extra_routines.push(r);
        }
    }
    let routine = match xml::name(lang) {
        "LD" => read_ld(lower, lang, pk, &mut blocks, ids, &st_actions),
        "ST" => {
            let code = crate::xml::content_fragment(lower.src, lang).text;
            st_routine(&name, code.trim().to_string(), "an ST body")
        }
        "FBD" => {
            let stmts = crate::graph::lower_body(lower, lang, pk, &mut blocks);
            st_routine(
                &name,
                plcc_st::print_statements(&stmts, 0).trim_end().to_string(),
                "an FBD body, as the ST it lowers to",
            )
        }
        _ => return None,
    };
    out.variables = variables(&blocks);
    let mut routine = routine;
    if routine.name.is_empty() {
        routine.name = name.clone();
    }
    out.routines.push(routine);
    out.routines.extend(extra_routines);
    Some(out)
}

fn st_routine(name: &str, code: String, what: &str) -> m::Routine {
    m::Routine {
        id: 0,
        name: name.to_string(),
        rungs: vec![m::Rung {
            id: 0,
            elements: vec![Element::St(m::StBox {
                id: 0,
                code,
                notes: vec![format!("{what}: not ladder")],
            })],
            ..Default::default()
        }],
    }
}

/// Text of a graphical `<comment>`.
fn comment_text(n: XNode) -> String {
    let mut t = String::new();
    for d in n.descendants() {
        if d.is_text()
            && let Some(s) = d.text()
        {
            t.push_str(s);
        }
    }
    t.trim().to_string()
}

fn read_ld(
    lower: &mut Lower,
    lang: XNode,
    pou: PouKind,
    blocks: &mut Vec<VarBlock>,
    ids: &mut Ids,
    st_actions: &HashMap<String, (String, Vec<String>)>,
) -> m::Routine {
    let comments: Vec<(f64, u32, String)> = xml::children(lang, "comment")
        .map(|c| {
            let id = xml::attr(c, "localId")
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0);
            (xml::position(c).1, id, comment_text(c))
        })
        .collect();
    let mut routine = m::Routine {
        id: 0,
        name: String::new(),
        rungs: Vec::new(),
    };
    let Some(mut g) = Graph::build(lower, lang, pou, blocks) else {
        return routine;
    };
    let nets = networks(&g.elems, &g.by_id);
    // Label-only networks attach to the network they sit beside.
    let mut rungs: Vec<(Option<String>, Vec<usize>, f64)> = Vec::new();
    let mut i = 0;
    while i < nets.len() {
        let net = &nets[i];
        if let [only] = net.as_slice()
            && let Kind::Label { label } = &g.elems[*only].kind
        {
            let y = g.elems[*only].pos.1;
            if let Some(next) = nets.get(i + 1)
                && !is_label_only(&g.elems, next)
                && y >= min_y(&g.elems, next) - 1.0
                && y <= max_y(&g.elems, next)
            {
                rungs.push((Some(label.clone()), next.clone(), y));
                i += 2;
                continue;
            }
            rungs.push((Some(label.clone()), Vec::new(), y));
            i += 1;
            continue;
        }
        rungs.push((None, net.clone(), min_y(&g.elems, net)));
        i += 1;
    }
    let mut failed: Vec<(usize, String)> = Vec::new();
    let mut built: Vec<m::Rung> = Vec::new();
    for (k, (label, net, _)) in rungs.iter().enumerate() {
        let mut rung = m::Rung {
            id: 0,
            label: label.clone(),
            ..Default::default()
        };
        if !net.is_empty() {
            match Reducer::new(&g.elems, &g.by_id, net, ids, st_actions).rung() {
                Ok((elements, rail)) => {
                    rung.elements = elements;
                    if let Some(r) = rail {
                        rung.id = ids.claim(Some(r));
                    }
                }
                Err(why) => failed.push((k, why)),
            }
        }
        built.push(rung);
    }
    // Networks that do not reduce: the ST they lower to, in an ST box.
    for (k, why) in failed {
        let stmts = g.lower_network(&rungs[k].1);
        built[k].elements = vec![Element::St(m::StBox {
            id: 0,
            code: plcc_st::print_statements(&stmts, 0).trim_end().to_string(),
            notes: vec![format!("not a series/parallel network: {why}")],
        })];
    }
    if !g.extra.is_empty() {
        blocks.push(VarBlock {
            list_name: None,
            kind: VarBlockKind::Var,
            is_constant: false,
            is_retain: false,
            is_non_retain: false,
            declarations: std::mem::take(&mut g.extra),
            span: plcc_st::Span::empty(),
        });
    }
    // Comments: each goes to the first rung at or below it.
    let tops: Vec<f64> = rungs.iter().map(|r| r.2).collect();
    for (y, _, text) in &comments {
        if text.is_empty() {
            continue;
        }
        if let Some(k) = tops.iter().position(|t| *t >= *y)
            && built[k].comment.is_none()
        {
            built[k].comment = Some(text.clone());
        }
    }
    routine.rungs = built;
    routine
}

fn is_label_only(elems: &[Elem], net: &[usize]) -> bool {
    matches!(net, [only] if matches!(elems[*only].kind, Kind::Label { .. }))
}

fn min_y(elems: &[Elem], net: &[usize]) -> f64 {
    net.iter()
        .map(|&i| elems[i].pos.1)
        .fold(f64::INFINITY, f64::min)
}

fn max_y(elems: &[Elem], net: &[usize]) -> f64 {
    net.iter()
        .map(|&i| elems[i].pos.1)
        .fold(f64::NEG_INFINITY, f64::max)
}

fn is_rail(e: &Elem) -> bool {
    matches!(e.kind, Kind::LeftRail | Kind::RightRail)
}

/// Networks without the rails, in execution order (as the LD lowering orders
/// them).
fn networks(elems: &[Elem], by_id: &HashMap<u32, usize>) -> Vec<Vec<usize>> {
    let n = elems.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    let mut connectors = HashMap::new();
    for (i, e) in elems.iter().enumerate() {
        if let Kind::Connector { name } = &e.kind {
            connectors.insert(name.to_ascii_uppercase(), i);
        }
    }
    for i in 0..n {
        if is_rail(&elems[i]) {
            continue;
        }
        for pin in &elems[i].ins {
            for c in &pin.conns {
                if let Some(&j) = by_id.get(&c.ref_id)
                    && !is_rail(&elems[j])
                {
                    let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                    parent[a] = b;
                }
            }
        }
        if let Kind::Continuation { name } = &elems[i].kind
            && let Some(&j) = connectors.get(&name.to_ascii_uppercase())
        {
            let (a, b) = (find(&mut parent, i), find(&mut parent, j));
            parent[a] = b;
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..n {
        if is_rail(&elems[i]) {
            continue;
        }
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    let mut nets: Vec<Vec<usize>> = groups.into_values().collect();
    let all_ordered = nets
        .iter()
        .all(|g| g.iter().any(|&i| elems[i].eo.is_some()));
    let key = |g: &Vec<usize>| -> (u32, f64, f64) {
        let eo = if all_ordered {
            g.iter()
                .filter_map(|&i| elems[i].eo)
                .min()
                .unwrap_or(u32::MAX)
        } else {
            0
        };
        let y = min_y(elems, g);
        let x = g
            .iter()
            .map(|&i| elems[i].pos.0)
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

// ── Series/parallel reduction ──

#[derive(Clone, Debug)]
enum Tree {
    Leaf(usize),
    Ser(Vec<Tree>),
    Par(Vec<Tree>),
}

impl Tree {
    fn min_y(&self, elems: &[Elem]) -> f64 {
        match self {
            Tree::Leaf(i) => elems[*i].pos.1,
            Tree::Ser(v) | Tree::Par(v) => v
                .iter()
                .map(|t| t.min_y(elems))
                .fold(f64::INFINITY, f64::min),
        }
    }
}

struct Edge2 {
    from: usize,
    to: usize,
    t: Tree,
}

/// How each pin of a block is wired.
#[derive(Default)]
struct BlockWiring {
    /// Input pins fed by power flow, in pin order.
    power_ins: Vec<usize>,
    /// Output formals (upper case) read by power flow, in pin order.
    power_outs: Vec<String>,
    /// Input pin → data expression text.
    data_in: HashMap<usize, String>,
    /// Output formal → outVariable text.
    data_out: HashMap<String, String>,
}

struct Reducer<'a> {
    elems: &'a [Elem],
    by_id: &'a HashMap<u32, usize>,
    net: &'a [usize],
    in_net: HashSet<usize>,
    ids: &'a mut Ids,
    st_actions: &'a HashMap<String, (String, Vec<String>)>,
    /// Union-find over wire nodes.
    parent: Vec<usize>,
    ports: HashMap<(usize, String), usize>,
    s: usize,
    t: usize,
    edges: Vec<Edge2>,
    wiring: HashMap<usize, BlockWiring>,
    /// Consumers of each (element, output formal): (element, input pin).
    consumers: HashMap<(usize, String), Vec<(usize, usize)>>,
    /// Secondary pins: (block, pin index or output formal) → terminal node.
    pin_in_nodes: Vec<(usize, usize, usize)>,
    pin_out_nodes: Vec<(usize, String, usize)>,
    /// The left rail feeding this network, when it feeds nothing else.
    rail: Option<u32>,
}

type R<T> = Result<T, String>;

impl<'a> Reducer<'a> {
    fn new(
        elems: &'a [Elem],
        by_id: &'a HashMap<u32, usize>,
        net: &'a [usize],
        ids: &'a mut Ids,
        st_actions: &'a HashMap<String, (String, Vec<String>)>,
    ) -> Self {
        let mut r = Reducer {
            elems,
            by_id,
            net,
            in_net: net.iter().copied().collect(),
            ids,
            st_actions,
            parent: Vec::new(),
            ports: HashMap::new(),
            s: 0,
            t: 0,
            edges: Vec::new(),
            wiring: HashMap::new(),
            consumers: HashMap::new(),
            pin_in_nodes: Vec::new(),
            pin_out_nodes: Vec::new(),
            rail: None,
        };
        r.s = r.node();
        r.t = r.node();
        r
    }

    fn node(&mut self) -> usize {
        self.parent.push(self.parent.len());
        self.parent.len() - 1
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) -> usize {
        let (ra, rb) = (self.find(a), self.find(b));
        // Keep S and T as representatives.
        if ra == rb {
            return ra;
        }
        if rb == self.s || rb == self.t {
            self.parent[ra] = rb;
            rb
        } else {
            self.parent[rb] = ra;
            ra
        }
    }

    fn port(&mut self, elem: usize, formal: &str) -> usize {
        if let Some(&p) = self.ports.get(&(elem, formal.to_string())) {
            return p;
        }
        let p = self.node();
        self.ports.insert((elem, formal.to_string()), p);
        p
    }

    /// The (element, output formal upper-cased) a connection reads.
    fn source(&self, c: &crate::graph::Conn) -> R<(usize, String)> {
        let &j = self
            .by_id
            .get(&c.ref_id)
            .ok_or_else(|| format!("unknown localId {}", c.ref_id))?;
        let e = &self.elems[j];
        if !matches!(e.kind, Kind::Block { .. }) {
            return Ok((j, String::new()));
        }
        let f = match &c.formal {
            Some(f) => f.clone(),
            None => e
                .outs
                .iter()
                .find(|o| !o.formal.eq_ignore_ascii_case("ENO"))
                .map(|o| o.formal.clone())
                .ok_or("a block output without a name")?,
        };
        Ok((j, f.to_ascii_uppercase()))
    }

    fn is_data_source(&self, j: usize) -> bool {
        matches!(
            self.elems[j].kind,
            Kind::InVar { .. } | Kind::InOutVar { .. }
        )
    }

    fn is_data_sink(&self, k: usize) -> bool {
        matches!(
            self.elems[k].kind,
            Kind::OutVar { .. } | Kind::InOutVar { .. }
        )
    }

    /// The wire node an input pin sits on (power sources only).
    fn input_node(&mut self, i: usize, pin: usize) -> R<usize> {
        let conns: Vec<(usize, String)> = self.elems[i].ins[pin]
            .conns
            .iter()
            .map(|c| self.source(c))
            .collect::<R<_>>()?;
        if conns.is_empty() {
            return Err(format!("{}: input not connected", self.elems[i].label()));
        }
        let mut node = None;
        for (j, f) in conns {
            let n = match &self.elems[j].kind {
                Kind::LeftRail => {
                    let rid = self.elems[j].id;
                    self.rail = match self.rail {
                        None => Some(rid),
                        Some(r) if r == rid => Some(r),
                        _ => Some(u32::MAX),
                    };
                    self.s
                }
                Kind::Continuation { name } => {
                    // The node at the connector's input.
                    let up = name.to_ascii_uppercase();
                    let ci = self
                        .elems
                        .iter()
                        .position(|e| matches!(&e.kind, Kind::Connector { name } if name.to_ascii_uppercase() == up))
                        .ok_or_else(|| format!("continuation `{name}` has no connector"))?;
                    self.port(ci, "")
                }
                _ if self.is_data_source(j) => {
                    return Err(format!("{} is fed by a variable", self.elems[i].label()));
                }
                _ => self.port(j, &f),
            };
            node = Some(match node {
                None => n,
                Some(prev) => self.union(prev, n),
            });
        }
        Ok(self.find(node.unwrap_or(self.s)))
    }

    fn classify_block(&mut self, i: usize) -> R<()> {
        let e = &self.elems[i];
        let mut w = BlockWiring::default();
        for (p, pin) in e.ins.iter().enumerate() {
            if pin.edge != Edge::None {
                return Err(format!("{}: an edge on pin {}", e.label(), pin.formal));
            }
            if let Some(x) = &pin.expr {
                if !pin.conns.is_empty() {
                    return Err(format!(
                        "{}: pin {} mixes wires and an expression",
                        e.label(),
                        pin.formal
                    ));
                }
                w.data_in.insert(p, plcc_st::print_expression(x));
                continue;
            }
            if pin.conns.is_empty() {
                continue;
            }
            let srcs: Vec<(usize, String)> =
                pin.conns.iter().map(|c| self.source(c)).collect::<R<_>>()?;
            let data = srcs.iter().filter(|(j, _)| self.is_data_source(*j)).count();
            if data == srcs.len() {
                if srcs.len() != 1 {
                    return Err(format!("{}: pin {} joins variables", e.label(), pin.formal));
                }
                let j = srcs[0].0;
                let text = match &self.elems[j].kind {
                    Kind::InVar { expr, negated } => {
                        let t = plcc_st::print_expression(expr);
                        if *negated { format!("NOT ({t})") } else { t }
                    }
                    Kind::InOutVar { expr } => plcc_st::print_expression(expr),
                    _ => unreachable!(),
                };
                w.data_in.insert(p, text);
            } else if data == 0 {
                w.power_ins.push(p);
            } else {
                return Err(format!(
                    "{}: pin {} mixes power and a variable",
                    e.label(),
                    pin.formal
                ));
            }
        }
        for o in &e.outs {
            let f = o.formal.to_ascii_uppercase();
            let cons = self
                .consumers
                .get(&(i, f.clone()))
                .cloned()
                .unwrap_or_default();
            if cons.is_empty() {
                continue;
            }
            let data = cons.iter().filter(|(k, _)| self.is_data_sink(*k)).count();
            if data == cons.len() {
                if cons.len() != 1 {
                    return Err(format!(
                        "{}: output {} feeds several variables",
                        e.label(),
                        o.formal
                    ));
                }
                let k = cons[0].0;
                match &self.elems[k].kind {
                    Kind::OutVar {
                        expr,
                        negated: false,
                    } => {
                        w.data_out.insert(f, plcc_st::print_expression(expr));
                    }
                    _ => {
                        return Err(format!(
                            "{}: output {} feeds a negated or in-out variable",
                            e.label(),
                            o.formal
                        ));
                    }
                }
            } else if data == 0 {
                w.power_outs.push(f);
            } else {
                return Err(format!(
                    "{}: output {} feeds power and a variable",
                    e.label(),
                    o.formal
                ));
            }
        }
        // In-out pins read on their output side are not drawable.
        if let Kind::Block { inouts, .. } = &e.kind {
            for io in inouts {
                if self
                    .consumers
                    .get(&(i, io.clone()))
                    .is_some_and(|c| !c.is_empty())
                {
                    return Err(format!(
                        "{}: in-out {} is read on its output side",
                        e.label(),
                        io
                    ));
                }
            }
        }
        self.wiring.insert(i, w);
        Ok(())
    }

    /// Reduce the network; the rung's elements and the id of its own left rail.
    fn rung(mut self) -> R<(Vec<Element>, Option<u32>)> {
        // Who reads what.
        for &k in self.net {
            for (q, pin) in self.elems[k].ins.iter().enumerate() {
                for c in &pin.conns {
                    let (j, f) = self.source(c)?;
                    self.consumers.entry((j, f)).or_default().push((k, q));
                }
            }
        }
        // Outputs read by the right rail count as power reads.
        for (j, e) in self.elems.iter().enumerate() {
            if matches!(e.kind, Kind::RightRail) {
                for (q, pin) in e.ins.iter().enumerate() {
                    for c in &pin.conns {
                        let (src, f) = self.source(c)?;
                        if self.in_net.contains(&src) {
                            self.consumers.entry((src, f)).or_default().push((j, q));
                        }
                    }
                }
            }
        }
        let net = self.net.to_vec();
        for &i in &net {
            if matches!(self.elems[i].kind, Kind::Block { .. }) {
                self.classify_block(i)?;
            }
        }
        // Edges.
        for &i in &net {
            match &self.elems[i].kind {
                Kind::Contact { .. } | Kind::Coil { .. } => {
                    let a = self.input_node(i, 0)?;
                    let b = self.port(i, "");
                    self.edges.push(Edge2 {
                        from: a,
                        to: b,
                        t: Tree::Leaf(i),
                    });
                }
                Kind::Jump { .. } | Kind::Return => {
                    let a = self.input_node(i, 0)?;
                    let b = self.node();
                    self.edges.push(Edge2 {
                        from: a,
                        to: b,
                        t: Tree::Leaf(i),
                    });
                }
                Kind::Connector { .. } => {
                    let a = self.input_node(i, 0)?;
                    let b = self.port(i, "");
                    self.union(a, b);
                }
                Kind::Block { .. } => {
                    let w = self.wiring.remove(&i).unwrap_or_default();
                    let from = match w.power_ins.first() {
                        Some(&p) => self.input_node(i, p)?,
                        None => self.s,
                    };
                    for &p in w.power_ins.iter().skip(1) {
                        let n = self.input_node(i, p)?;
                        self.pin_in_nodes.push((i, p, n));
                    }
                    let to = match w.power_outs.first() {
                        Some(f) => self.port(i, f),
                        None => self.node(),
                    };
                    for f in w.power_outs.iter().skip(1) {
                        let n = self.port(i, f);
                        self.pin_out_nodes.push((i, f.clone(), n));
                    }
                    self.wiring.insert(i, w);
                    self.edges.push(Edge2 {
                        from,
                        to,
                        t: Tree::Leaf(i),
                    });
                }
                Kind::InVar { .. } | Kind::OutVar { .. } | Kind::InOutVar { .. } => {
                    // Data: every reader / writer must be a block pin.
                    let e = &self.elems[i];
                    if let Kind::OutVar { .. } | Kind::InOutVar { .. } = e.kind {
                        for c in &e.ins[0].conns {
                            let (j, _) = self.source(c)?;
                            if !matches!(self.elems[j].kind, Kind::Block { .. }) {
                                return Err(format!("{} is fed by power flow", e.label()));
                            }
                        }
                    }
                    if let Kind::InVar { .. } | Kind::InOutVar { .. } = e.kind {
                        let readers = self
                            .consumers
                            .get(&(i, String::new()))
                            .cloned()
                            .unwrap_or_default();
                        for (k, _) in readers {
                            if !matches!(self.elems[k].kind, Kind::Block { .. }) {
                                return Err(format!(
                                    "{} feeds {}",
                                    e.label(),
                                    self.elems[k].label()
                                ));
                            }
                        }
                    }
                }
                Kind::Continuation { .. } => {}
                Kind::Label { .. } => return Err("a label inside a network".into()),
                Kind::LeftRail | Kind::RightRail => {}
            }
        }
        // Every open end (an output nothing reads) and the right rail: T.
        let froms: Vec<usize> = self.edges.iter().map(|e| e.from).collect();
        let starts: HashSet<usize> = froms.into_iter().map(|n| self.find(n)).collect();
        let pins: Vec<usize> = self
            .pin_in_nodes
            .iter()
            .map(|x| x.2)
            .chain(self.pin_out_nodes.iter().map(|x| x.2))
            .collect();
        let protected: HashSet<usize> = pins.into_iter().map(|n| self.find(n)).collect();
        let ends: Vec<usize> = self.edges.iter().map(|e| e.to).collect();
        for n in ends {
            let r = self.find(n);
            if !starts.contains(&r) && !protected.contains(&r) && r != self.s {
                let t = self.t;
                self.union(r, t);
            }
        }
        // Normalize edge ends.
        for k in 0..self.edges.len() {
            let (f, t) = (self.edges[k].from, self.edges[k].to);
            self.edges[k].from = self.find(f);
            self.edges[k].to = self.find(t);
        }
        let mut terminals: HashSet<usize> = [self.s, self.t].into_iter().collect();
        let pin_in: Vec<(usize, usize, usize)> = self
            .pin_in_nodes
            .clone()
            .into_iter()
            .map(|(b, p, n)| (b, p, self.find(n)))
            .collect();
        let pin_out: Vec<(usize, String, usize)> = self
            .pin_out_nodes
            .clone()
            .into_iter()
            .map(|(b, f, n)| (b, f, self.find(n)))
            .collect();
        terminals.extend(pin_in.iter().map(|x| x.2));
        terminals.extend(pin_out.iter().map(|x| x.2));
        self.reduce(&terminals)?;
        // Pin paths.
        let mut pin_rungs: HashMap<(usize, String), Vec<Element>> = HashMap::new();
        for (b, p, n) in &pin_in {
            let formal = self.elems[*b].ins[*p].formal.to_ascii_uppercase();
            let path = if *n == self.s {
                Vec::new()
            } else {
                let t = self.take_edge(self.s, *n)?;
                self.elements(t)
            };
            pin_rungs.insert((*b, formal), path);
        }
        for (b, f, n) in &pin_out {
            let t = self.take_edge(*n, self.t)?;
            let path = self.elements(t);
            pin_rungs.insert((*b, f.clone()), path);
        }
        let main = if self.edges.is_empty() {
            Vec::new()
        } else {
            let t = self.take_edge(self.s, self.t)?;
            self.elements_with(t, &mut pin_rungs)
        };
        if !self.edges.is_empty() {
            return Err("parts of the network are not connected to the rung".into());
        }
        let rail = self.rail.filter(|r| *r != u32::MAX);
        Ok((main, rail))
    }

    fn take_edge(&mut self, from: usize, to: usize) -> R<Tree> {
        let k = self
            .edges
            .iter()
            .position(|e| e.from == from && e.to == to)
            .ok_or("the network does not reduce to series and parallel connections")?;
        Ok(self.edges.remove(k).t)
    }

    fn reduce(&mut self, terminals: &HashSet<usize>) -> R<()> {
        loop {
            let mut changed = false;
            // Parallel: same ends.
            let mut i = 0;
            while i < self.edges.len() {
                let (f, t) = (self.edges[i].from, self.edges[i].to);
                if f == t {
                    return Err("a wire loops back on itself".into());
                }
                let same: Vec<usize> = (i + 1..self.edges.len())
                    .filter(|&j| self.edges[j].from == f && self.edges[j].to == t)
                    .collect();
                if !same.is_empty() {
                    let mut legs = vec![std::mem::replace(
                        &mut self.edges[i].t,
                        Tree::Ser(Vec::new()),
                    )];
                    for &j in same.iter().rev() {
                        legs.push(self.edges.remove(j).t);
                    }
                    let mut flat = Vec::new();
                    for l in legs {
                        match l {
                            Tree::Par(v) => flat.extend(v),
                            other => flat.push(other),
                        }
                    }
                    let elems = self.elems;
                    flat.sort_by(|a, b| {
                        a.min_y(elems)
                            .partial_cmp(&b.min_y(elems))
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                    self.edges[i].t = Tree::Par(flat);
                    changed = true;
                }
                i += 1;
            }
            // Series: an inner node with one edge in and one out.
            let mut nodes: Vec<usize> = self.edges.iter().flat_map(|e| [e.from, e.to]).collect();
            nodes.sort_unstable();
            nodes.dedup();
            for n in nodes {
                if terminals.contains(&n) {
                    continue;
                }
                let ins: Vec<usize> = (0..self.edges.len())
                    .filter(|&k| self.edges[k].to == n)
                    .collect();
                let outs: Vec<usize> = (0..self.edges.len())
                    .filter(|&k| self.edges[k].from == n)
                    .collect();
                if let ([a], [b]) = (ins.as_slice(), outs.as_slice()) {
                    let (a, b) = (*a, *b);
                    let second = self.edges[b].t.clone();
                    let to = self.edges[b].to;
                    let first = std::mem::replace(&mut self.edges[a].t, Tree::Ser(Vec::new()));
                    let mut v = Vec::new();
                    for t in [first, second] {
                        match t {
                            Tree::Ser(x) => v.extend(x),
                            other => v.push(other),
                        }
                    }
                    self.edges[a].t = Tree::Ser(v);
                    self.edges[a].to = to;
                    self.edges.remove(b);
                    changed = true;
                    break;
                }
                if ins.is_empty() && !outs.is_empty() {
                    return Err("an element is not powered from the left rail".into());
                }
            }
            if !changed {
                return Ok(());
            }
        }
    }

    fn elements(&mut self, t: Tree) -> Vec<Element> {
        let mut none = HashMap::new();
        self.elements_with(t, &mut none)
    }

    fn elements_with(
        &mut self,
        t: Tree,
        pin_rungs: &mut HashMap<(usize, String), Vec<Element>>,
    ) -> Vec<Element> {
        match t {
            Tree::Ser(v) => v
                .into_iter()
                .flat_map(|x| self.elements_with(x, pin_rungs))
                .collect(),
            Tree::Par(v) => {
                let legs = v
                    .into_iter()
                    .map(|x| self.elements_with(x, pin_rungs))
                    .collect();
                vec![Element::Branch(m::Branch {
                    id: 0,
                    legs,
                    src: None,
                })]
            }
            Tree::Leaf(i) => vec![self.leaf(i, pin_rungs)],
        }
    }

    fn leaf(
        &mut self,
        i: usize,
        pin_rungs: &mut HashMap<(usize, String), Vec<Element>>,
    ) -> Element {
        let e = &self.elems[i];
        let id = self.ids.claim(Some(e.id));
        match &e.kind {
            Kind::Contact { var, negated, edge } => Element::Contact(m::Contact {
                id,
                operand: plcc_st::print_expression(var),
                kind: match (edge, negated) {
                    (Edge::Rising, _) => m::ContactKind::Rising,
                    (Edge::Falling, _) => m::ContactKind::Falling,
                    (Edge::None, true) => m::ContactKind::Nc,
                    (Edge::None, false) => m::ContactKind::No,
                },
                notes: Vec::new(),
                src: None,
            }),
            Kind::Coil {
                var,
                negated,
                storage,
                edge,
            } => Element::Coil(m::Coil {
                id,
                operand: plcc_st::print_expression(var),
                kind: match (storage, edge, negated) {
                    (Storage::Set, _, _) => m::CoilKind::Set,
                    (Storage::Reset, _, _) => m::CoilKind::Reset,
                    (_, Edge::Rising, _) => m::CoilKind::Rising,
                    (_, Edge::Falling, _) => m::CoilKind::Falling,
                    (_, _, true) => m::CoilKind::Negated,
                    _ => m::CoilKind::Normal,
                },
                notes: Vec::new(),
                src: None,
            }),
            Kind::Jump { label } => Element::Jump(m::Jump {
                id,
                label: label.clone(),
                src: None,
            }),
            Kind::Return => Element::Return(m::Return { id, src: None }),
            Kind::Block {
                type_name,
                instance,
                inouts,
            } => {
                if instance.is_none()
                    && let Some((code, notes)) =
                        self.st_actions.get(&type_name.name.to_ascii_uppercase())
                    && type_name
                        .name
                        .to_ascii_uppercase()
                        .starts_with(ST_BOX_ACTION)
                {
                    return Element::St(m::StBox {
                        id,
                        code: code.clone(),
                        notes: notes.clone(),
                    });
                }
                let w = self.wiring.remove(&i).unwrap_or_default();
                let mut pins = Vec::new();
                for (p, pin) in e.ins.iter().enumerate() {
                    let up = pin.formal.to_ascii_uppercase();
                    pins.push(m::Pin {
                        name: pin.formal.clone(),
                        dir: if inouts.contains(&up) {
                            m::PinDir::InOut
                        } else {
                            m::PinDir::Input
                        },
                        value: w.data_in.get(&p).cloned(),
                        negated: pin.negated,
                        rung: pin_rungs.remove(&(i, up)),
                        src: None,
                    });
                }
                for o in &e.outs {
                    let up = o.formal.to_ascii_uppercase();
                    pins.push(m::Pin {
                        name: o.formal.clone(),
                        dir: m::PinDir::Output,
                        value: w.data_out.get(&up).cloned(),
                        negated: o.negated,
                        rung: pin_rungs.remove(&(i, up)),
                        src: None,
                    });
                }
                Element::Block(m::Block {
                    id,
                    name: type_name.name.clone(),
                    instance: instance.as_ref().map(|n| n.name.clone()),
                    power_in: w.power_ins.first().map(|&p| e.ins[p].formal.clone()),
                    power_out: w.power_outs.first().and_then(|f| {
                        e.outs
                            .iter()
                            .find(|o| o.formal.eq_ignore_ascii_case(f))
                            .map(|o| o.formal.clone())
                    }),
                    pins,
                    notes: Vec::new(),
                    src: None,
                })
            }
            _ => Element::St(m::StBox {
                id,
                code: String::new(),
                notes: vec![format!("unexpected {}", e.label())],
            }),
        }
    }
}
