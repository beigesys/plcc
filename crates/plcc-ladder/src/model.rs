// SPDX-License-Identifier: MPL-2.0

//! The ladder model: one program's rungs as a series/parallel tree of
//! elements, independent of the notation it came from (PLCopen LD, Rockwell
//! rung text) and serializable as JSON for an editor.
//!
//! * A [`Project`] holds [`Pou`]s (programs, function blocks, functions;
//!   Logix programs), each with [`Variable`]s and [`Routine`]s (one per IEC
//!   body; any number in Logix).
//! * A [`Routine`] is a list of [`Rung`]s. A rung is a *series* of
//!   [`Element`]s from the left rail to the right: power flows left to
//!   right; a [`Branch`] holds parallel legs (each a series; an empty leg is a
//!   short) whose results are ORed where they join.
//! * Every element, rung, routine and POU has an [`Id`], unique in the
//!   project. Readers assign them (PLCopen: the element's `localId` where it is
//!   free); writers keep them (PLCopen `localId`s), so an editor can refer to an
//!   element across a save and a reload.
//! * Operands, pin values and ST-box code are plain text: ST expressions in
//!   the IEC dialect, Logix operands (tag paths, immediates, CPT expressions)
//!   in the Logix dialect.
//!
//! What the elements *mean* depends on [`Dialect`]; `docs/ladder-translation.md`
//! has the semantics and the IEC ↔ Logix mapping.

use serde::{Deserialize, Serialize};
use std::ops::Range;

/// Element identifier, unique in a [`Project`].
pub type Id = u32;

/// Whose ladder semantics the model follows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dialect {
    /// IEC 61131-3 LD (PLCopen XML; CODESYS, OpenPLC, Beremiz): blocks are
    /// function blocks and functions; a false rung does not run a block
    /// wired through EN.
    #[default]
    Iec,
    /// Rockwell Logix 5000 RLL: every instruction runs on every scan with the
    /// rung condition (a false rung clears an OTE, resets a TON); blocks are
    /// Logix instructions with positional operands.
    Logix,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub dialect: Dialect,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// VAR_GLOBAL / Logix controller tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub globals: Vec<Variable>,
    pub pous: Vec<Pou>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PouKind {
    #[default]
    Program,
    FunctionBlock,
    Function,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pou {
    pub id: Id,
    pub name: String,
    #[serde(default)]
    pub kind: PouKind,
    /// A FUNCTION's result type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_type: Option<String>,
    /// Interface and local variables / Logix program tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variables: Vec<Variable>,
    pub routines: Vec<Routine>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VarSection {
    #[default]
    Local,
    Input,
    Output,
    InOut,
    External,
    Temp,
    Global,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Variable {
    pub name: String,
    /// IEC type (`BOOL`, `TON`, `ARRAY[0..9] OF INT`) or Logix data type
    /// (`BOOL`, `TIMER`, `DINT[10]`).
    pub data_type: String,
    #[serde(default)]
    pub section: VarSection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial: Option<String>,
    /// IEC direct address (`%IX0.0`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub constant: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub retain: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Routine {
    pub id: Id,
    pub name: String,
    pub rungs: Vec<Rung>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rung {
    pub id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// A jump target (PLCopen `<label>` before the network, Logix `LBL` as the
    /// rung's first instruction).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The rung, left to right.
    pub elements: Vec<Element>,
    /// Where the label came from (the `LBL` instruction of rung text).
    #[serde(skip)]
    pub label_src: Option<Src>,
}

/// One element of a rung.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Element {
    Contact(Contact),
    Coil(Coil),
    Branch(Branch),
    Block(Block),
    Jump(Jump),
    Return(Return),
    /// A block of Structured Text statements, for what cannot be drawn.
    St(StBox),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContactKind {
    /// Normally open: `--| |--` (Logix XIC).
    #[default]
    No,
    /// Normally closed: `--|/|--` (Logix XIO).
    Nc,
    /// Positive transition of the operand: `--|P|--` (IEC only).
    Rising,
    /// Negative transition of the operand: `--|N|--` (IEC only).
    Falling,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Contact {
    pub id: Id,
    pub operand: String,
    #[serde(default)]
    pub kind: ContactKind,
    /// Semantic notes (translation warnings, dialect differences).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    #[serde(skip)]
    pub src: Option<Src>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoilKind {
    /// `--( )--`: operand := power (Logix OTE).
    #[default]
    Normal,
    /// `--(/)--`: operand := NOT power (IEC only).
    Negated,
    /// `--(S)--`: set when powered (Logix OTL).
    Set,
    /// `--(R)--`: reset when powered (Logix OTU).
    Reset,
    /// `--(P)--`: operand := rising edge of the power (IEC only).
    Rising,
    /// `--(N)--`: operand := falling edge of the power (IEC only).
    Falling,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Coil {
    pub id: Id,
    pub operand: String,
    #[serde(default)]
    pub kind: CoilKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    #[serde(skip)]
    pub src: Option<Src>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Branch {
    pub id: Id,
    /// Parallel legs, top to bottom; each a series. An empty leg passes the
    /// power at the branch straight to the join.
    pub legs: Vec<Vec<Element>>,
    #[serde(skip)]
    pub src: Option<Src>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PinDir {
    #[default]
    Input,
    Output,
    InOut,
}

/// A named pin of a [`Block`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pin {
    /// The formal parameter (`IN`, `PT`, `Q`) or Logix operand name (`Timer`,
    /// `Source A`); Logix operands are positional, in pin order.
    pub name: String,
    #[serde(default)]
    pub dir: PinDir,
    /// Input: the expression wired to the pin. Output: the variable the output
    /// is written to. Logix: the operand text. `None`: unconnected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// The pin is inverted (IEC `negated` pin).
    #[serde(default, skip_serializing_if = "is_false")]
    pub negated: bool,
    /// An input fed by its own path from the left rail (IEC: a second
    /// power-flow input such as a counter's reset), or an output driving its
    /// own path to the right rail (a second power-flow output). The elements
    /// of that path, left to right.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rung: Option<Vec<Element>>,
    #[serde(skip)]
    pub src: Option<Range<usize>>,
}

/// A function block, function or instruction box.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub id: Id,
    /// FB type, function or instruction name (`TON`, `ADD`, `MOV`, `CPT`).
    pub name: String,
    /// IEC function block instance (`T1`). Logix instructions name their
    /// backing tag as an operand instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    #[serde(default)]
    pub pins: Vec<Pin>,
    /// IEC: the input pin the rung's power enters (`EN`, `IN`, `CU`); `None`:
    /// the block takes no power (its output is ANDed into the rung). Unused in
    /// the Logix dialect, where the rung condition enters every instruction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_in: Option<String>,
    /// IEC: the output pin that continues the rung (`ENO`, `Q`); `None`: the
    /// rung continues with the power that entered. Unused in the Logix dialect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_out: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    #[serde(skip)]
    pub src: Option<Src>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Jump {
    pub id: Id,
    pub label: String,
    #[serde(skip)]
    pub src: Option<Src>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Return {
    pub id: Id,
    #[serde(skip)]
    pub src: Option<Src>,
}

/// Structured Text statements in a rung, run when the rung is powered; the
/// power passes through unchanged.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StBox {
    pub id: Id,
    pub code: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Where an element came from in its source text (not serialized): byte
/// ranges into the text the reader was given (a Logix rung's neutral text).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Src {
    /// The whole element.
    pub span: Range<usize>,
    /// The instruction mnemonic.
    pub name: Range<usize>,
    /// Each operand, trimmed.
    pub operands: Vec<Range<usize>>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Element {
    pub fn id(&self) -> Id {
        match self {
            Element::Contact(e) => e.id,
            Element::Coil(e) => e.id,
            Element::Branch(e) => e.id,
            Element::Block(e) => e.id,
            Element::Jump(e) => e.id,
            Element::Return(e) => e.id,
            Element::St(e) => e.id,
        }
    }

    fn id_mut(&mut self) -> &mut Id {
        match self {
            Element::Contact(e) => &mut e.id,
            Element::Coil(e) => &mut e.id,
            Element::Branch(e) => &mut e.id,
            Element::Block(e) => &mut e.id,
            Element::Jump(e) => &mut e.id,
            Element::Return(e) => &mut e.id,
            Element::St(e) => &mut e.id,
        }
    }
}

/// Visit every element of a series (depth first, left to right, legs top to
/// bottom, pin paths included).
pub fn walk<'a>(series: &'a [Element], f: &mut impl FnMut(&'a Element)) {
    for e in series {
        f(e);
        match e {
            Element::Branch(b) => {
                for l in &b.legs {
                    walk(l, f);
                }
            }
            Element::Block(b) => {
                for p in &b.pins {
                    if let Some(r) = &p.rung {
                        walk(r, f);
                    }
                }
            }
            _ => {}
        }
    }
}

fn walk_mut(series: &mut [Element], f: &mut impl FnMut(&mut Element)) {
    for e in series {
        f(e);
        match e {
            Element::Branch(b) => {
                for l in &mut b.legs {
                    walk_mut(l, f);
                }
            }
            Element::Block(b) => {
                for p in &mut b.pins {
                    if let Some(r) = &mut p.rung {
                        walk_mut(r, f);
                    }
                }
            }
            _ => {}
        }
    }
}

impl Project {
    /// The largest id in use (0 for an empty project).
    pub fn max_id(&self) -> Id {
        let mut m = 0;
        for p in &self.pous {
            m = m.max(p.id);
            for r in &p.routines {
                m = m.max(r.id);
                for g in &r.rungs {
                    m = m.max(g.id);
                    walk(&g.elements, &mut |e| m = m.max(e.id()));
                }
            }
        }
        m
    }

    /// Give every POU, routine, rung and element a fresh id, 1, 2, 3, ... in
    /// document order.
    pub fn renumber(&mut self) {
        let mut next = 0;
        let mut fresh = || {
            next += 1;
            next
        };
        for p in &mut self.pous {
            p.id = fresh();
            for r in &mut p.routines {
                r.id = fresh();
                for g in &mut r.rungs {
                    g.id = fresh();
                    walk_mut(&mut g.elements, &mut |e| *e.id_mut() = fresh());
                }
            }
        }
    }

    /// The project as pretty JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn from_json(text: &str) -> Result<Project, serde_json::Error> {
        serde_json::from_str(text)
    }
}

/// Hands out unique ids: wanted ones when free, fresh ones otherwise.
#[derive(Clone, Debug, Default)]
pub struct Ids {
    next: Id,
    used: std::collections::HashSet<Id>,
    /// Ids that will be claimed later: [`Ids::fresh`] does not hand them out.
    reserved: std::collections::HashSet<Id>,
}

impl Ids {
    pub fn new() -> Self {
        Ids::default()
    }

    /// `want` when it is free, else a fresh id.
    pub fn claim(&mut self, want: Option<Id>) -> Id {
        if let Some(w) = want
            && w > 0
            && self.used.insert(w)
        {
            return w;
        }
        self.fresh()
    }

    pub fn fresh(&mut self) -> Id {
        loop {
            self.next += 1;
            if !self.reserved.contains(&self.next) && self.used.insert(self.next) {
                return self.next;
            }
        }
    }

    /// Keep `id` for a later [`Ids::claim`].
    pub fn reserve(&mut self, id: Id) {
        self.reserved.insert(id);
    }
}
