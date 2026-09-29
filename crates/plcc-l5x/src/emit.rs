// SPDX-License-Identifier: MPL-2.0

//! The generated Structured Text and its map back onto the L5X file.
//!
//! Lowering writes ST text. Every piece of text is recorded with the L5X span
//! it stands for: text copied from the file (an ST routine line, an operand)
//! maps byte for byte; synthesized text maps onto the element it was made for
//! (the instruction, the rung, the tag). After the ordinary plcc-st parser has
//! read the text, every span of the AST is rewritten through this map, so
//! parser, type-checker and codegen diagnostics point into the `.L5X` file.

use plcc_st::Span;
use std::ops::Range;

#[derive(Clone, Debug)]
struct Segment {
    gen_r: Range<usize>,
    src: Range<usize>,
    /// Text copied from the file: offsets map linearly.
    verbatim: bool,
}

#[derive(Default)]
pub(crate) struct Out {
    pub text: String,
    segs: Vec<Segment>,
    /// Where synthesized text maps to (innermost element being lowered).
    ctx: Vec<Span>,
}

impl Out {
    pub fn new() -> Self {
        Out::default()
    }

    pub fn push_ctx(&mut self, s: Span) {
        self.ctx.push(s);
    }

    pub fn pop_ctx(&mut self) {
        self.ctx.pop();
    }

    fn ctx_span(&self) -> Span {
        self.ctx.last().copied().unwrap_or(Span::new(0, 0))
    }

    fn record(&mut self, len: usize, src: Span, verbatim: bool) {
        let start = self.text.len() - len;
        if len == 0 {
            return;
        }
        if let Some(last) = self.segs.last_mut()
            && !verbatim
            && !last.verbatim
            && last.gen_r.end == start
            && last.src == (src.start..src.end)
        {
            last.gen_r.end = start + len;
            return;
        }
        self.segs.push(Segment {
            gen_r: start..start + len,
            src: src.start..src.end,
            verbatim,
        });
    }

    /// Synthesized text, mapped to the current context element.
    pub fn s(&mut self, t: &str) -> &mut Self {
        self.text.push_str(t);
        let c = self.ctx_span();
        self.record(t.len(), c, false);
        self
    }

    /// Synthesized text standing for `src`.
    pub fn m(&mut self, t: &str, src: Span) -> &mut Self {
        self.text.push_str(t);
        self.record(t.len(), src, false);
        self
    }

    /// Text copied unchanged from the file at `src_start`.
    pub fn v(&mut self, t: &str, src_start: usize) -> &mut Self {
        self.text.push_str(t);
        self.record(t.len(), Span::new(src_start, src_start + t.len()), true);
        self
    }

    /// Append another buffer (its segments shift along).
    pub fn append(&mut self, other: Out) {
        let base = self.text.len();
        self.text.push_str(&other.text);
        for mut s in other.segs {
            s.gen_r = s.gen_r.start + base..s.gen_r.end + base;
            self.segs.push(s);
        }
    }

    pub fn map(self) -> SpanMap {
        SpanMap { segs: self.segs }
    }
}

/// Every span in `value` set to the start of the file (for generated code
/// that stands for nothing in it).
pub(crate) fn zero_spans<T: serde::Serialize + serde::de::DeserializeOwned>(value: T) -> T {
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
            serde_json::Value::Array(items) => items.iter_mut().for_each(walk),
            _ => {}
        }
    }
    let Ok(mut json) = serde_json::to_value(&value) else {
        return value;
    };
    walk(&mut json);
    serde_json::from_value(json).unwrap_or(value)
}

pub(crate) struct SpanMap {
    segs: Vec<Segment>,
}

impl SpanMap {
    fn find(&self, p: usize) -> Option<&Segment> {
        let i = self.segs.partition_point(|s| s.gen_r.end <= p);
        self.segs.get(i).filter(|s| s.gen_r.start <= p)
    }

    fn start(&self, p: usize) -> usize {
        match self.find(p) {
            Some(s) if s.verbatim => s.src.start + (p - s.gen_r.start),
            Some(s) => s.src.start,
            None => self.find(p.saturating_sub(1)).map_or(0, |s| s.src.end),
        }
    }

    fn end(&self, p: usize) -> usize {
        if p == 0 {
            return self.start(0);
        }
        match self.find(p - 1) {
            Some(s) if s.verbatim => s.src.start + (p - s.gen_r.start),
            Some(s) => s.src.end,
            None => self.start(p),
        }
    }

    pub fn span(&self, s: Span) -> Span {
        let start = self.start(s.start);
        let end = if s.end > s.start {
            self.end(s.end)
        } else {
            start
        };
        if end < start {
            Span::new(start, start)
        } else {
            Span::new(start, end)
        }
    }

    /// Rewrite every `Span` inside `value` (any serializable AST node): the
    /// serde representation of a span is a uniform `{start, end}` object.
    pub fn remap<T: serde::Serialize + serde::de::DeserializeOwned>(&self, value: T) -> T {
        let Ok(mut json) = serde_json::to_value(&value) else {
            return value;
        };
        self.walk(&mut json);
        serde_json::from_value(json).unwrap_or(value)
    }

    fn walk(&self, v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(map) => {
                if map.len() == 2
                    && let (Some(s), Some(e)) = (
                        map.get("start").and_then(|x| x.as_u64()),
                        map.get("end").and_then(|x| x.as_u64()),
                    )
                {
                    let sp = self.span(Span::new(s as usize, e as usize));
                    map.insert("start".into(), sp.start.into());
                    map.insert("end".into(), sp.end.into());
                    return;
                }
                for (_, child) in map.iter_mut() {
                    self.walk(child);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    self.walk(item);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_and_synthesized_text_map_back() {
        let mut o = Out::new();
        o.push_ctx(Span::new(100, 120));
        o.s("x := ");
        o.v("Motor", 50);
        o.s(";");
        let m = o.map();
        assert_eq!(m.span(Span::new(5, 10)), Span::new(50, 55));
        assert_eq!(m.span(Span::new(6, 8)), Span::new(51, 53));
        assert_eq!(m.span(Span::new(0, 4)), Span::new(100, 120));
        assert_eq!(m.span(Span::new(0, 11)), Span::new(100, 120));
    }
}
