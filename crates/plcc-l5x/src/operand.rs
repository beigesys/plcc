// SPDX-License-Identifier: MPL-2.0

//! Ladder operands and the expressions of CPT, CMP and array subscripts.
//!
//! Syntax per 1756-RM003 "Compute/Math Instructions" (CPT "Formatting
//! expressions", operator order table) and "Index through arrays" / "Bit
//! Addressing": tag paths `a.b[i,j].c.5`, indirect bits `dint.[idx]`,
//! module tags `Local:1:I.Data`, status keywords `S:FS`, immediate values in
//! any radix (`16#ff`, `2#1010`, `8#17`), `1.$` for infinity.
//!
//! Spans are byte ranges of the text being parsed (the rung text or an ST
//! line); callers map them onto the L5X file.

use std::ops::Range;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Seg {
    Member(String, Range<usize>),
    Bit(u32, Range<usize>),
    /// `.[expr]`: bit number computed at run time.
    IndirectBit(Box<LExpr>),
    Index(Vec<LExpr>, Range<usize>),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TagPath {
    /// As written, including `:` segments (`Local:1:I`, `S:FS`).
    pub base: String,
    pub base_span: Range<usize>,
    pub segs: Vec<Seg>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BinOp {
    Pow,
    Mul,
    Div,
    Mod,
    Add,
    Sub,
    And,
    Xor,
    Or,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    LAnd,
    LXor,
    LOr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnOp {
    Neg,
    Not,
    LNot,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LKind {
    Int(i128),
    Real(f64),
    Str(String),
    Path(TagPath),
    Unary(UnOp, Box<LExpr>),
    Binary(BinOp, Box<LExpr>, Box<LExpr>),
    Call(String, Vec<LExpr>),
    /// `?`: an unset pseudo-operand.
    Unset,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LExpr {
    pub kind: LKind,
    pub span: Range<usize>,
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Int(i128),
    Real(f64),
    Str(String),
    Punct(&'static str),
}

pub(crate) struct ParseErr {
    pub message: String,
    pub span: Range<usize>,
}

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
    end: usize,
}

const PUNCT: &[&str] = &[
    "**", "<=", ">=", "<>", "&&", "||", "^^", ":=", "(", ")", "[", "]", ",", ".", ":", "+", "-",
    "*", "/", "=", "<", ">", "&", "|", "!", "?", "^", "#",
];

impl<'a> Lexer<'a> {
    fn skip_ws(&mut self) {
        let b = self.src.as_bytes();
        while self.pos < self.end && b[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn next(&mut self) -> Result<Option<(Tok, Range<usize>)>, ParseErr> {
        self.skip_ws();
        if self.pos >= self.end {
            return Ok(None);
        }
        let b = self.src.as_bytes();
        let start = self.pos;
        let c = b[start];
        if c.is_ascii_alphabetic() || c == b'_' {
            let mut p = start;
            while p < self.end && (b[p].is_ascii_alphanumeric() || b[p] == b'_') {
                p += 1;
            }
            self.pos = p;
            return Ok(Some((Tok::Ident(self.src[start..p].to_string()), start..p)));
        }
        if c.is_ascii_digit() {
            return self.number().map(Some);
        }
        if c == b'\'' || c == b'"' {
            let q = c;
            let mut p = start + 1;
            let mut s = String::new();
            while p < self.end && b[p] != q {
                if b[p] == b'$' && p + 1 < self.end {
                    let n = b[p + 1];
                    let (ch, len) = match n {
                        b'$' => ('$', 2),
                        b'\'' => ('\'', 2),
                        b'"' => ('"', 2),
                        b'L' | b'l' | b'N' | b'n' => ('\n', 2),
                        b'P' | b'p' => ('\x0c', 2),
                        b'R' | b'r' => ('\r', 2),
                        b'T' | b't' => ('\t', 2),
                        _ if p + 2 < self.end
                            && n.is_ascii_hexdigit()
                            && b[p + 2].is_ascii_hexdigit() =>
                        {
                            let v = u8::from_str_radix(&self.src[p + 1..p + 3], 16).unwrap_or(0);
                            (v as char, 3)
                        }
                        _ => ('$', 1),
                    };
                    s.push(ch);
                    p += len;
                } else {
                    let ch = self.src[p..].chars().next().unwrap_or(' ');
                    s.push(ch);
                    p += ch.len_utf8();
                }
            }
            if p >= self.end {
                return Err(ParseErr {
                    message: "unterminated string".into(),
                    span: start..self.end,
                });
            }
            self.pos = p + 1;
            return Ok(Some((Tok::Str(s), start..p + 1)));
        }
        for p in PUNCT {
            if self.src[start..self.end].starts_with(p) {
                self.pos = start + p.len();
                return Ok(Some((Tok::Punct(p), start..self.pos)));
            }
        }
        Err(ParseErr {
            message: format!("unexpected character `{}`", &self.src[start..start + 1]),
            span: start..start + 1,
        })
    }

    fn number(&mut self) -> Result<(Tok, Range<usize>), ParseErr> {
        let b = self.src.as_bytes();
        let start = self.pos;
        let mut p = start;
        while p < self.end && (b[p].is_ascii_digit() || b[p] == b'_') {
            p += 1;
        }
        // Radix: 16#ff, 2#1010, 8#17.
        if p < self.end && b[p] == b'#' {
            let radix: u32 = self.src[start..p].replace('_', "").parse().unwrap_or(10);
            let mut q = p + 1;
            while q < self.end && (b[q].is_ascii_alphanumeric() || b[q] == b'_') {
                q += 1;
            }
            let digits = self.src[p + 1..q].replace('_', "");
            let v = u128::from_str_radix(&digits, radix).map_err(|_| ParseErr {
                message: format!("`{}` is not a base-{radix} number", &self.src[start..q]),
                span: start..q,
            })?;
            self.pos = q;
            return Ok((Tok::Int(v as i128), start..q));
        }
        let mut real = false;
        if p + 1 < self.end && b[p] == b'.' && b[p + 1] == b'$' {
            // `1.$`: infinity (1756-RM003 DIV).
            self.pos = p + 2;
            return Ok((Tok::Real(f64::INFINITY), start..p + 2));
        }
        if p + 1 < self.end && b[p] == b'.' && b[p + 1].is_ascii_digit() {
            real = true;
            p += 1;
            while p < self.end && (b[p].is_ascii_digit() || b[p] == b'_') {
                p += 1;
            }
        }
        if p < self.end && (b[p] == b'e' || b[p] == b'E') {
            let mut q = p + 1;
            if q < self.end && (b[q] == b'+' || b[q] == b'-') {
                q += 1;
            }
            if q < self.end && b[q].is_ascii_digit() {
                while q < self.end && b[q].is_ascii_digit() {
                    q += 1;
                }
                real = true;
                p = q;
            }
        }
        let text = self.src[start..p].replace('_', "");
        self.pos = p;
        if real {
            let v: f64 = text.parse().map_err(|_| ParseErr {
                message: format!("`{text}` is not a number"),
                span: start..p,
            })?;
            Ok((Tok::Real(v), start..p))
        } else {
            let v: i128 = text.parse().map_err(|_| ParseErr {
                message: format!("`{text}` is not a number"),
                span: start..p,
            })?;
            Ok((Tok::Int(v), start..p))
        }
    }
}

pub(crate) struct Parser<'a> {
    toks: Vec<(Tok, Range<usize>)>,
    i: usize,
    end: usize,
    _src: &'a str,
}

/// Parse `src[range]` as one expression.
pub(crate) fn parse_expr(src: &str, range: Range<usize>) -> Result<LExpr, ParseErr> {
    let mut lx = Lexer {
        src,
        pos: range.start,
        end: range.end,
    };
    let mut toks = Vec::new();
    while let Some(t) = lx.next()? {
        toks.push(t);
    }
    let mut p = Parser {
        toks,
        i: 0,
        end: range.end,
        _src: src,
    };
    if p.toks.is_empty() {
        return Err(ParseErr {
            message: "empty operand".into(),
            span: range,
        });
    }
    let e = p.expr(0)?;
    if let Some((t, sp)) = p.toks.get(p.i) {
        return Err(ParseErr {
            message: format!("unexpected `{}` in operand", tok_text(t)),
            span: sp.clone(),
        });
    }
    Ok(e)
}

fn tok_text(t: &Tok) -> String {
    match t {
        Tok::Ident(s) => s.clone(),
        Tok::Int(v) => v.to_string(),
        Tok::Real(v) => v.to_string(),
        Tok::Str(s) => format!("'{s}'"),
        Tok::Punct(p) => p.to_string(),
    }
}

/// Binary operators by precedence level, lowest first (1756-RM003 CPT "Determine
/// the order of operation", read bottom-up).
fn binop(t: &Tok) -> Option<(u8, BinOp)> {
    Some(match t {
        Tok::Punct("||") => (1, BinOp::LOr),
        Tok::Punct("^^") => (2, BinOp::LXor),
        Tok::Punct("&&") => (3, BinOp::LAnd),
        Tok::Punct("=") => (4, BinOp::Eq),
        Tok::Punct("<>") => (4, BinOp::Ne),
        Tok::Punct("<") => (4, BinOp::Lt),
        Tok::Punct("<=") => (4, BinOp::Le),
        Tok::Punct(">") => (4, BinOp::Gt),
        Tok::Punct(">=") => (4, BinOp::Ge),
        Tok::Ident(s) if s.eq_ignore_ascii_case("OR") => (5, BinOp::Or),
        Tok::Punct("|") => (5, BinOp::Or),
        Tok::Ident(s) if s.eq_ignore_ascii_case("XOR") => (6, BinOp::Xor),
        Tok::Ident(s) if s.eq_ignore_ascii_case("AND") => (7, BinOp::And),
        Tok::Punct("&") => (7, BinOp::And),
        Tok::Punct("+") => (8, BinOp::Add),
        Tok::Punct("-") => (8, BinOp::Sub),
        Tok::Punct("*") => (9, BinOp::Mul),
        Tok::Punct("/") => (9, BinOp::Div),
        Tok::Ident(s) if s.eq_ignore_ascii_case("MOD") => (9, BinOp::Mod),
        Tok::Punct("**") => (11, BinOp::Pow),
        _ => return None,
    })
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.i).map(|t| &t.0)
    }

    fn span_here(&self) -> Range<usize> {
        self.toks
            .get(self.i)
            .map(|t| t.1.clone())
            .unwrap_or(self.end..self.end)
    }

    fn eat(&mut self, p: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Punct(q)) if *q == p) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, p: &str) -> Result<Range<usize>, ParseErr> {
        let sp = self.span_here();
        if self.eat(p) {
            Ok(sp)
        } else {
            Err(ParseErr {
                message: format!(
                    "expected `{p}`, found {}",
                    self.peek()
                        .map_or("end of operand".into(), |t| format!("`{}`", tok_text(t)))
                ),
                span: sp,
            })
        }
    }

    fn expr(&mut self, min: u8) -> Result<LExpr, ParseErr> {
        let mut lhs = self.unary()?;
        loop {
            let Some((prec, op)) = self.peek().and_then(binop) else {
                break;
            };
            if prec < min {
                break;
            }
            self.i += 1;
            // `**` is evaluated left to right too ("Operations of equal order are
            // performed from left to right").
            let rhs = self.expr(prec + 1)?;
            let span = lhs.span.start..rhs.span.end;
            lhs = LExpr {
                kind: LKind::Binary(op, Box::new(lhs), Box::new(rhs)),
                span,
            };
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<LExpr, ParseErr> {
        let sp = self.span_here();
        let op = match self.peek() {
            Some(Tok::Punct("-")) => Some(UnOp::Neg),
            Some(Tok::Punct("+")) => {
                self.i += 1;
                return self.unary();
            }
            Some(Tok::Punct("!")) => Some(UnOp::LNot),
            Some(Tok::Ident(s)) if s.eq_ignore_ascii_case("NOT") => Some(UnOp::Not),
            _ => None,
        };
        if let Some(op) = op {
            self.i += 1;
            // Unary binds tighter than * but looser than **.
            let inner = self.expr(10)?;
            let span = sp.start..inner.span.end;
            // Fold `-5` / `-1.5` into the literal.
            if op == UnOp::Neg {
                match inner.kind {
                    LKind::Int(v) => {
                        return Ok(LExpr {
                            kind: LKind::Int(-v),
                            span,
                        });
                    }
                    LKind::Real(v) => {
                        return Ok(LExpr {
                            kind: LKind::Real(-v),
                            span,
                        });
                    }
                    _ => {}
                }
            }
            return Ok(LExpr {
                kind: LKind::Unary(op, Box::new(inner)),
                span,
            });
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<LExpr, ParseErr> {
        let Some((tok, sp)) = self.toks.get(self.i).cloned() else {
            return Err(ParseErr {
                message: "expected an operand".into(),
                span: self.end..self.end,
            });
        };
        self.i += 1;
        match tok {
            Tok::Int(v) => Ok(LExpr {
                kind: LKind::Int(v),
                span: sp,
            }),
            Tok::Real(v) => Ok(LExpr {
                kind: LKind::Real(v),
                span: sp,
            }),
            Tok::Str(s) => Ok(LExpr {
                kind: LKind::Str(s),
                span: sp,
            }),
            Tok::Punct("?") => Ok(LExpr {
                kind: LKind::Unset,
                span: sp,
            }),
            Tok::Punct("(") => {
                let e = self.expr(0)?;
                let close = self.expect(")")?;
                Ok(LExpr {
                    kind: e.kind,
                    span: sp.start..close.end,
                })
            }
            Tok::Ident(name) => {
                if self.eat("(") {
                    let mut args = Vec::new();
                    if !self.eat(")") {
                        loop {
                            args.push(self.expr(0)?);
                            if self.eat(")") {
                                break;
                            }
                            self.expect(",")?;
                        }
                    }
                    let end = self.toks[self.i - 1].1.end;
                    return Ok(LExpr {
                        kind: LKind::Call(name.to_ascii_uppercase(), args),
                        span: sp.start..end,
                    });
                }
                self.path(name, sp)
            }
            other => Err(ParseErr {
                message: format!("unexpected `{}` in operand", tok_text(&other)),
                span: sp,
            }),
        }
    }

    fn path(&mut self, name: String, sp: Range<usize>) -> Result<LExpr, ParseErr> {
        let mut base = name;
        let mut base_span = sp;
        // Module and keyword prefixes: `Local:1:I`, `S:FS`, `Program:P`.
        while matches!(self.peek(), Some(Tok::Punct(":")))
            && matches!(
                self.toks.get(self.i + 1).map(|t| &t.0),
                Some(Tok::Ident(_)) | Some(Tok::Int(_))
            )
            && self.toks[self.i].1.start == base_span.end
        {
            self.i += 1;
            let (t, s) = self.toks[self.i].clone();
            self.i += 1;
            base.push(':');
            base.push_str(&tok_text(&t));
            base_span.end = s.end;
        }
        let mut segs = Vec::new();
        loop {
            if self.eat(".") {
                let Some((t, s)) = self.toks.get(self.i).cloned() else {
                    return Err(ParseErr {
                        message: "expected a member after `.`".into(),
                        span: self.end..self.end,
                    });
                };
                self.i += 1;
                match t {
                    Tok::Ident(m) => segs.push(Seg::Member(m, s)),
                    Tok::Int(b) => segs.push(Seg::Bit(b as u32, s)),
                    Tok::Real(_) => {
                        return Err(ParseErr {
                            message: "unexpected number after `.`".into(),
                            span: s,
                        });
                    }
                    Tok::Punct("[") => {
                        let e = self.expr(0)?;
                        self.expect("]")?;
                        segs.push(Seg::IndirectBit(Box::new(e)));
                    }
                    other => {
                        return Err(ParseErr {
                            message: format!("unexpected `{}` after `.`", tok_text(&other)),
                            span: s,
                        });
                    }
                }
            } else if matches!(self.peek(), Some(Tok::Punct("["))) {
                let open = self.span_here();
                self.i += 1;
                let mut idx = vec![self.expr(0)?];
                while self.eat(",") {
                    idx.push(self.expr(0)?);
                }
                let close = self.expect("]")?;
                segs.push(Seg::Index(idx, open.start..close.end));
            } else {
                break;
            }
        }
        let end = self.toks[self.i - 1].1.end;
        Ok(LExpr {
            span: base_span.start..end,
            kind: LKind::Path(TagPath {
                base,
                base_span,
                segs,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> LExpr {
        parse_expr(s, 0..s.len()).unwrap_or_else(|e| panic!("{s}: {}", e.message))
    }

    #[test]
    fn paths_and_expressions() {
        let e = p("Local:1:I.Data.3");
        let LKind::Path(tp) = e.kind else { panic!() };
        assert_eq!(tp.base, "Local:1:I");
        assert_eq!(tp.segs.len(), 2);
        assert!(matches!(tp.segs[1], Seg::Bit(3, _)));
        let e = p("a[i+1,2].b");
        assert!(matches!(e.kind, LKind::Path(_)));
        let e = p("(a + 2) * b ** 2 MOD 3");
        assert!(matches!(e.kind, LKind::Binary(BinOp::Mod, _, _)));
        assert_eq!(p("-5").kind, LKind::Int(-5));
        assert_eq!(p("16#FF").kind, LKind::Int(255));
        assert_eq!(p("3.4028235E+38").kind, LKind::Real(3.4028235E+38));
        assert!(matches!(
            p("ABS(x) > 2 AND y").kind,
            LKind::Binary(BinOp::Gt, _, _)
        ));
        assert!(matches!(p("S:FS").kind, LKind::Path(ref t) if t.base == "S:FS"));
        assert_eq!(p("?").kind, LKind::Unset);
    }
}
