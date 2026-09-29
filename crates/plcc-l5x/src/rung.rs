// SPDX-License-Identifier: MPL-2.0

//! Rung neutral text (1756-RM014 "Define a ladder logic routine", "Neutral text
//! for ladder instructions"): `XIC(a)XIO(b)[OTE(c),OTL(d)]OTU(e);`.
//!
//! A rung is a sequence of instructions and branches. A branch `[leg, leg, ...]`
//! holds legs, each a sequence (possibly empty, which passes the rung condition
//! through). Operands are split at top-level commas; brackets, parentheses and
//! quotes inside an operand are respected. Operand text is parsed later, by
//! [`crate::operand`], once the instruction says what it expects.

use crate::operand::ParseErr;
use std::ops::Range;

#[derive(Clone, Debug)]
pub(crate) struct Instr {
    pub name: String,
    pub name_span: Range<usize>,
    /// Trimmed operand text ranges.
    pub operands: Vec<Range<usize>>,
    pub span: Range<usize>,
}

#[derive(Clone, Debug)]
pub(crate) enum Element {
    Instr(Instr),
    Branch(Vec<Vec<Element>>, Range<usize>),
}

struct P<'a> {
    s: &'a [u8],
    src: &'a str,
    pos: usize,
}

/// Parse all rungs in `text` (normally one, terminated by `;`).
pub(crate) fn parse(text: &str) -> Result<Vec<Vec<Element>>, ParseErr> {
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
            return Err(ParseErr {
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

    fn seq(&mut self, depth: usize) -> Result<Vec<Element>, ParseErr> {
        let mut out = Vec::new();
        loop {
            self.ws();
            if self.pos >= self.s.len() {
                if depth > 0 {
                    return Err(ParseErr {
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
                                return Err(ParseErr {
                                    message: "expected `,` or `]` in branch".into(),
                                    span: self.pos..self.pos + 1,
                                });
                            }
                        }
                    }
                    out.push(Element::Branch(legs, start..self.pos));
                }
                c if c.is_ascii_alphabetic() || c == b'_' => {
                    out.push(Element::Instr(self.instr()?))
                }
                _ => {
                    return Err(ParseErr {
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

    fn instr(&mut self) -> Result<Instr, ParseErr> {
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
            return Err(ParseErr {
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
                return Err(ParseErr {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branches_and_operands() {
        let t = "XIC(a)XIO(b)[OTE(c),OTL(d[1,2]) ,]OTU(e); ";
        let r = parse(t).unwrap_or_else(|e| panic!("{}", e.message));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].len(), 4);
        let Element::Branch(legs, _) = &r[0][2] else {
            panic!()
        };
        assert_eq!(legs.len(), 3);
        assert!(legs[2].is_empty());
        let Element::Instr(otl) = &legs[1][0] else {
            panic!()
        };
        assert_eq!(&t[otl.operands[0].clone()], "d[1,2]");
        let r = parse("NOP();").unwrap_or_else(|e| panic!("{}", e.message));
        let Element::Instr(nop) = &r[0][0] else {
            panic!()
        };
        assert!(nop.operands.is_empty());
        let r = parse("TON(T1,?,?)CPT(x, (a+b)*2);").unwrap_or_else(|e| panic!("{}", e.message));
        let Element::Instr(ton) = &r[0][0] else {
            panic!()
        };
        assert_eq!(ton.operands.len(), 3);
    }
}
