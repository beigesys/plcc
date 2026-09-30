// SPDX-License-Identifier: MPL-2.0

//! Diagnostics with file, line and column. Semantic checks name the offending
//! value by its path in the document (`io[3].address`); [`Locator`] turns a
//! path into a byte range using the spans of toml's document parser.

use serde::Serialize;
use std::fmt;
use std::ops::Range;
use toml::de::{DeTable, DeValue};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// One problem in a manifest.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Diagnostic {
    /// The file name the caller gave, if any.
    pub file: Option<String>,
    pub severity: Severity,
    pub message: String,
    /// Dotted path of the value (`target.image.M`, `io[2].address`); empty for
    /// syntax errors.
    pub path: String,
    /// 1-based line and column (in characters) of the start, when known.
    pub line: Option<usize>,
    pub col: Option<usize>,
    /// Byte range in the source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<Range<usize>>,
}

impl Diagnostic {
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let file = self.file.as_deref().unwrap_or("<manifest>");
        match (self.line, self.col) {
            (Some(l), Some(c)) => write!(f, "{file}:{l}:{c}: ")?,
            _ => write!(f, "{file}: ")?,
        }
        let sev = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        write!(f, "{sev}: ")?;
        if !self.path.is_empty() {
            write!(f, "{}: ", self.path)?;
        }
        f.write_str(&self.message)
    }
}

impl std::error::Error for Diagnostic {}

impl miette::Diagnostic for Diagnostic {
    fn severity(&self) -> Option<miette::Severity> {
        Some(match self.severity {
            Severity::Error => miette::Severity::Error,
            Severity::Warning => miette::Severity::Warning,
        })
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = miette::LabeledSpan> + '_>> {
        let span = self.span.clone()?;
        Some(Box::new(std::iter::once(miette::LabeledSpan::new(
            Some(self.message.clone()),
            span.start,
            span.end.saturating_sub(span.start),
        ))))
    }
}

/// One step of a path: a table key or an array index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seg {
    Key(String),
    Index(usize),
}

/// A path into the document.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Path(pub Vec<Seg>);

impl Path {
    pub fn root() -> Self {
        Path(Vec::new())
    }
    pub fn key(&self, k: &str) -> Self {
        let mut p = self.clone();
        p.0.push(Seg::Key(k.to_string()));
        p
    }
    pub fn index(&self, i: usize) -> Self {
        let mut p = self.clone();
        p.0.push(Seg::Index(i));
        p
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, s) in self.0.iter().enumerate() {
            match s {
                Seg::Key(k) if i == 0 => f.write_str(k)?,
                Seg::Key(k) => write!(f, ".{k}")?,
                Seg::Index(n) => write!(f, "[{n}]")?,
            }
        }
        Ok(())
    }
}

/// Byte offset → 1-based (line, column in characters).
pub fn line_col(source: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(source.len());
    let before = &source[..floor_char_boundary(source, offset)];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, col)
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Finds the source range of a path.
pub struct Locator<'i> {
    source: &'i str,
    doc: Option<toml::Spanned<DeTable<'i>>>,
}

impl<'i> Locator<'i> {
    pub fn new(source: &'i str) -> Self {
        Locator {
            source,
            doc: DeTable::parse(source).ok(),
        }
    }

    /// The span of the value at `path`, or of the nearest ancestor that exists.
    pub fn span(&self, path: &Path) -> Option<Range<usize>> {
        let doc = self.doc.as_ref()?;
        let mut best: Option<Range<usize>> = None;
        let mut table: Option<&DeTable<'i>> = Some(doc.get_ref());
        let mut value: Option<&DeValue<'i>> = None;
        for seg in &path.0 {
            match seg {
                Seg::Key(k) => {
                    let t = match (table, value) {
                        (Some(t), _) => t,
                        (None, Some(DeValue::Table(t))) => t,
                        _ => break,
                    };
                    let Some((_, v)) = t.iter().find(|(key, _)| key.get_ref().as_ref() == k.as_str())
                    else {
                        break;
                    };
                    best = Some(v.span());
                    table = None;
                    value = Some(v.get_ref());
                }
                Seg::Index(i) => {
                    let Some(DeValue::Array(a)) = value else { break };
                    let Some(v) = a.get(*i) else { break };
                    best = Some(v.span());
                    table = None;
                    value = Some(v.get_ref());
                }
            }
        }
        best
    }

    pub fn diagnostic(&self, file: Option<&str>, severity: Severity, path: &Path, message: String) -> Diagnostic {
        let span = self.span(path).filter(|s| s.start < self.source.len() || self.source.is_empty());
        let (line, col) = match &span {
            Some(s) => {
                let (l, c) = line_col(self.source, s.start);
                (Some(l), Some(c))
            }
            None => (None, None),
        };
        Diagnostic {
            file: file.map(str::to_string),
            severity,
            message,
            path: path.to_string(),
            line,
            col,
            span,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locate_paths() {
        let src = "[device]\nid = \"x\"\n\n[[io]]\nid = \"a\"\n\n[[io]]\nid = \"b\"\naddress = \"%IX0.0\"\n";
        let loc = Locator::new(src);
        let s = loc.span(&Path::root().key("device").key("id")).unwrap();
        assert_eq!(&src[s.clone()], "\"x\"");
        assert_eq!(line_col(src, s.start), (2, 6));
        let s = loc
            .span(&Path::root().key("io").index(1).key("address"))
            .unwrap();
        assert_eq!(&src[s.clone()], "\"%IX0.0\"");
        assert_eq!(line_col(src, s.start).0, 9);
        // A missing key falls back to its parent.
        let s = loc.span(&Path::root().key("io").index(1).key("label"));
        assert!(s.is_some());
        assert_eq!(Path::root().key("io").index(1).key("label").to_string(), "io[1].label");
    }
}
