// SPDX-License-Identifier: MPL-2.0

//! Small helpers over `roxmltree`, and [`Text`]: character data decoded out of
//! the raw XML (CDATA sections, entities) with a byte map back to the file, so
//! rung text and Structured Text keep exact spans.

use plcc_st::Span;
use roxmltree::Node;
use std::ops::Range;

pub(crate) type XNode<'a, 'i> = Node<'a, 'i>;

pub(crate) fn name<'a>(n: XNode<'a, '_>) -> &'a str {
    n.tag_name().name()
}

/// Span of just the start tag, which reads better in a diagnostic than the
/// whole element.
pub(crate) fn tag_span(src: &str, n: XNode) -> Span {
    let r = n.range();
    let end = src[r.clone()].find('>').map_or(r.end, |p| r.start + p + 1);
    Span::new(r.start, end)
}

pub(crate) fn elements<'a, 'i>(n: XNode<'a, 'i>) -> impl Iterator<Item = XNode<'a, 'i>> {
    n.children().filter(|c| c.is_element())
}

pub(crate) fn child<'a, 'i>(n: XNode<'a, 'i>, local: &str) -> Option<XNode<'a, 'i>> {
    elements(n).find(|c| name(*c) == local)
}

pub(crate) fn children<'a, 'i>(
    n: XNode<'a, 'i>,
    local: &'static str,
) -> impl Iterator<Item = XNode<'a, 'i>> {
    elements(n).filter(move |c| name(*c) == local)
}

pub(crate) fn attr<'a>(n: XNode<'a, '_>, a: &str) -> Option<&'a str> {
    n.attribute(a)
}

pub(crate) fn attr_bool(n: XNode, a: &str) -> bool {
    matches!(
        n.attribute(a).map(|v| v.trim().to_ascii_lowercase()),
        Some(ref v) if v == "true" || v == "1"
    )
}

/// Span of an attribute's value, or of the element's start tag without one.
pub(crate) fn attr_span(src: &str, n: XNode, a: &str) -> Span {
    n.attribute_node(a)
        .map(|at| Span::from(at.range_value()))
        .unwrap_or_else(|| tag_span(src, n))
}

/// Decoded character data plus, for every decoded byte, the XML byte offset it
/// came from.
#[derive(Clone, Debug)]
pub(crate) struct Text {
    pub text: String,
    /// `map[i]` is the raw offset of decoded byte `i`; `map[text.len()]` is the
    /// end of the raw range.
    map: Vec<usize>,
}

impl Text {
    /// Decode `src[range]`: character data with entities and CDATA sections;
    /// comments and any markup are skipped.
    pub fn decode(src: &str, range: Range<usize>) -> Text {
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
                i = src[i..end].find('>').map_or(end, |p| i + p + 1);
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
                let next = rest.find(['<', '&']).map_or(end, |p| i + p);
                push_raw(&mut text, &mut map, src, i, next);
                i = next;
            }
        }
        map.push(end);
        Text { text, map }
    }

    /// The content of an element (between its start and end tags).
    pub fn content(src: &str, n: XNode) -> Text {
        let r = n.range();
        let Some(first) = n.first_child() else {
            return Text::decode(src, r.end..r.end);
        };
        let start = first.range().start;
        let end = src[r.clone()]
            .rfind("</")
            .map_or(r.end, |p| r.start + p)
            .max(start);
        Text::decode(src, start..end)
    }

    /// An attribute value.
    pub fn attr(src: &str, n: XNode, a: &str) -> Option<Text> {
        let at = n.attribute_node(a)?;
        Some(Text::decode(src, at.range_value()))
    }

    /// Raw offset of decoded byte `pos`.
    pub fn raw(&self, pos: usize) -> usize {
        self.map
            .get(pos)
            .copied()
            .unwrap_or_else(|| *self.map.last().unwrap_or(&0))
    }

    /// Raw span of decoded range `r`.
    pub fn span(&self, r: Range<usize>) -> Span {
        let start = self.raw(r.start);
        let end = if r.end > r.start {
            self.raw(r.end - 1) + 1
        } else {
            start
        };
        Span::new(start, end.max(start))
    }

    /// Whether decoded bytes `r` are a contiguous, unescaped copy of the XML
    /// (so an offset inside maps linearly).
    pub fn is_verbatim(&self, r: Range<usize>) -> bool {
        if r.end <= r.start {
            return true;
        }
        let a = self.raw(r.start);
        let b = self.raw(r.end - 1);
        b >= a && b - a == r.end - 1 - r.start
    }

    /// Several texts joined by newlines (the `<Line>`s of an ST routine); a
    /// separator maps to the end of the line before it.
    pub fn join(parts: &[Text]) -> Text {
        let mut text = String::new();
        let mut map = Vec::new();
        for (i, p) in parts.iter().enumerate() {
            if i > 0 {
                text.push('\n');
                map.push(parts[i - 1].map.last().copied().unwrap_or(0));
            }
            text.push_str(&p.text);
            map.extend_from_slice(&p.map[..p.text.len()]);
        }
        map.push(
            parts
                .last()
                .and_then(|p| p.map.last().copied())
                .unwrap_or(0),
        );
        Text { text, map }
    }

    pub fn whole(&self) -> Span {
        self.span(0..self.text.len())
    }
}

fn push_raw(text: &mut String, map: &mut Vec<usize>, src: &str, from: usize, to: usize) {
    text.push_str(&src[from..to]);
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
