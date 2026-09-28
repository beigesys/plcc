// SPDX-License-Identifier: MPL-2.0

//! Structured Text embedded in PLCopen XML: `<ST>` bodies, contact/coil
//! variables, `inVariable` expressions, initial values, task intervals.
//!
//! Each fragment is decoded out of the raw XML (CDATA, entities, XHTML wrapper
//! tags) *with a byte map back to the XML*, handed to the ordinary plcc-st
//! parser inside a small synthetic wrapper, and every span of the result is then
//! remapped onto the XML file. Diagnostics from the parser, the type checker and
//! codegen therefore point at the right place in the `.xml` the user opened.

use crate::error::PlcOpenError;
use plcc_st::ast::{Expression, Statement, StatementKind};
use plcc_st::{Declaration, Span};
use std::ops::Range;

/// Decoded text plus, for every decoded byte, the XML byte offset it came from.
pub(crate) struct Fragment {
    pub text: String,
    /// `map[i]` is the raw offset of decoded byte `i`; `map[text.len()]` is the
    /// end of the raw range.
    map: Vec<usize>,
    /// Raw range of the whole fragment (fallback for synthetic wrapper text).
    pub raw: Range<usize>,
}

impl Fragment {
    /// Decode `src[range]`: character data with entities, CDATA sections, and
    /// markup (XHTML `<p>`, `<br/>`, …) that is skipped (`<br>` becomes a newline).
    pub fn decode(src: &str, range: Range<usize>) -> Fragment {
        let bytes = src.as_bytes();
        let mut text = String::new();
        let mut map = Vec::new();
        let mut i = range.start;
        let end = range.end.min(src.len());
        while i < end {
            let rest = &src[i..end];
            if rest.starts_with("<![CDATA[") {
                let body_start = i + 9;
                let body_end = src[body_start..end]
                    .find("]]>")
                    .map_or(end, |p| body_start + p);
                push_raw(&mut text, &mut map, src, body_start, body_end);
                i = (body_end + 3).min(end);
            } else if rest.starts_with("<!--") {
                i = src[i..end].find("-->").map_or(end, |p| i + p + 3);
            } else if bytes[i] == b'<' {
                let close = src[i..end].find('>').map_or(end, |p| i + p + 1);
                let tag = &src[i..close];
                let name = tag
                    .trim_start_matches(['<', '/'])
                    .split(|c: char| c.is_whitespace() || c == '/' || c == '>')
                    .next()
                    .unwrap_or("");
                let local = name.rsplit(':').next().unwrap_or(name);
                if local.eq_ignore_ascii_case("br") || (local.eq_ignore_ascii_case("p") && tag.starts_with("</")) {
                    text.push('\n');
                    map.push(i);
                }
                i = close;
            } else if bytes[i] == b'&' {
                let semi = src[i..end].find(';').map(|p| i + p);
                let decoded = semi.and_then(|s| decode_entity(&src[i + 1..s]));
                match (semi, decoded) {
                    (Some(s), Some(c)) => {
                        let mut buf = [0u8; 4];
                        for _ in 0..c.encode_utf8(&mut buf).len() {
                            map.push(i);
                        }
                        text.push(c);
                        i = s + 1;
                    }
                    _ => {
                        text.push('&');
                        map.push(i);
                        i += 1;
                    }
                }
            } else {
                // Copy up to the next markup character in one go.
                let next = rest.find(['<', '&']).map_or(end, |p| i + p);
                push_raw(&mut text, &mut map, src, i, next);
                i = next;
            }
        }
        map.push(end);
        Fragment {
            text,
            map,
            raw: range,
        }
    }

    /// Map a decoded byte offset back to a raw XML offset.
    fn raw_at(&self, pos: usize) -> usize {
        self.map
            .get(pos)
            .copied()
            .unwrap_or_else(|| *self.map.last().unwrap_or(&self.raw.end))
    }
}

fn push_raw(text: &mut String, map: &mut Vec<usize>, src: &str, from: usize, to: usize) {
    let s = &src[from..to];
    text.push_str(s);
    map.extend(from..to);
}

fn decode_entity(name: &str) -> Option<char> {
    match name {
        "lt" => Some('<'),
        "gt" => Some('>'),
        "amp" => Some('&'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => {
            let num = name.strip_prefix('#')?;
            let code = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => num.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}

/// Maps spans of a wrapped source (`prefix + fragment + suffix`) onto the XML.
struct Remap<'f> {
    frag: &'f Fragment,
    prefix_len: usize,
}

impl Remap<'_> {
    fn pos(&self, p: usize) -> usize {
        if p < self.prefix_len {
            self.frag.raw.start
        } else {
            self.frag.raw_at((p - self.prefix_len).min(self.frag.text.len()))
        }
    }

    fn span(&self, s: Span) -> Span {
        let start = self.pos(s.start);
        let end = self.pos(s.end).max(start);
        Span::new(start, end)
    }

    /// Rewrite every `Span` inside `value` (any serializable AST node).
    ///
    /// The AST has no visitor, and a hand-written one would have to track every
    /// node type forever; the serde representation is uniform (`{start, end}`
    /// objects), so remap through it.
    fn remap<T: serde::Serialize + serde::de::DeserializeOwned>(&self, value: T) -> T {
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

    fn error(&self, e: &plcc_st::ParseError) -> PlcOpenError {
        use plcc_st::ParseError as P;
        let (message, span) = match e {
            P::UnexpectedToken { span, .. } | P::UnexpectedEof { span, .. } => (e.to_string(), *span),
            P::General { message, span } => (message.clone(), *span),
        };
        let start = self.pos(span.offset());
        let end = self.pos(span.offset() + span.len()).max(start);
        PlcOpenError::new(message, Span::new(start, end))
    }
}

const BODY_PREFIX: &str = "PROGRAM __plcopen_body\n";
const BODY_SUFFIX: &str = "\nEND_PROGRAM\n";
const EXPR_PREFIX: &str = "PROGRAM __plcopen_expr\n__plcopen_expr := ";
const EXPR_SUFFIX: &str = "\n;\nEND_PROGRAM\n";

/// Parse a statement list (an `<ST>` body).
pub(crate) fn parse_statements(frag: &Fragment, errors: &mut Vec<PlcOpenError>) -> Vec<Statement> {
    let wrapped = format!("{BODY_PREFIX}{}{BODY_SUFFIX}", frag.text);
    let (unit, parse_errors) = plcc_st::parse(&wrapped);
    let remap = Remap {
        frag,
        prefix_len: BODY_PREFIX.len(),
    };
    errors.extend(parse_errors.iter().map(|e| remap.error(e)));
    let mut decls = unit.declarations.into_iter();
    match (decls.next(), decls.next()) {
        (Some(Declaration::Program(p)), None) => remap.remap(p.body),
        _ => {
            if parse_errors.is_empty() {
                errors.push(PlcOpenError::new(
                    "an ST body must contain statements only (no POU or type declarations)",
                    Span::from(frag.raw.clone()),
                ));
            }
            Vec::new()
        }
    }
}

/// Parse a single ST expression. `what` names it in diagnostics ("contact variable").
pub(crate) fn parse_expression(
    frag: &Fragment,
    what: &str,
    errors: &mut Vec<PlcOpenError>,
) -> Option<Expression> {
    if frag.text.trim().is_empty() {
        errors.push(PlcOpenError::new(
            format!("empty {what}"),
            Span::from(frag.raw.clone()),
        ));
        return None;
    }
    let wrapped = format!("{EXPR_PREFIX}{}{EXPR_SUFFIX}", frag.text);
    let (unit, parse_errors) = plcc_st::parse(&wrapped);
    let remap = Remap {
        frag,
        prefix_len: EXPR_PREFIX.len(),
    };
    if !parse_errors.is_empty() {
        errors.extend(parse_errors.iter().map(|e| remap.error(e)));
        return None;
    }
    let mut decls = unit.declarations.into_iter();
    if let (Some(Declaration::Program(p)), None) = (decls.next(), decls.next())
        && let [stmt] = p.body.as_slice()
        && let StatementKind::Assignment { value, .. } = &stmt.kind
    {
        return Some(remap.remap(value.clone()));
    }
    errors.push(PlcOpenError::new(
        format!("`{}` is not a single ST expression ({what})", frag.text.trim()),
        Span::from(frag.raw.clone()),
    ));
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_maps_cdata_and_entities_back_to_the_xml() {
        let xml = "<ST><xhtml:p><![CDATA[x := 1;]]></xhtml:p> y := a &lt; b;</ST>";
        let inner = 4..xml.len() - 5;
        let f = Fragment::decode(xml, inner);
        assert_eq!(f.text, "x := 1;\n y := a < b;");
        let x_raw = xml.find("x :=").unwrap();
        assert_eq!(f.raw_at(0), x_raw);
        let lt = f.text.find('<').unwrap();
        assert_eq!(&xml[f.raw_at(lt)..f.raw_at(lt) + 4], "&lt;");
        let b = f.text.rfind('b').unwrap();
        assert_eq!(&xml[f.raw_at(b)..f.raw_at(b) + 1], "b");
    }

    #[test]
    fn expression_spans_point_into_the_xml() {
        let xml = r#"<variable>Motor &amp; 1</variable>"#;
        let f = Fragment::decode(xml, 10..xml.len() - 11);
        let mut errors = Vec::new();
        let e = parse_expression(&f, "test", &mut errors).expect("parses");
        assert!(errors.is_empty());
        assert_eq!(&xml[e.span.start..e.span.end], "Motor &amp; 1");
    }
}
