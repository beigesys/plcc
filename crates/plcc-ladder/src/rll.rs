// SPDX-License-Identifier: MPL-2.0

//! Rockwell rung neutral text (1756-RM014 "Neutral text for ladder
//! instructions"): `XIC(a)XIO(b)[OTE(c),OTL(d)]OTU(e);` ↔ the ladder model.
//!
//! **Syntax.** A rung is a sequence of instructions and branches ending in
//! `;`. A branch `[leg, leg, ...]` holds legs, each a sequence (possibly empty,
//! which passes the rung condition through). Operands are split at top-level
//! commas; brackets, parentheses and quotes inside an operand are respected.
//!
//! **Reading** ([`read`]): XIC/XIO become contacts, OTE/OTL/OTU coils, `JMP`
//! a jump, an operand-less `RET` a return, an `LBL` that starts the rung the
//! rung's label; every other instruction a [`Block`] whose pins are its
//! operands, named from the catalog (RM003 operand names). Operand text is
//! kept verbatim (trimmed); each element keeps its byte ranges in the text
//! ([`Src`]) for diagnostics.
//!
//! **Writing** ([`write`]) is canonical: operands separated by `,` with
//! nothing around it, and every branch leg followed by a space, the way Studio
//! 5000 exports rungs: `[XIC(Start) ,XIC(Motor) ]XIO(Stop)OTE(Motor);`.
//! Reading canonical text and writing it back gives the same text.

use crate::catalog;
use crate::model::*;
use std::ops::Range;

/// A syntax error in rung text, with its byte range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextError {
    pub message: String,
    pub span: Range<usize>,
}

/// One instruction as written.
#[derive(Clone, Debug)]
pub struct Instr {
    pub name: String,
    pub name_span: Range<usize>,
    /// Trimmed operand text ranges.
    pub operands: Vec<Range<usize>>,
    pub span: Range<usize>,
}

/// The syntax tree of rung text.
#[derive(Clone, Debug)]
pub enum RawElement {
    Instr(Instr),
    Branch(Vec<Vec<RawElement>>, Range<usize>),
}

struct P<'a> {
    s: &'a [u8],
    src: &'a str,
    pos: usize,
}

/// Parse all rungs in `text` (normally one, terminated by `;`).
pub fn parse(text: &str) -> Result<Vec<Vec<RawElement>>, TextError> {
    let mut p = P {
        s: text.as_bytes(),
        src: text,
        pos: 0,
    };
    let mut rungs = Vec::new();
    loop {
        p.ws();
        if p.pos >= p.s.len() {
            break;
        }
        let seq = p.seq(0)?;
        p.ws();
        if p.pos < p.s.len() && p.s[p.pos] == b';' {
            p.pos += 1;
        } else if p.pos < p.s.len() {
            return Err(TextError {
                message: format!(
                    "unexpected `{}` in rung",
                    p.src[p.pos..].chars().next().unwrap_or(' ')
                ),
                span: p.pos..p.pos + 1,
            });
        }
        rungs.push(seq);
    }
    Ok(rungs)
}

impl P<'_> {
    fn ws(&mut self) {
        while self.pos < self.s.len() && self.s[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn seq(&mut self, depth: usize) -> Result<Vec<RawElement>, TextError> {
        let mut out = Vec::new();
        loop {
            self.ws();
            if self.pos >= self.s.len() {
                if depth > 0 {
                    return Err(TextError {
                        message: "unterminated branch `[`".into(),
                        span: self.pos..self.pos,
                    });
                }
                return Ok(out);
            }
            match self.s[self.pos] {
                b';' | b',' | b']' => return Ok(out),
                b'[' => {
                    let start = self.pos;
                    self.pos += 1;
                    let mut legs = Vec::new();
                    loop {
                        legs.push(self.seq(depth + 1)?);
                        self.ws();
                        match self.s.get(self.pos) {
                            Some(b',') => self.pos += 1,
                            Some(b']') => {
                                self.pos += 1;
                                break;
                            }
                            _ => {
                                return Err(TextError {
                                    message: "expected `,` or `]` in branch".into(),
                                    span: self.pos..self.pos + 1,
                                });
                            }
                        }
                    }
                    out.push(RawElement::Branch(legs, start..self.pos));
                }
                c if c.is_ascii_alphabetic() || c == b'_' => {
                    out.push(RawElement::Instr(self.instr()?))
                }
                _ => {
                    return Err(TextError {
                        message: format!(
                            "unexpected `{}` in rung",
                            self.src[self.pos..].chars().next().unwrap_or(' ')
                        ),
                        span: self.pos..self.pos + 1,
                    });
                }
            }
        }
    }

    fn instr(&mut self) -> Result<Instr, TextError> {
        let start = self.pos;
        while self.pos < self.s.len()
            && (self.s[self.pos].is_ascii_alphanumeric() || self.s[self.pos] == b'_')
        {
            self.pos += 1;
        }
        let name_span = start..self.pos;
        let name = self.src[name_span.clone()].to_string();
        self.ws();
        if self.s.get(self.pos) != Some(&b'(') {
            return Err(TextError {
                message: format!("expected `(` after instruction `{name}`"),
                span: name_span,
            });
        }
        self.pos += 1;
        let mut operands = Vec::new();
        let mut depth = 0usize;
        let mut op_start = self.pos;
        loop {
            let Some(&c) = self.s.get(self.pos) else {
                return Err(TextError {
                    message: format!("unterminated operand list of `{name}`"),
                    span: start..self.pos,
                });
            };
            match c {
                b'\'' | b'"' => {
                    self.pos += 1;
                    while self.pos < self.s.len() && self.s[self.pos] != c {
                        if self.s[self.pos] == b'$' {
                            self.pos += 1;
                        }
                        self.pos += 1;
                    }
                }
                b'(' | b'[' => depth += 1,
                b')' | b']' if depth > 0 => depth -= 1,
                b')' => {
                    operands.push(trim(self.src, op_start..self.pos));
                    self.pos += 1;
                    break;
                }
                b',' if depth == 0 => {
                    operands.push(trim(self.src, op_start..self.pos));
                    op_start = self.pos + 1;
                }
                _ => {}
            }
            self.pos += 1;
        }
        // `NOP()` has no operands, not one empty one.
        if operands.len() == 1 && operands[0].is_empty() {
            operands.clear();
        }
        Ok(Instr {
            name,
            name_span,
            operands,
            span: start..self.pos,
        })
    }
}

fn trim(src: &str, r: Range<usize>) -> Range<usize> {
    let s = &src[r.clone()];
    let lead = s.len() - s.trim_start().len();
    let t = s.trim();
    r.start + lead..r.start + lead + t.len()
}

// ── Reading into the model ──

/// The instruction a model element read from rung text was written as (with
/// its byte ranges in that text); `None` for an element made elsewhere.
pub fn instr_of(e: &Element) -> Option<Instr> {
    let (name, src) = match e {
        Element::Contact(c) => (
            if c.kind == ContactKind::No {
                "XIC"
            } else {
                "XIO"
            },
            c.src.as_ref(),
        ),
        Element::Coil(c) => (
            match c.kind {
                CoilKind::Set => "OTL",
                CoilKind::Reset => "OTU",
                _ => "OTE",
            },
            c.src.as_ref(),
        ),
        Element::Jump(j) => ("JMP", j.src.as_ref()),
        Element::Return(r) => ("RET", r.src.as_ref()),
        Element::Block(b) => (b.name.as_str(), b.src.as_ref()),
        Element::Branch(_) | Element::St(_) => return None,
    };
    let src = src?;
    Some(Instr {
        name: name.to_string(),
        name_span: src.name.clone(),
        operands: src.operands.clone(),
        span: src.span.clone(),
    })
}

/// Rung text → model rungs (Logix dialect), one per `;`-terminated
/// sequence. Ids come from `ids`.
pub fn read(text: &str, ids: &mut Ids) -> Result<Vec<Rung>, TextError> {
    let raw = parse(text)?;
    Ok(raw
        .iter()
        .map(|seq| {
            let id = ids.fresh();
            let mut label = None;
            let mut label_src = None;
            let mut body: &[RawElement] = seq;
            if let Some(RawElement::Instr(first)) = seq.first()
                && first.name == "LBL"
                && first.operands.len() == 1
            {
                label = Some(text[first.operands[0].clone()].to_string());
                label_src = Some(src_of(first));
                body = &seq[1..];
            }
            Rung {
                id,
                comment: None,
                label,
                elements: series(text, body, ids),
                label_src,
            }
        })
        .collect())
}

fn series(text: &str, seq: &[RawElement], ids: &mut Ids) -> Vec<Element> {
    seq.iter().map(|e| element(text, e, ids)).collect()
}

fn src_of(i: &Instr) -> Src {
    Src {
        span: i.span.clone(),
        name: i.name_span.clone(),
        operands: i.operands.clone(),
    }
}

fn element(text: &str, e: &RawElement, ids: &mut Ids) -> Element {
    match e {
        RawElement::Branch(legs, r) => {
            let id = ids.fresh();
            Element::Branch(Branch {
                id,
                legs: legs.iter().map(|l| series(text, l, ids)).collect(),
                src: Some(Src {
                    span: r.clone(),
                    ..Default::default()
                }),
            })
        }
        RawElement::Instr(i) => {
            let id = ids.fresh();
            let up = i.name.to_ascii_uppercase();
            let op = |k: usize| text[i.operands[k].clone()].to_string();
            let one = i.operands.len() == 1;
            let src = Some(src_of(i));
            // Only the exact upper-case spelling maps to a contact or coil, so
            // writing the element back gives the same text.
            match up.as_str() {
                "XIC" | "XIO" if one && i.name == up => Element::Contact(Contact {
                    id,
                    operand: op(0),
                    kind: if up == "XIC" {
                        ContactKind::No
                    } else {
                        ContactKind::Nc
                    },
                    notes: Vec::new(),
                    src,
                }),
                "OTE" | "OTL" | "OTU" if one && i.name == up => Element::Coil(Coil {
                    id,
                    operand: op(0),
                    kind: match up.as_str() {
                        "OTE" => CoilKind::Normal,
                        "OTL" => CoilKind::Set,
                        _ => CoilKind::Reset,
                    },
                    notes: Vec::new(),
                    src,
                }),
                "JMP" if one && i.name == up => Element::Jump(Jump {
                    id,
                    label: op(0),
                    src,
                }),
                "RET" if i.operands.is_empty() && i.name == up => {
                    Element::Return(Return { id, src })
                }
                _ => Element::Block(Block {
                    id,
                    name: i.name.clone(),
                    instance: None,
                    pins: i
                        .operands
                        .iter()
                        .enumerate()
                        .map(|(k, r)| Pin {
                            name: catalog::logix_operand_name(&i.name, k),
                            dir: catalog::logix_operand_dir(&i.name, k),
                            value: Some(text[r.clone()].to_string()),
                            negated: false,
                            rung: None,
                            src: Some(r.clone()),
                        })
                        .collect(),
                    power_in: None,
                    power_out: None,
                    notes: Vec::new(),
                    src,
                }),
            }
        }
    }
}

// ── Writing ──

/// Why a model rung cannot be written as Logix rung text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteError {
    pub element: Id,
    pub message: String,
}

/// A rung as canonical neutral text, `;` included.
pub fn write(rung: &Rung) -> Result<String, WriteError> {
    let mut out = String::new();
    if let Some(l) = &rung.label {
        out.push_str(&format!("LBL({l})"));
    }
    write_series(&rung.elements, &mut out)?;
    out.push(';');
    Ok(out)
}

fn write_series(series: &[Element], out: &mut String) -> Result<(), WriteError> {
    for e in series {
        write_element(e, out)?;
    }
    Ok(())
}

fn write_element(e: &Element, out: &mut String) -> Result<(), WriteError> {
    let unsupported = |id: Id, what: &str| WriteError {
        element: id,
        message: format!("{what} has no Logix rung text"),
    };
    match e {
        Element::Contact(c) => {
            let m = match c.kind {
                ContactKind::No => "XIC",
                ContactKind::Nc => "XIO",
                ContactKind::Rising => return Err(unsupported(c.id, "a rising-edge contact")),
                ContactKind::Falling => return Err(unsupported(c.id, "a falling-edge contact")),
            };
            out.push_str(&format!("{m}({})", c.operand));
        }
        Element::Coil(c) => {
            let m = match c.kind {
                CoilKind::Normal => "OTE",
                CoilKind::Set => "OTL",
                CoilKind::Reset => "OTU",
                CoilKind::Negated => return Err(unsupported(c.id, "a negated coil")),
                CoilKind::Rising | CoilKind::Falling => {
                    return Err(unsupported(c.id, "an edge coil"));
                }
            };
            out.push_str(&format!("{m}({})", c.operand));
        }
        Element::Branch(b) => {
            out.push('[');
            for (k, leg) in b.legs.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                write_series(leg, out)?;
                out.push(' ');
            }
            out.push(']');
        }
        Element::Block(b) => {
            if b.instance.is_some() || b.pins.iter().any(|p| p.rung.is_some() || p.negated) {
                return Err(unsupported(b.id, &format!("IEC block {}", b.name)));
            }
            let ops: Vec<&str> = b
                .pins
                .iter()
                .map(|p| p.value.as_deref().unwrap_or("?"))
                .collect();
            out.push_str(&format!("{}({})", b.name, ops.join(",")));
        }
        Element::Jump(j) => out.push_str(&format!("JMP({})", j.label)),
        Element::Return(_) => out.push_str("RET()"),
        Element::St(s) => return Err(unsupported(s.id, "an ST box")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branches_and_operands() {
        let t = "XIC(a)XIO(b)[OTE(c),OTL(d[1,2]) ,]OTU(e); ";
        let r = parse(t).unwrap_or_else(|e| panic!("{}", e.message));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].len(), 4);
        let RawElement::Branch(legs, _) = &r[0][2] else {
            panic!()
        };
        assert_eq!(legs.len(), 3);
        assert!(legs[2].is_empty());
        let RawElement::Instr(otl) = &legs[1][0] else {
            panic!()
        };
        assert_eq!(&t[otl.operands[0].clone()], "d[1,2]");
        let r = parse("NOP();").unwrap_or_else(|e| panic!("{}", e.message));
        let RawElement::Instr(nop) = &r[0][0] else {
            panic!()
        };
        assert!(nop.operands.is_empty());
        let r = parse("TON(T1,?,?)CPT(x, (a+b)*2);").unwrap_or_else(|e| panic!("{}", e.message));
        let RawElement::Instr(ton) = &r[0][0] else {
            panic!()
        };
        assert_eq!(ton.operands.len(), 3);
    }

    #[test]
    fn canonical_text_round_trips() {
        for t in [
            "[XIC(Start) ,XIC(Motor) ]XIO(Stop)OTE(Motor);",
            "LBL(Top)XIC(a)[OTE(c) ,OTL(d[1,2]) , ]OTU(e);",
            "TON(T1,1000,0)CPT(x,(a + b) * 2)JMP(Top);",
            "XIC(a)[[XIC(b) ,XIO(c) ]OTE(d) ,RET() ];",
            "NOP();",
        ] {
            let mut ids = Ids::new();
            let rungs = read(t, &mut ids).unwrap();
            assert_eq!(rungs.len(), 1);
            assert_eq!(write(&rungs[0]).unwrap(), t);
        }
    }

    #[test]
    fn operands_are_named_from_the_catalog() {
        let mut ids = Ids::new();
        let r = read("TON(T1,500,0)ADD(a,b,c)JSR(Sub,1,x,y);", &mut ids).unwrap();
        let names = |k: usize| match &r[0].elements[k] {
            Element::Block(b) => b.pins.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
            _ => panic!(),
        };
        assert_eq!(names(0), ["Timer", "Preset", "Accum"]);
        assert_eq!(names(1), ["Source A", "Source B", "Dest"]);
        assert_eq!(
            names(2),
            ["Routine Name", "Input Count", "Parameter 1", "Parameter 2"]
        );
    }
}
