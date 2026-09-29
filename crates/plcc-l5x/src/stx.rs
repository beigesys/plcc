// SPDX-License-Identifier: MPL-2.0

//! Structured Text routines.
//!
//! Logix ST is close to IEC ST (same statements, IEC operator precedence —
//! 1756-PM007 "Determine the order of execution"), so a routine's text is
//! handed to the plcc-st parser after a token-level pass that
//!
//! * resolves every tag path through the Logix scopes (aliases, module tags
//!   `Local:1:I.Data`, program tags, keyword-named tags) exactly like ladder
//!   operands, so the text is copied byte for byte wherever nothing changes;
//! * turns the ST forms of instructions into plcc calls: `JSR(Routine, n,
//!   in...)` → a call of the routine's method (inputs copied to its SBR
//!   tags), `RET()` / `TND()` → `RETURN`, `SBR(...)` → nothing, `EVENT(task)`,
//!   `SIZE(array, dim, dest)`, Add-On Instruction calls `Aoi(tag, args...)` →
//!   an FB call with the required parameters;
//! * maps the non-retentive assignment `[:=]` to `:=`.
//!
//! After parsing, [`crate::stsem`] adds the Logix numeric semantics that IEC
//! does not have (integer `/` and MOD by zero, REAL → integer rounding).

use crate::emit::Out;
use crate::error::L5xError;
use crate::model::{RoutineDef, Usage};
use crate::names::ident;
use crate::operand::{self, LKind};
use crate::rll::{RoutineOut, Shared, Subs, cast};
use crate::scope::Ctx;
use crate::types::Ty;
use crate::xml::Text;
use std::collections::HashMap;
use std::ops::Range;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum K {
    Ws,
    Comment,
    /// `#region` / `#endregion` (editor outlining, 1756-PM007 "About
    /// outlining in Structured Text routines"): no meaning, dropped.
    Directive,
    Ident,
    Num,
    Str,
    Punct,
}

#[derive(Clone, Debug)]
struct Tok {
    k: K,
    r: Range<usize>,
}

/// Reserved words of Logix ST (left exactly as written).
const WORDS: &[&str] = &[
    "IF",
    "THEN",
    "ELSIF",
    "ELSE",
    "END_IF",
    "CASE",
    "OF",
    "END_CASE",
    "FOR",
    "TO",
    "BY",
    "DO",
    "END_FOR",
    "WHILE",
    "END_WHILE",
    "REPEAT",
    "UNTIL",
    "END_REPEAT",
    "EXIT",
    "RETURN",
    "AND",
    "OR",
    "XOR",
    "NOT",
    "MOD",
    "TRUE",
    "FALSE",
];

fn lex(s: &str) -> Result<Vec<Tok>, (String, Range<usize>)> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let start = i;
        let c = b[i];
        let k = if c.is_ascii_whitespace() {
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            K::Ws
        } else if c == b'#'
            && (s[i + 1..].to_ascii_lowercase().starts_with("region")
                || s[i + 1..].to_ascii_lowercase().starts_with("endregion"))
        {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            K::Directive
        } else if s[i..].starts_with("//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            K::Comment
        } else if s[i..].starts_with("(*") || s[i..].starts_with("/*") {
            let close = if b[i] == b'(' { "*)" } else { "*/" };
            match s[i + 2..].find(close) {
                Some(p) => i = i + 2 + p + 2,
                None => return Err(("unterminated comment".into(), start..b.len())),
            }
            K::Comment
        } else if c == b'\'' || c == b'"' {
            i += 1;
            while i < b.len() && b[i] != c {
                if b[i] == b'$' {
                    i += 1;
                }
                i += 1;
            }
            if i >= b.len() {
                return Err(("unterminated string".into(), start..b.len()));
            }
            i += 1;
            K::Str
        } else if c.is_ascii_alphabetic() || c == b'_' {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            K::Ident
        } else if c.is_ascii_digit() {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'#') {
                i += 1;
            }
            // Fraction (not a `..` range) and exponent.
            if i + 1 < b.len() && b[i] == b'.' && b[i + 1].is_ascii_digit() {
                i += 1;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
                    i += 1;
                }
            }
            if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                let mut j = i + 1;
                if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
                    j += 1;
                }
                if j < b.len() && b[j].is_ascii_digit() {
                    while j < b.len() && b[j].is_ascii_digit() {
                        j += 1;
                    }
                    i = j;
                }
            }
            K::Num
        } else {
            let two = ["[:=]", ":=", "<=", ">=", "<>", "**", ".."];
            let mut n = 1;
            for t in two {
                if s[i..].starts_with(t) {
                    n = t.len();
                    break;
                }
            }
            i += n.max(s[i..].chars().next().map_or(1, char::len_utf8));
            K::Punct
        };
        out.push(Tok { k, r: start..i });
    }
    Ok(out)
}

struct W<'a, 'x> {
    sh: &'a Shared<'a>,
    ctx: &'a Ctx<'x>,
    text: &'a Text,
    toks: Vec<Tok>,
    routines: &'a HashMap<String, String>,
    subs: &'a Subs,
    method: String,
    in_aoi: bool,
    errors: Vec<L5xError>,
    temps: std::collections::BTreeSet<String>,
    /// Prescan statements (non-retentive assignments).
    prescan: Vec<String>,
    /// The tag path emitted last (the target of a following `[:=]`).
    last_path: Option<(String, Ty)>,
}

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
    let text = Text::join(&r.lines);
    let method = routines
        .get(&r.name.text.to_ascii_lowercase())
        .cloned()
        .unwrap_or_default();
    let mut out = RoutineOut {
        temps: Default::default(),
        body: Out::new(),
        prescan: Out::new(),
    };
    let toks = match lex(&text.text) {
        Ok(t) => t,
        Err((m, sp)) => return (out, vec![L5xError::new(m, text.span(sp))]),
    };
    let mut w = W {
        sh,
        ctx,
        text: &text,
        toks,
        routines,
        subs,
        method,
        in_aoi,
        errors: Vec::new(),
        temps: Default::default(),
        prescan: Vec::new(),
        last_path: None,
    };
    let n = w.toks.len();
    out.body.push_ctx(r.span);
    let body = w.translate(0..n);
    out.temps = std::mem::take(&mut w.temps);
    out.prescan.push_ctx(r.span);
    for line in std::mem::take(&mut w.prescan) {
        out.prescan.s(&line).s("\n");
    }
    out.prescan.pop_ctx();
    out.body.append(body);
    out.body.s("\n");
    out.body.pop_ctx();
    (out, w.errors)
}

/// SBR parameters of ST routines (`SBR(a, b);` as their first statement).
pub(crate) fn sbr_params(ctx: &Ctx, r: &RoutineDef, map: &mut HashMap<String, Vec<(String, Ty)>>) {
    let text = Text::join(&r.lines);
    let Ok(toks) = lex(&text.text) else { return };
    let sig: Vec<&Tok> = toks
        .iter()
        .filter(|t| !matches!(t.k, K::Ws | K::Comment | K::Directive))
        .collect();
    let Some(first) = sig.first() else { return };
    if !text.text[first.r.clone()].eq_ignore_ascii_case("SBR") {
        return;
    }
    let Some(open) = sig.get(1).filter(|t| &text.text[t.r.clone()] == "(") else {
        return;
    };
    let Some(close) = text.text[open.r.end..].find(')') else {
        return;
    };
    let inner = open.r.end..open.r.end + close;
    let mut params = Vec::new();
    let mut start = inner.start;
    for (i, c) in text.text[inner.clone()].char_indices() {
        if c == ',' {
            params.push(start..inner.start + i);
            start = inner.start + i + 1;
        }
    }
    params.push(start..inner.end);
    let mut out = Vec::new();
    for p in params {
        let Ok(e) = operand::parse_expr(&text.text, p) else {
            return;
        };
        let Ok(d) = ctx.dest(&e, &text) else { return };
        out.push(d);
    }
    map.insert(r.name.text.to_ascii_lowercase(), out);
}

/// Types of the values the first `RET(v1, ...)` of an ST routine returns.
pub(crate) fn ret_types(ctx: &Ctx, r: &RoutineDef) -> Option<Vec<Ty>> {
    use crate::scope::Dom;
    use crate::types::Elem;
    let text = Text::join(&r.lines);
    let toks = lex(&text.text).ok()?;
    for (i, t) in toks.iter().enumerate() {
        if t.k != K::Ident || !text.text[t.r.clone()].eq_ignore_ascii_case("RET") {
            continue;
        }
        let open = toks[i + 1..]
            .iter()
            .find(|x| !matches!(x.k, K::Ws | K::Comment | K::Directive))?;
        if &text.text[open.r.clone()] != "(" {
            continue;
        }
        let close = text.text[open.r.end..].find(')')? + open.r.end;
        let inner = open.r.end..close;
        if text.text[inner.clone()].trim().is_empty() {
            continue;
        }
        let mut tys = Vec::new();
        let mut start = inner.start;
        let mut parts = Vec::new();
        for (j, c) in text.text[inner.clone()].char_indices() {
            if c == ',' {
                parts.push(start..inner.start + j);
                start = inner.start + j + 1;
            }
        }
        parts.push(start..inner.end);
        for p in parts {
            let e = operand::parse_expr(&text.text, p).ok()?;
            let v = ctx.value(&e, &text).ok()?;
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
    None
}

impl W<'_, '_> {
    fn s(&self, r: &Range<usize>) -> &str {
        &self.text.text[r.clone()]
    }

    fn copy(&self, out: &mut Out, r: Range<usize>) {
        let t = &self.text.text[r.clone()];
        if self.text.is_verbatim(r.clone()) {
            out.v(t, self.text.raw(r.start));
        } else {
            out.m(t, self.text.span(r));
        }
    }

    fn sig_next(&self, i: usize, end: usize) -> Option<usize> {
        (i..end).find(|&j| !matches!(self.toks[j].k, K::Ws | K::Comment | K::Directive))
    }

    fn sig_prev(&self, i: usize, start: usize) -> Option<usize> {
        (start..i)
            .rev()
            .find(|&j| !matches!(self.toks[j].k, K::Ws | K::Comment | K::Directive))
    }

    fn is_p(&self, i: usize, p: &str) -> bool {
        self.toks
            .get(i)
            .is_some_and(|t| t.k == K::Punct && self.s(&t.r) == p)
    }

    /// Index of the `)` matching the `(` at `open`.
    fn close_paren(&self, open: usize, end: usize) -> Option<usize> {
        let mut depth = 0i32;
        for j in open..end {
            if self.toks[j].k != K::Punct {
                continue;
            }
            match self.s(&self.toks[j].r) {
                "(" | "[" => depth += 1,
                ")" | "]" => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(j);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Top-level comma-separated argument token ranges inside `(open, close)`.
    fn args(&self, open: usize, close: usize) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        let mut depth = 0i32;
        let mut start = open + 1;
        for j in open + 1..close {
            if self.toks[j].k != K::Punct {
                continue;
            }
            match self.s(&self.toks[j].r) {
                "(" | "[" => depth += 1,
                ")" | "]" => depth -= 1,
                "," if depth == 0 => {
                    out.push(start..j);
                    start = j + 1;
                }
                _ => {}
            }
        }
        if (start..close).any(|j| !matches!(self.toks[j].k, K::Ws | K::Comment | K::Directive))
            || !out.is_empty()
        {
            out.push(start..close);
        }
        out
    }

    /// Text range covered by tokens `r` (trimmed of whitespace tokens).
    fn text_range(&self, r: Range<usize>) -> Option<Range<usize>> {
        let a = self.sig_next(r.start, r.end)?;
        let b = self.sig_prev(r.end, r.start)?;
        Some(self.toks[a].r.start..self.toks[b].r.end)
    }

    /// Argument `k` of a call as a Logix value (operand syntax).
    fn arg_value(&mut self, args: &[Range<usize>], k: usize) -> Option<crate::scope::Val> {
        let r = args.get(k).and_then(|r| self.text_range(r.clone()))?;
        match operand::parse_expr(&self.text.text, r)
            .map_err(|e| L5xError::new(e.message, self.text.span(e.span)))
            .and_then(|e| self.ctx.value(&e, self.text))
        {
            Ok(v) => Some(v),
            Err(e) => {
                self.errors.push(e);
                None
            }
        }
    }

    fn err(&mut self, m: impl Into<String>, r: Range<usize>) {
        self.errors.push(L5xError::new(m, self.text.span(r)));
    }

    /// Extent of a tag path starting at identifier token `i`: `a:b.c[..].5`.
    fn path_end(&self, i: usize, end: usize) -> usize {
        let mut j = i + 1;
        // `:seg` right after (no spaces): module tags, `S:FS`, `Program:X`.
        while j + 1 < end && self.is_p(j, ":") && matches!(self.toks[j + 1].k, K::Ident | K::Num) {
            j += 2;
        }
        loop {
            let Some(n) = self.sig_next(j, end) else {
                break;
            };
            if self.is_p(n, ".") {
                let Some(m) = self.sig_next(n + 1, end) else {
                    break;
                };
                if matches!(self.toks[m].k, K::Ident | K::Num) {
                    j = m + 1;
                    continue;
                }
                if self.is_p(m, "[")
                    && let Some(c) = self.close_paren(m, end)
                {
                    j = c + 1;
                    continue;
                }
                break;
            } else if self.is_p(n, "[") {
                match self.close_paren(n, end) {
                    Some(c) => j = c + 1,
                    None => break,
                }
            } else {
                break;
            }
        }
        j
    }

    fn translate(&mut self, range: Range<usize>) -> Out {
        let mut out = Out::new();
        let mut i = range.start;
        while i < range.end {
            let t = self.toks[i].clone();
            match t.k {
                K::Directive => {
                    out.m(" ", self.text.span(t.r.clone()));
                    i += 1;
                }
                K::Ws | K::Comment | K::Num | K::Str => {
                    self.copy(&mut out, t.r.clone());
                    i += 1;
                }
                K::Punct => {
                    if self.s(&t.r) == "[:=]" {
                        // Non-retentive assignment: "the tag ... is reset to
                        // zero each time the controller enters the Run mode"
                        // (1756-RM003 "Specify a non-retentive assignment").
                        if let Some((st, ty)) = self.last_path.take() {
                            let zero = match ty.elem() {
                                Some(crate::types::Elem::Bool) => Some("FALSE"),
                                Some(e) if e.is_real() => Some("0.0"),
                                Some(e) if e.is_int() => Some("0"),
                                _ => None,
                            };
                            if let Some(z) = zero {
                                self.prescan.push(format!("{st} := {z};"));
                            }
                        }
                        out.m(":=", self.text.span(t.r.clone()));
                    } else {
                        self.copy(&mut out, t.r.clone());
                    }
                    i += 1;
                }
                K::Ident => {
                    i = self.ident(i, range.end, &mut out);
                }
            }
        }
        out
    }

    fn ident(&mut self, i: usize, end: usize, out: &mut Out) -> usize {
        let t = self.toks[i].clone();
        let name = self.s(&t.r).to_string();
        let up = name.to_ascii_uppercase();
        let after_dot = self.sig_prev(i, 0).is_some_and(|p| self.is_p(p, "."));
        if after_dot {
            out.m(&ident(&name), self.text.span(t.r.clone()));
            return i + 1;
        }
        if WORDS.contains(&up.as_str()) {
            self.copy(out, t.r.clone());
            return i + 1;
        }
        if let Some(open) = self.sig_next(i + 1, end)
            && self.is_p(open, "(")
            && let Some(close) = self.close_paren(open, end)
            && let Some(next) = self.call(&up, i, open, close, end, out)
        {
            return next;
        }
        // A tag path.
        let pend = self.path_end(i, end);
        let r = t.r.start..self.toks[pend - 1].r.end;
        if let Ok(e) = operand::parse_expr(&self.text.text, r.clone())
            && let LKind::Path(p) = &e.kind
        {
            match self.ctx.path(p, self.text) {
                Ok((st, ty)) => {
                    self.last_path = Some((st.clone(), ty));
                    if st == self.s(&r) && self.text.is_verbatim(r.clone()) {
                        self.copy(out, r);
                    } else {
                        out.m(&st, self.text.span(r));
                    }
                    return pend;
                }
                Err(e) if !e.message.starts_with("unknown tag") => {
                    self.errors.push(e);
                    out.m(&ident(&name), self.text.span(t.r.clone()));
                    return i + 1;
                }
                Err(_) => {}
            }
        }
        // Not a tag: a function name, an enumerated value... left to plcc.
        let st = ident(&name);
        if st == name {
            self.copy(out, t.r.clone());
        } else {
            out.m(&st, self.text.span(t.r.clone()));
        }
        i + 1
    }

    /// Instruction-style calls that need rewriting. Returns the token index to
    /// continue at, or None to treat the call as an ordinary function call.
    fn call(
        &mut self,
        up: &str,
        i: usize,
        open: usize,
        close: usize,
        end: usize,
        out: &mut Out,
    ) -> Option<usize> {
        let whole = self.toks[i].r.start..self.toks[close].r.end;
        let span = self.text.span(whole.clone());
        // Consume the statement's `;` when the rewrite ends with its own.
        let after_semi = match self.sig_next(close + 1, end) {
            Some(s) if self.is_p(s, ";") => s + 1,
            _ => close + 1,
        };
        let args = self.args(open, close);
        let arg_text = |w: &Self, k: usize| -> Option<String> {
            args.get(k)
                .and_then(|r| w.text_range(r.clone()))
                .map(|r| w.s(&r).trim().to_string())
        };
        match up {
            "JSR" => {
                let Some(name) = arg_text(self, 0) else {
                    self.err("JSR needs a routine name", whole);
                    return Some(after_semi);
                };
                if self.in_aoi {
                    self.err("JSR inside an Add-On Instruction", whole);
                    return Some(after_semi);
                }
                let Some(m) = self.routines.get(&name.to_ascii_lowercase()).cloned() else {
                    self.err(format!("unknown routine `{name}`"), whole);
                    return Some(after_semi);
                };
                let n: usize = arg_text(self, 1).and_then(|x| x.parse().ok()).unwrap_or(0);
                let extra = args.len().saturating_sub(2);
                let mut stmts = String::new();
                let targets = self
                    .subs
                    .sbr
                    .get(&name.to_ascii_lowercase())
                    .cloned()
                    .unwrap_or_default();
                if targets.len() < n.min(extra) {
                    self.err(
                        format!("JSR passes {n} input(s) but `{name}` has no SBR taking them"),
                        whole,
                    );
                    return Some(after_semi);
                }
                for (k, (target, tty)) in targets.iter().enumerate().take(n.min(extra)) {
                    let Some(v) = self.arg_value(&args, 2 + k) else {
                        continue;
                    };
                    let value = match tty.elem() {
                        Some(e) => cast(&v, e),
                        None => v.st,
                    };
                    stmts.push_str(&format!("{target} := {value}; "));
                }
                stmts.push_str(&format!("{m}();"));
                let rtypes = self
                    .subs
                    .ret
                    .get(&m.to_ascii_lowercase())
                    .cloned()
                    .unwrap_or_default();
                for k in 0..extra.saturating_sub(n) {
                    let Some(r) = args.get(2 + n + k).and_then(|r| self.text_range(r.clone()))
                    else {
                        continue;
                    };
                    let Some(rt) = rtypes.get(k) else {
                        self.err(
                            format!("`{name}` returns fewer values than this JSR expects"),
                            r,
                        );
                        continue;
                    };
                    let dest = operand::parse_expr(&self.text.text, r.clone())
                        .map_err(|e| L5xError::new(e.message, self.text.span(e.span)))
                        .and_then(|e| self.ctx.dest(&e, self.text));
                    let (d, dty) = match dest {
                        Ok(x) => x,
                        Err(e) => {
                            self.errors.push(e);
                            continue;
                        }
                    };
                    let rv = Subs::ret_var(&m, k);
                    match (dty.elem(), crate::scope::dom_of(rt)) {
                        (Some(de), Some(rd)) => {
                            let v = crate::scope::Val {
                                st: rv,
                                dom: rd,
                                ty: rt.clone(),
                            };
                            stmts.push_str(&format!(" {d} := {};", cast(&v, de)));
                        }
                        _ => stmts.push_str(&format!(" {d} := {rv};")),
                    }
                }
                out.m(&stmts, span);
                Some(after_semi)
            }
            "RET" | "TND" => {
                let mut stmts = String::new();
                if up == "RET" {
                    let types = self
                        .subs
                        .ret
                        .get(&self.method.to_ascii_lowercase())
                        .cloned()
                        .unwrap_or_default();
                    for k in 0..args.len() {
                        let Some(v) = self.arg_value(&args, k) else {
                            continue;
                        };
                        let value = match types.get(k).and_then(|t| t.elem()) {
                            Some(e) => cast(&v, e),
                            None => v.st,
                        };
                        stmts.push_str(&format!("{} := {value}; ", Subs::ret_var(&self.method, k)));
                    }
                }
                stmts.push_str("RETURN;");
                out.m(&stmts, span);
                Some(after_semi)
            }
            "SBR" => Some(after_semi),
            "EVENT" => {
                let name = arg_text(self, 0).unwrap_or_default();
                out.m(&format!("lx__event_{} := TRUE;", ident(&name)), span);
                Some(after_semi)
            }
            "DEG" | "RAD" => {
                out.m(
                    &format!("lx__{}", up.to_ascii_lowercase()),
                    self.text.span(self.toks[i].r.clone()),
                );
                Some(i + 1)
            }
            "SIZE" => {
                // SIZE(array, dimension, dest): a constant here, since plcc
                // arrays have fixed bounds.
                let res = (|| -> Result<String, L5xError> {
                    let arr = args
                        .first()
                        .and_then(|r| self.text_range(r.clone()))
                        .ok_or_else(|| L5xError::new("SIZE needs an array", span))?;
                    let e = operand::parse_expr(&self.text.text, arr.clone())
                        .map_err(|e| L5xError::new(e.message, self.text.span(e.span)))?;
                    let LKind::Path(p) = &e.kind else {
                        return Err(L5xError::new(
                            "SIZE needs an array tag",
                            self.text.span(arr),
                        ));
                    };
                    let dims = self.ctx.size_dims(p, self.text)?;
                    let d: usize = arg_text(self, 1).and_then(|x| x.parse().ok()).unwrap_or(0);
                    let n = dims
                        .get(d)
                        .copied()
                        .ok_or_else(|| L5xError::new("SIZE: no such dimension", span))?;
                    let dr = args
                        .get(2)
                        .and_then(|r| self.text_range(r.clone()))
                        .ok_or_else(|| L5xError::new("SIZE needs a destination", span))?;
                    let de = operand::parse_expr(&self.text.text, dr)
                        .map_err(|e| L5xError::new(e.message, self.text.span(e.span)))?;
                    let (dst, _) = self.ctx.dest(&de, self.text)?;
                    Ok(format!("{dst} := {n};"))
                })();
                match res {
                    Ok(s) => {
                        out.m(&s, span);
                    }
                    Err(e) => self.errors.push(e),
                };
                Some(after_semi)
            }
            "COP" | "CPS" | "FLL" => {
                let res = (|| -> Result<String, L5xError> {
                    let op = |k: usize| -> Result<crate::operand::LExpr, L5xError> {
                        let r = args
                            .get(k)
                            .and_then(|r| self.text_range(r.clone()))
                            .ok_or_else(|| L5xError::new(format!("{up} needs 3 operands"), span))?;
                        operand::parse_expr(&self.text.text, r)
                            .map_err(|e| L5xError::new(e.message, self.text.span(e.span)))
                    };
                    let (a, b, l) = (op(0)?, op(1)?, op(2)?);
                    let len = self.ctx.value(&l, self.text)?;
                    if up == "FLL" {
                        crate::rll::fll_code(self.ctx, &a, &b, &len, self.text, span)
                    } else {
                        crate::rll::cop_code(self.ctx, &a, &b, &len, self.text, span)
                    }
                })();
                match res {
                    Ok(s) => {
                        self.temps.insert("lx__i : LINT".into());
                        self.temps.insert("lx__n : LINT".into());
                        out.m(&s, span);
                    }
                    Err(e) => self.errors.push(e),
                }
                Some(after_semi)
            }
            "CONCAT" | "MID" | "DELETE" | "INSERT" | "FIND" | "UPPER" | "LOWER" | "DTOS"
            | "STOD" => {
                let mut ops = Vec::new();
                for r in &args {
                    let Some(r) = self.text_range(r.clone()) else {
                        continue;
                    };
                    match operand::parse_expr(&self.text.text, r) {
                        Ok(e) => ops.push(e),
                        Err(e) => {
                            self.errors
                                .push(L5xError::new(e.message, self.text.span(e.span)));
                            return Some(after_semi);
                        }
                    }
                }
                match crate::rll::string_code(self.ctx, self.sh.strings, up, &ops, self.text, span)
                {
                    Ok(s) => {
                        out.m(&s, span);
                    }
                    Err(e) => self.errors.push(e),
                }
                Some(after_semi)
            }
            "GSV" | "SSV" => {
                let what: Vec<String> = (0..3).filter_map(|k| arg_text(self, k)).collect();
                self.errors.push(L5xError::warning(
                    format!(
                        "{up} {}: plcc has no controller object model; {}",
                        what.join("."),
                        if up == "GSV" {
                            "the destination keeps its value"
                        } else {
                            "nothing is set"
                        }
                    ),
                    span,
                ));
                Some(after_semi)
            }
            "MSG" => {
                self.errors.push(L5xError::warning(
                    "MSG: plcc has no CIP messaging; the message never starts (EN, DN and ER stay FALSE)",
                    span,
                ));
                Some(after_semi)
            }
            "BTDT" | "MVMT" | "PID" => {
                self.err(
                    format!("{up} in a Structured Text routine is not supported yet"),
                    whole,
                );
                Some(after_semi)
            }
            _ => {
                let sig = self.sh.aois.get(&up.to_ascii_lowercase())?.clone();
                self.aoi_call(&sig, &args, whole, out);
                Some(after_semi)
            }
        }
    }

    fn aoi_call(
        &mut self,
        sig: &crate::lower::AoiSig,
        args: &[Range<usize>],
        whole: Range<usize>,
        out: &mut Out,
    ) {
        let span = self.text.span(whole.clone());
        let req: Vec<_> = sig
            .params
            .iter()
            .filter(|p| {
                p.required
                    && !p.logix.eq_ignore_ascii_case("EnableIn")
                    && !p.logix.eq_ignore_ascii_case("EnableOut")
            })
            .cloned()
            .collect();
        if args.len() != req.len() + 1 {
            self.err(
                format!("expected {} operand(s) (the backing tag and the required parameters), found {}", req.len() + 1, args.len()),
                whole,
            );
            return;
        }
        let tag = self.translate(args[0].clone()).text.trim().to_string();
        let mut call_args = vec!["EnableIn := TRUE".to_string()];
        let mut outs = Vec::new();
        let mut pre = String::new();
        for (k, p) in req.iter().enumerate() {
            let a = self.translate(args[k + 1].clone()).text.trim().to_string();
            match p.usage {
                // An alias parameter is a member of the backing tag.
                Usage::Input if p.alias => pre.push_str(&format!("{tag}.{} := {a}; ", p.st)),
                Usage::Input | Usage::InOut => call_args.push(format!("{} := {a}", p.st)),
                _ => outs.push(format!("{a} := {tag}.{};", p.st)),
            }
        }
        let mut s = format!("{pre}{tag}({});", call_args.join(", "));
        for o in outs {
            s.push(' ');
            s.push_str(&o);
        }
        out.m(&s, span);
    }
}
