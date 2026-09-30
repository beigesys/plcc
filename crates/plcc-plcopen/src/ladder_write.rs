// SPDX-License-Identifier: MPL-2.0

//! The ladder model (IEC dialect) → PLCopen XML (TC6 XML v2.01), with a
//! generated layout, so CODESYS, OpenPLC Editor and Beremiz can open it.
//!
//! **Layout.** Each rung is a band below the previous one. Inside a band the
//! series/parallel tree is laid out on a grid: a series left to right, a
//! branch's legs one below the other; a contact or coil takes one cell, a
//! block two columns and as many rows as its pins need. The band starts with
//! a left power rail and ends with a right power rail; wires carry their end
//! points (`<position>`s of each `<connection>`) with an orthogonal bend, and
//! every connection point its `<relPosition>`. A pin path (`Pin::rung`) is laid
//! out in rows below the rung; a data pin's value becomes an `inVariable` /
//! `outVariable` next to the pin; a rung comment a `<comment>` above the band.
//!
//! **Ids.** Elements keep their model id as `localId`; a rung's left rail gets
//! the rung's id; everything else (right rails, variables, labels, comments)
//! takes ids above the largest model id. [`crate::ladder_read`] maps them back,
//! so model → XML → model gives the same model (see its tests).
//!
//! **ST boxes** cannot be drawn in LD: each becomes an action of the POU named
//! `LD_ST_<id>` holding the code, called by a box of that name through EN/ENO
//! (the reader turns it back into the ST box). A FUNCTION has no actions, so an
//! ST box in one is an error. Further routines of a POU are written as actions
//! with LD bodies.

use crate::ladder_read::ST_BOX_ACTION;
use plcc_ladder::model::*;
use std::fmt::Write;

/// Why a model cannot be written.
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

const X0: i64 = 20;
const CW: i64 = 90; // column width
const RH: i64 = 40; // row height
const GAP: i64 = 30; // between bands
const CONTACT_W: i64 = 21;
const CONTACT_H: i64 = 20;
const BLOCK_W: i64 = 110;

/// Write a PLCopen project. The model must be in the IEC dialect. The
/// warnings name what PLCopen XML cannot hold (declarations and POU members
/// carried as ST).
pub fn write(project: &Project) -> Result<(String, Vec<String>), Vec<WriteError>> {
    if project.dialect != Dialect::Iec {
        return Err(vec![WriteError {
            element: 0,
            message: "PLCopen LD is IEC ladder: translate a Logix model to the IEC dialect first"
                .into(),
        }]);
    }
    let mut w = W {
        out: String::new(),
        next: project.max_id(),
        errors: Vec::new(),
        rail: (0, Vec::new()),
    };
    w.project(project);
    let mut warnings = Vec::new();
    for d in &project.declarations {
        let first = d.lines().next().unwrap_or("");
        warnings.push(format!(
            "not written to PLCopen XML (declarations are not in the ladder model): {first}"
        ));
    }
    for p in project.pous.iter().filter(|p| !p.members.is_empty()) {
        warnings.push(format!(
            "{}: methods / properties / actions are not written to PLCopen XML",
            p.name
        ));
    }
    if w.errors.is_empty() {
        Ok((w.out, warnings))
    } else {
        Err(w.errors)
    }
}

struct W {
    out: String,
    next: Id,
    errors: Vec<WriteError>,
    /// The left rail of the rung being written, and the y of each of its
    /// connection points.
    rail: (Id, Vec<i64>),
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn cdata(s: &str) -> String {
    format!("<![CDATA[{}]]>", s.replace("]]>", "]]]]><![CDATA[>"))
}

/// An output port: element id and formal parameter ("" for single outputs),
/// and its absolute point.
#[derive(Clone, Debug)]
struct Port {
    id: Id,
    formal: String,
    at: (i64, i64),
}

/// Laid-out size of a tree, in grid cells.
#[derive(Clone, Copy)]
struct Size {
    cols: i64,
    rows: i64,
}

impl W {
    /// A connection point of the current rung's left rail on the row at `row`.
    fn rail_port(&mut self, row: i64) -> Port {
        let y = row + 10;
        if !self.rail.1.contains(&y) {
            self.rail.1.push(y);
        }
        Port {
            id: self.rail.0,
            formal: String::new(),
            at: (X0 + 3, y),
        }
    }

    fn fresh(&mut self) -> Id {
        self.next += 1;
        self.next
    }

    fn err(&mut self, element: Id, message: impl Into<String>) {
        self.errors.push(WriteError {
            element,
            message: message.into(),
        });
    }

    fn project(&mut self, p: &Project) {
        let name = if p.name.is_empty() { "plcc" } else { &p.name };
        self.out
            .push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
        self.out.push_str(
            "<project xmlns=\"http://www.plcopen.org/xml/tc6_0201\" xmlns:xhtml=\"http://www.w3.org/1999/xhtml\">\n",
        );
        self.out.push_str(
            "  <fileHeader companyName=\"plcc\" productName=\"plcc\" productVersion=\"0.1\" creationDateTime=\"2026-01-01T00:00:00\"/>\n",
        );
        let _ = writeln!(self.out, "  <contentHeader name=\"{}\">", esc(name));
        self.out.push_str("    <coordinateInfo>\n      <fbd><scaling x=\"1\" y=\"1\"/></fbd>\n      <ld><scaling x=\"1\" y=\"1\"/></ld>\n      <sfc><scaling x=\"1\" y=\"1\"/></sfc>\n    </coordinateInfo>\n  </contentHeader>\n");
        self.out
            .push_str("  <types>\n    <dataTypes/>\n    <pous>\n");
        for pou in &p.pous {
            self.pou(pou);
        }
        self.out
            .push_str("    </pous>\n  </types>\n  <instances>\n    <configurations>\n");
        if !p.globals.is_empty() {
            self.out.push_str("      <configuration name=\"Config0\">\n        <resource name=\"Res0\">\n          <task name=\"MainTask\" interval=\"T#20ms\" priority=\"0\">\n");
            for pou in p.pous.iter().filter(|q| q.kind == PouKind::Program) {
                let _ = writeln!(
                    self.out,
                    "            <pouInstance name=\"{}\" typeName=\"{}\"/>",
                    esc(&pou.name),
                    esc(&pou.name)
                );
            }
            self.out
                .push_str("          </task>\n        </resource>\n");
            self.out.push_str("        <globalVars>\n");
            for v in &p.globals {
                self.variable(v, 10);
            }
            self.out
                .push_str("        </globalVars>\n      </configuration>\n");
        }
        self.out
            .push_str("    </configurations>\n  </instances>\n</project>\n");
    }

    fn variable(&mut self, v: &Variable, indent: usize) {
        let pad = " ".repeat(indent);
        let mut attrs = format!("name=\"{}\"", esc(&v.name));
        if let Some(a) = &v.address {
            let _ = write!(attrs, " address=\"{}\"", esc(a));
        }
        let _ = writeln!(self.out, "{pad}<variable {attrs}>");
        let _ = writeln!(self.out, "{pad}  <type>{}</type>", type_xml(&v.data_type));
        if let Some(i) = &v.initial {
            let _ = writeln!(
                self.out,
                "{pad}  <initialValue>{}</initialValue>",
                value_xml(i)
            );
        }
        if let Some(c) = &v.comment {
            let _ = writeln!(
                self.out,
                "{pad}  <documentation><xhtml:p>{}</xhtml:p></documentation>",
                cdata(c)
            );
        }
        let _ = writeln!(self.out, "{pad}</variable>");
    }

    fn pou(&mut self, pou: &Pou) {
        let ty = match pou.kind {
            PouKind::Program => "program",
            PouKind::FunctionBlock => "functionBlock",
            PouKind::Function => "function",
        };
        let _ = writeln!(
            self.out,
            "      <pou name=\"{}\" pouType=\"{ty}\">",
            esc(&pou.name)
        );
        self.out.push_str("        <interface>\n");
        if pou.kind == PouKind::Function {
            let rt = pou.return_type.as_deref().unwrap_or("BOOL");
            let _ = writeln!(
                self.out,
                "          <returnType>{}</returnType>",
                type_xml(rt)
            );
        }
        for (section, tag) in [
            (VarSection::Input, "inputVars"),
            (VarSection::Output, "outputVars"),
            (VarSection::InOut, "inOutVars"),
            (VarSection::External, "externalVars"),
            (VarSection::Global, "globalVars"),
            (VarSection::Local, "localVars"),
            (VarSection::Temp, "tempVars"),
        ] {
            for (constant, retain) in [(false, false), (true, false), (false, true)] {
                let vars: Vec<&Variable> = pou
                    .variables
                    .iter()
                    .filter(|v| {
                        v.section == section && v.constant == constant && v.retain == retain
                    })
                    .collect();
                if vars.is_empty() {
                    continue;
                }
                let mut attrs = String::new();
                if constant {
                    attrs.push_str(" constant=\"true\"");
                }
                if retain {
                    attrs.push_str(" retain=\"true\"");
                }
                let _ = writeln!(self.out, "          <{tag}{attrs}>");
                for v in vars {
                    self.variable(v, 12);
                }
                let _ = writeln!(self.out, "          </{tag}>");
            }
        }
        self.out.push_str("        </interface>\n");
        // Actions: further routines, then ST boxes.
        let mut st_boxes = Vec::new();
        for r in pou.routines.iter().filter(|r| st_only(r).is_none()) {
            for g in &r.rungs {
                walk(&g.elements, &mut |e| {
                    if let Element::St(s) = e {
                        st_boxes.push(s.clone());
                    }
                });
            }
        }
        if pou.kind == PouKind::Function && (!st_boxes.is_empty() || pou.routines.len() > 1) {
            self.err(
                pou.id,
                format!(
                    "FUNCTION {}: ST boxes and extra routines are written as actions, which a FUNCTION cannot have",
                    pou.name
                ),
            );
        }
        if pou.routines.len() > 1 || !st_boxes.is_empty() {
            self.out.push_str("        <actions>\n");
            for r in pou.routines.iter().skip(1) {
                let _ = writeln!(
                    self.out,
                    "          <action name=\"{}\">\n            <body>",
                    esc(&r.name)
                );
                self.ld_body(r);
                self.out
                    .push_str("            </body>\n          </action>\n");
            }
            for s in &st_boxes {
                let doc = if s.notes.is_empty() {
                    String::new()
                } else {
                    format!(
                        "\n            <documentation><xhtml:p>{}</xhtml:p></documentation>",
                        cdata(&s.notes.join("\n"))
                    )
                };
                let _ = writeln!(
                    self.out,
                    "          <action name=\"{ST_BOX_ACTION}{}\">\n            <body>\n              <ST><xhtml:p>{}</xhtml:p></ST>\n            </body>{doc}\n          </action>",
                    s.id,
                    cdata(&s.code)
                );
            }
            self.out.push_str("        </actions>\n");
        }
        self.out.push_str("        <body>\n");
        match pou.routines.first() {
            // A routine that is one ST box (an ST POU read into the model) is
            // an ST body again.
            Some(r) if st_only(r).is_some() => {
                let code = st_only(r).unwrap_or_default();
                let _ = writeln!(
                    self.out,
                    "          <ST><xhtml:p>{}</xhtml:p></ST>",
                    cdata(&code)
                );
            }
            Some(r) => self.ld_body(r),
            None => self.out.push_str("          <LD/>\n"),
        }
        self.out.push_str("        </body>\n      </pou>\n");
    }

    fn ld_body(&mut self, r: &Routine) {
        self.out.push_str("          <LD>\n");
        let mut top = 20;
        for g in &r.rungs {
            top = self.rung(g, top) + GAP;
        }
        self.out.push_str("          </LD>\n");
    }

    fn el(&mut self, s: &str) {
        self.out.push_str("            ");
        self.out.push_str(s);
        self.out.push('\n');
    }

    /// Lay out and write one rung whose band starts at `top`; returns the
    /// band's bottom.
    fn rung(&mut self, g: &Rung, mut top: i64) -> i64 {
        if let Some(c) = &g.comment {
            let id = self.fresh();
            let lines = c.lines().count().max(1) as i64;
            let h = 10 + 16 * lines;
            self.el(&format!(
                "<comment localId=\"{id}\" height=\"{h}\" width=\"400\"><position x=\"{X0}\" y=\"{top}\"/><content><xhtml:p>{}</xhtml:p></content></comment>",
                cdata(c)
            ));
            top += h + 10;
        }
        let size = size_of(&g.elements);
        let rows = size.rows.max(1);
        let x_start = X0 + 40;
        // Label: beside the rail, on the rung's first row.
        if let Some(l) = &g.label {
            let id = self.fresh();
            self.el(&format!(
                "<label localId=\"{id}\" label=\"{}\" height=\"20\" width=\"60\"><position x=\"{}\" y=\"{top}\"/></label>",
                esc(l),
                X0 - 15
            ));
            if g.elements.is_empty() {
                return top + 20;
            }
        }
        let rail_id = if g.id > 0 { g.id } else { self.fresh() };
        self.rail = (rail_id, Vec::new());
        let rail = self.rail_port(top);
        let mut extra_row = top + rows * RH;
        let mut body = Vec::new();
        let outs = self.series(
            &g.elements,
            vec![rail],
            x_start,
            top,
            &mut extra_row,
            &mut body,
        );
        let bottom = extra_row.max(top + rows * RH);
        let rail_h = (bottom - top).max(40);
        let mut points = String::new();
        for y in std::mem::take(&mut self.rail.1) {
            let _ = write!(
                points,
                "<connectionPointOut formalParameter=\"\"><relPosition x=\"3\" y=\"{}\"/></connectionPointOut>",
                y - top
            );
        }
        self.el(&format!(
            "<leftPowerRail localId=\"{rail_id}\" height=\"{rail_h}\" width=\"3\"><position x=\"{X0}\" y=\"{top}\"/>{points}</leftPowerRail>"
        ));
        for b in body {
            self.el(&b);
        }
        // Right rail: every open end of the rung.
        let right_x = x_start + size.cols.max(1) * CW + 20;
        let rid = self.fresh();
        let mut pins = String::new();
        for o in &outs.1 {
            let _ = write!(
                pins,
                "<connectionPointIn><relPosition x=\"0\" y=\"{}\"/>{}</connectionPointIn>",
                o.at.1 - top,
                conn(o, (right_x, o.at.1))
            );
        }
        self.el(&format!(
            "<rightPowerRail localId=\"{rid}\" height=\"{rail_h}\" width=\"3\"><position x=\"{right_x}\" y=\"{top}\"/>{pins}</rightPowerRail>"
        ));
        bottom
    }

    /// Lay out a series from `x`, `y`; `ins` feed its first element. Returns
    /// (the ports at its right end that continue, ends that go to the right
    /// rail).
    fn series(
        &mut self,
        elems: &[Element],
        mut ins: Vec<Port>,
        mut x: i64,
        y: i64,
        extra_row: &mut i64,
        body: &mut Vec<String>,
    ) -> (Vec<Port>, Vec<Port>) {
        let mut ends = Vec::new();
        for e in elems {
            let (next, more_ends) = self.element(e, ins, x, y, extra_row, body);
            ends.extend(more_ends);
            ins = next;
            x += size_of(std::slice::from_ref(e)).cols * CW;
        }
        // What leaves the series continues; open ends go to the right rail.
        let mut all = ends;
        all.extend(ins.iter().cloned());
        (ins, all)
    }

    fn element(
        &mut self,
        e: &Element,
        ins: Vec<Port>,
        x: i64,
        y: i64,
        extra_row: &mut i64,
        body: &mut Vec<String>,
    ) -> (Vec<Port>, Vec<Port>) {
        let wire = y + 10;
        match e {
            Element::Contact(c) => {
                let (neg, edge) = match c.kind {
                    ContactKind::No => ("false", "none"),
                    ContactKind::Nc => ("true", "none"),
                    ContactKind::Rising => ("false", "rising"),
                    ContactKind::Falling => ("false", "falling"),
                };
                body.push(format!(
                    "<contact localId=\"{}\" negated=\"{neg}\" edge=\"{edge}\" height=\"{CONTACT_H}\" width=\"{CONTACT_W}\"><position x=\"{x}\" y=\"{y}\"/>{}<connectionPointOut><relPosition x=\"{CONTACT_W}\" y=\"10\"/></connectionPointOut><variable>{}</variable></contact>",
                    c.id,
                    cpi(&ins, (x, wire), 10),
                    esc(&c.operand)
                ));
                (
                    vec![Port {
                        id: c.id,
                        formal: String::new(),
                        at: (x + CONTACT_W, wire),
                    }],
                    Vec::new(),
                )
            }
            Element::Coil(c) => {
                let (neg, storage, edge) = match c.kind {
                    CoilKind::Normal => ("false", "none", "none"),
                    CoilKind::Negated => ("true", "none", "none"),
                    CoilKind::Set => ("false", "set", "none"),
                    CoilKind::Reset => ("false", "reset", "none"),
                    CoilKind::Rising => ("false", "none", "rising"),
                    CoilKind::Falling => ("false", "none", "falling"),
                };
                body.push(format!(
                    "<coil localId=\"{}\" negated=\"{neg}\" storage=\"{storage}\" edge=\"{edge}\" height=\"{CONTACT_H}\" width=\"{CONTACT_W}\"><position x=\"{x}\" y=\"{y}\"/>{}<connectionPointOut><relPosition x=\"{CONTACT_W}\" y=\"10\"/></connectionPointOut><variable>{}</variable></coil>",
                    c.id,
                    cpi(&ins, (x, wire), 10),
                    esc(&c.operand)
                ));
                (
                    vec![Port {
                        id: c.id,
                        formal: String::new(),
                        at: (x + CONTACT_W, wire),
                    }],
                    Vec::new(),
                )
            }
            Element::Jump(j) => {
                body.push(format!(
                    "<jump localId=\"{}\" label=\"{}\" height=\"20\" width=\"40\"><position x=\"{x}\" y=\"{y}\"/>{}</jump>",
                    j.id,
                    esc(&j.label),
                    cpi(&ins, (x, wire), 10)
                ));
                (Vec::new(), Vec::new())
            }
            Element::Return(r) => {
                body.push(format!(
                    "<return localId=\"{}\" height=\"20\" width=\"40\"><position x=\"{x}\" y=\"{y}\"/>{}</return>",
                    r.id,
                    cpi(&ins, (x, wire), 10)
                ));
                (Vec::new(), Vec::new())
            }
            Element::Branch(b) => {
                let mut outs = Vec::new();
                let mut ends = Vec::new();
                let mut row_y = y;
                for leg in &b.legs {
                    if leg.is_empty() {
                        outs.extend(ins.iter().cloned());
                        row_y += RH;
                        continue;
                    }
                    let (o, _) = self.series(leg, ins.clone(), x, row_y, extra_row, body);
                    // Coils and blocks at a leg's end may also be open ends;
                    // they join here like every other leg end.
                    outs.extend(o);
                    let _ = &mut ends;
                    row_y += size_of(leg).rows.max(1) * RH;
                }
                (outs, ends)
            }
            Element::St(s) => {
                let block = Block {
                    id: s.id,
                    name: format!("{ST_BOX_ACTION}{}", s.id),
                    instance: None,
                    pins: vec![
                        Pin {
                            name: "EN".into(),
                            ..Default::default()
                        },
                        Pin {
                            name: "ENO".into(),
                            dir: PinDir::Output,
                            ..Default::default()
                        },
                    ],
                    power_in: Some("EN".into()),
                    power_out: Some("ENO".into()),
                    notes: Vec::new(),
                    src: None,
                };
                self.block(&block, ins, x, y, extra_row, body)
            }
            Element::Block(b) => self.block(b, ins, x, y, extra_row, body),
        }
    }

    fn block(
        &mut self,
        b: &Block,
        ins: Vec<Port>,
        x: i64,
        y: i64,
        extra_row: &mut i64,
        body: &mut Vec<String>,
    ) -> (Vec<Port>, Vec<Port>) {
        let inputs: Vec<&Pin> = b.pins.iter().filter(|p| p.dir != PinDir::Output).collect();
        let outputs: Vec<&Pin> = b.pins.iter().filter(|p| p.dir == PinDir::Output).collect();
        let is = |p: &Pin, n: &Option<String>| {
            n.as_deref().is_some_and(|n| p.name.eq_ignore_ascii_case(n))
        };
        // Pin rows: the power pin on the wire.
        let k_in = inputs.iter().position(|p| is(p, &b.power_in)).unwrap_or(0) as i64;
        let pin_y = |k: i64| 30 + 20 * k;
        let by = y + 10 - pin_y(k_in);
        let by = by.max(y - 20);
        let n = inputs.len().max(outputs.len()) as i64;
        let h = 20 + 20 * n.max(1);
        let bx = x + 10;
        let mut xml = String::new();
        let _ = write!(
            xml,
            "<block localId=\"{}\" typeName=\"{}\"",
            b.id,
            esc(&b.name)
        );
        if let Some(i) = &b.instance {
            let _ = write!(xml, " instanceName=\"{}\"", esc(i));
        }
        let _ = write!(
            xml,
            " height=\"{h}\" width=\"{BLOCK_W}\"><position x=\"{bx}\" y=\"{by}\"/>"
        );
        let mut in_xml = String::new();
        let mut io_xml = String::new();
        for (k, p) in inputs.iter().enumerate() {
            let at = (bx, by + pin_y(k as i64));
            let rel = pin_y(k as i64);
            let neg = if p.negated { " negated=\"true\"" } else { "" };
            let feed: Vec<Port> = if is(p, &b.power_in) {
                ins.clone()
            } else if let Some(r) = &p.rung {
                // A path from the left rail, laid out below the rung.
                let row = *extra_row;
                *extra_row += size_of(r).rows.max(1) * RH;
                let rail = self.rail_port(row);
                let (o, _) = self.series(r, vec![rail], X0 + 40, row, extra_row, body);
                o
            } else if let Some(v) = &p.value {
                let vid = self.fresh();
                let vx = bx - 80;
                body.push(format!(
                    "<inVariable localId=\"{vid}\" height=\"20\" width=\"60\"><position x=\"{vx}\" y=\"{}\"/><connectionPointOut><relPosition x=\"60\" y=\"10\"/></connectionPointOut><expression>{}</expression></inVariable>",
                    at.1 - 10,
                    esc(v)
                ));
                vec![Port {
                    id: vid,
                    formal: String::new(),
                    at: (vx + 60, at.1),
                }]
            } else {
                Vec::new()
            };
            let point = if feed.is_empty() {
                format!("<connectionPointIn><relPosition x=\"0\" y=\"{rel}\"/></connectionPointIn>")
            } else {
                cpi(&feed, at, rel)
            };
            let v = format!(
                "<variable formalParameter=\"{}\"{neg}>{point}</variable>",
                esc(&p.name)
            );
            if p.dir == PinDir::InOut {
                io_xml.push_str(&v);
            } else {
                in_xml.push_str(&v);
            }
        }
        let mut out_xml = String::new();
        let mut cont = Vec::new();
        let mut ends = Vec::new();
        for (k, p) in outputs.iter().enumerate() {
            let at = (bx + BLOCK_W, by + pin_y(k as i64));
            let neg = if p.negated { " negated=\"true\"" } else { "" };
            let _ = write!(
                out_xml,
                "<variable formalParameter=\"{}\"{neg}><connectionPointOut><relPosition x=\"{BLOCK_W}\" y=\"{}\"/></connectionPointOut></variable>",
                esc(&p.name),
                pin_y(k as i64)
            );
            let port = Port {
                id: b.id,
                formal: p.name.clone(),
                at,
            };
            if is(p, &b.power_out) {
                cont.push(port.clone());
            }
            if let Some(v) = &p.value {
                let vid = self.fresh();
                let vx = at.0 + 20;
                body.push(format!(
                    "<outVariable localId=\"{vid}\" height=\"20\" width=\"60\"><position x=\"{vx}\" y=\"{}\"/>{}<expression>{}</expression></outVariable>",
                    at.1 - 10,
                    cpi(std::slice::from_ref(&port), (vx, at.1), 10),
                    esc(v)
                ));
            }
            if let Some(r) = &p.rung {
                let row = *extra_row;
                *extra_row += size_of(r).rows.max(1) * RH;
                let (_, e) = self.series(r, vec![port.clone()], at.0 + 30, row, extra_row, body);
                ends.extend(e);
            }
        }
        let _ = write!(
            xml,
            "<inputVariables>{in_xml}</inputVariables><inOutVariables>{io_xml}</inOutVariables><outputVariables>{out_xml}</outputVariables></block>"
        );
        body.push(xml);
        if b.power_out.is_none() {
            // The rung continues with the power that entered the block.
            return (ins, ends);
        }
        (cont, ends)
    }
}

/// The code of a routine that is a single, unlabelled rung holding only an
/// ST box.
fn st_only(r: &Routine) -> Option<String> {
    match r.rungs.as_slice() {
        [g] if g.label.is_none() && g.comment.is_none() => match g.elements.as_slice() {
            [Element::St(s)] => Some(s.code.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// `<connectionPointIn>` at `at` (absolute) fed by `from`.
fn cpi(from: &[Port], at: (i64, i64), rel_y: i64) -> String {
    let mut s = format!("<connectionPointIn><relPosition x=\"0\" y=\"{rel_y}\"/>");
    for p in from {
        s.push_str(&conn(p, at));
    }
    s.push_str("</connectionPointIn>");
    s
}

/// A `<connection>` from `p` to the input at `at`: the wire's points, input
/// end first, with one orthogonal bend.
fn conn(p: &Port, at: (i64, i64)) -> String {
    let formal = if p.formal.is_empty() {
        String::new()
    } else {
        format!(" formalParameter=\"{}\"", esc(&p.formal))
    };
    let mid = (at.0 - 10).max(p.at.0);
    let mut pts = vec![at];
    if p.at.1 != at.1 {
        pts.push((mid, at.1));
        pts.push((mid, p.at.1));
    }
    pts.push(p.at);
    let mut s = format!("<connection refLocalId=\"{}\"{formal}>", p.id);
    for (x, y) in pts {
        let _ = write!(s, "<position x=\"{x}\" y=\"{y}\"/>");
    }
    s.push_str("</connection>");
    s
}

fn size_of(elems: &[Element]) -> Size {
    let mut s = Size { cols: 0, rows: 1 };
    for e in elems {
        let (c, r) = match e {
            Element::Contact(_) | Element::Coil(_) | Element::Jump(_) | Element::Return(_) => {
                (1, 1)
            }
            Element::Block(b) => {
                let ins = b.pins.iter().filter(|p| p.dir != PinDir::Output).count() as i64;
                let outs = b.pins.iter().filter(|p| p.dir == PinDir::Output).count() as i64;
                (2, ((20 + 20 * ins.max(outs)) + RH - 1) / RH)
            }
            Element::St(_) => (2, 2),
            Element::Branch(b) => {
                let mut cols = 0;
                let mut rows = 0;
                for l in &b.legs {
                    let ls = size_of(l);
                    cols = cols.max(ls.cols);
                    rows += ls.rows.max(1);
                }
                (cols.max(1), rows.max(1))
            }
        };
        s.cols += c;
        s.rows = s.rows.max(r);
    }
    s
}

const ELEMENTARY: &[&str] = &[
    "BOOL", "BYTE", "WORD", "DWORD", "LWORD", "SINT", "INT", "DINT", "LINT", "USINT", "UINT",
    "UDINT", "ULINT", "REAL", "LREAL", "TIME", "DATE", "DT", "TOD",
];

/// A type (as ST text) as a PLCopen `<type>` body.
fn type_xml(text: &str) -> String {
    let (u, errs) = plcc_st::parse(&format!("TYPE x : {text}; END_TYPE"));
    let spec = match u.declarations.into_iter().next() {
        Some(plcc_st::Declaration::TypeDecl(t)) if errs.is_empty() => t.type_spec,
        _ => return format!("<derived name=\"{}\"/>", esc(text)),
    };
    spec_xml(&spec)
}

fn spec_xml(t: &plcc_st::TypeSpec) -> String {
    use plcc_st::TypeSpecKind as K;
    match &t.kind {
        K::Named(i) => {
            let up = i.name.to_ascii_uppercase();
            let up = match up.as_str() {
                "DATE_AND_TIME" => "DT".to_string(),
                "TIME_OF_DAY" => "TOD".to_string(),
                _ => up,
            };
            if ELEMENTARY.contains(&up.as_str()) {
                format!("<{up}/>")
            } else {
                format!("<derived name=\"{}\"/>", esc(&i.name))
            }
        }
        K::StringType { wide, length } => {
            let tag = if *wide { "wstring" } else { "string" };
            match length {
                Some(l) => format!("<{tag} length=\"{}\"/>", esc(&plcc_st::print_expression(l))),
                None => format!("<{tag}/>"),
            }
        }
        K::Array { ranges, base } => {
            let mut s = String::from("<array>");
            for r in ranges {
                let _ = write!(
                    s,
                    "<dimension lower=\"{}\" upper=\"{}\"/>",
                    esc(&plcc_st::print_expression(&r.low)),
                    esc(&plcc_st::print_expression(&r.high))
                );
            }
            let _ = write!(s, "<baseType>{}</baseType></array>", spec_xml(base));
            s
        }
        K::Pointer(b) => format!("<pointer><baseType>{}</baseType></pointer>", spec_xml(b)),
        K::Subrange { base, low, high } => {
            let signed = !base.name.to_ascii_uppercase().starts_with('U');
            let tag = if signed {
                "subrangeSigned"
            } else {
                "subrangeUnsigned"
            };
            format!(
                "<{tag}><range lower=\"{}\" upper=\"{}\"/><baseType>{}</baseType></{tag}>",
                esc(&plcc_st::print_expression(low)),
                esc(&plcc_st::print_expression(high)),
                spec_xml(&plcc_st::TypeSpec {
                    kind: K::Named(base.clone()),
                    span: t.span
                })
            )
        }
        _ => format!(
            "<derived name=\"{}\"/>",
            esc(&plcc_st::printer::print_type_spec(t))
        ),
    }
}

/// An initial value (ST text) as a PLCopen value.
fn value_xml(text: &str) -> String {
    let t = text.trim();
    if let Some(inner) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']'))
        && !inner.contains('(')
        && !inner.contains('[')
    {
        let mut s = String::from("<arrayValue>");
        for v in inner.split(',') {
            let _ = write!(
                s,
                "<value><simpleValue value=\"{}\"/></value>",
                esc(v.trim())
            );
        }
        s.push_str("</arrayValue>");
        return s;
    }
    format!("<simpleValue value=\"{}\"/>", esc(t))
}
