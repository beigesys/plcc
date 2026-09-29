// SPDX-License-Identifier: MPL-2.0

//! ST text assembled from the pieces of a TwinCAT object, with a byte map back
//! to the XML file.
//!
//! A `.TcPOU` stores a POU as separate CDATA sections — the declaration, the
//! body, each method's declaration and body, each property accessor — that
//! together are one POU in ST. [`Source`] concatenates them (plus the few
//! keywords TwinCAT leaves implicit, such as `END_FUNCTION_BLOCK`) into one ST
//! text for the ordinary plcc-st parser, remembering for every byte where it
//! came from. Every span of the parsed AST and of every parse error is then
//! remapped onto the `.TcPOU`, so diagnostics from any later stage show the
//! line and column of the XML file.

use crate::error::TwinCatError;
use plcc_st::Span;
use std::ops::Range;

/// ST text plus, for each byte, the file offset it came from.
#[derive(Default)]
pub(crate) struct Source {
    pub text: String,
    /// `map[i]` is the file offset of text byte `i`; one extra entry at the end.
    map: Vec<usize>,
}

impl Source {
    /// Append the character data of `src[range]`: CDATA sections verbatim,
    /// entity references decoded, XML comments and markup skipped.
    pub fn push_xml(&mut self, src: &str, range: Range<usize>) {
        let bytes = src.as_bytes();
        let end = range.end.min(src.len());
        let mut i = range.start;
        while i < end {
            let rest = &src[i..end];
            if rest.starts_with("<![CDATA[") {
                let body = i + 9;
                let body_end = src[body..end].find("]]>").map_or(end, |p| body + p);
                self.push_raw(src, body, body_end);
                i = (body_end + 3).min(end);
            } else if rest.starts_with("<!--") {
                i = src[i..end].find("-->").map_or(end, |p| i + p + 3);
            } else if bytes[i] == b'<' {
                i = src[i..end].find('>').map_or(end, |p| i + p + 1);
            } else if bytes[i] == b'&' {
                let semi = src[i..end].find(';').map(|p| i + p);
                match semi.and_then(|s| decode_entity(&src[i + 1..s]).map(|c| (s, c))) {
                    Some((s, c)) => {
                        for _ in 0..c.len_utf8() {
                            self.map.push(i);
                        }
                        self.text.push(c);
                        i = s + 1;
                    }
                    None => {
                        self.text.push('&');
                        self.map.push(i);
                        i += 1;
                    }
                }
            } else {
                let next = rest.find(['<', '&']).map_or(end, |p| i + p);
                self.push_raw(src, i, next);
                i = next;
            }
        }
    }

    fn push_raw(&mut self, src: &str, from: usize, to: usize) {
        self.text.push_str(&src[from..to]);
        self.map.extend(from..to);
    }

    /// Append text that is not in the file (an implied keyword); its spans
    /// point at `at`.
    pub fn push_synthetic(&mut self, s: &str, at: usize) {
        self.text.push_str(s);
        self.map.extend(std::iter::repeat_n(at, s.len()));
    }

    fn at(&self, pos: usize) -> usize {
        match self.map.get(pos) {
            Some(p) => *p,
            None => self.map.last().map_or(0, |p| p + 1),
        }
    }

    pub fn span(&self, s: Span) -> Span {
        let start = self.at(s.start);
        // The end of a span is exclusive: map its last byte, then step past it.
        let end = if s.end > s.start {
            self.at(s.end - 1) + 1
        } else {
            start
        };
        Span::new(start, end.max(start))
    }

    /// Rewrite every `Span` inside `value` (any serializable AST node) from
    /// text offsets to file offsets.
    ///
    /// The AST has no visitor; its serde form is uniform (`{start, end}`
    /// objects), so remap through that.
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

    pub fn error(&self, e: &plcc_st::ParseError) -> TwinCatError {
        use plcc_st::ParseError as P;
        let (message, span) = match e {
            P::UnexpectedToken { span, .. } | P::UnexpectedEof { span, .. } => {
                (e.to_string(), *span)
            }
            P::General { message, span } => (message.clone(), *span),
        };
        let s = self.span(Span::new(span.offset(), span.offset() + span.len()));
        TwinCatError::new(message, s)
    }
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

/// The first keyword of an ST declaration, uppercased, skipping comments and
/// pragmas: `FUNCTION_BLOCK`, `PROGRAM`, `METHOD`, `TYPE`, ...
pub(crate) fn first_word(text: &str) -> String {
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let rest = &text[i..];
        if b[i].is_ascii_whitespace() {
            i += 1;
        } else if rest.starts_with("//") {
            i += rest.find('\n').unwrap_or(rest.len());
        } else if rest.starts_with("(*") {
            i += rest.find("*)").map_or(rest.len(), |p| p + 2);
        } else if rest.starts_with("/*") {
            i += rest.find("*/").map_or(rest.len(), |p| p + 2);
        } else if rest.starts_with('{') {
            i += rest.find('}').map_or(rest.len(), |p| p + 1);
        } else {
            let word: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            return word.to_uppercase();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cdata_maps_back_to_the_file() {
        let xml = "<ST><![CDATA[x := 1;]]></ST>";
        let mut s = Source::default();
        s.push_xml(xml, 4..xml.len() - 5);
        assert_eq!(s.text, "x := 1;");
        let sp = s.span(Span::new(0, 1));
        assert_eq!(&xml[sp.start..sp.end], "x");
        let sp = s.span(Span::new(5, 6));
        assert_eq!(&xml[sp.start..sp.end], "1");
    }

    #[test]
    fn first_word_skips_comments_and_pragmas() {
        assert_eq!(
            first_word("// doc\n{attribute 'hide'}\n(* x *) function_block FB"),
            "FUNCTION_BLOCK"
        );
    }
}
