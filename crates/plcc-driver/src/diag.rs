// SPDX-License-Identifier: MPL-2.0

//! Structured diagnostics: any `miette::Diagnostic` from any front end, located
//! in the file it came from, as plain data a browser editor can place.

use serde::Serialize;

/// How bad a diagnostic is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Advice,
}

/// Which stage reported a diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    /// Structured Text parser.
    Parse,
    /// PLCopen XML reader.
    Plcopen,
    /// Rockwell L5X reader.
    L5x,
    /// TwinCAT object / project reader.
    Twincat,
    /// The `io_map` TOML.
    IoMap,
    /// The request itself: a bad path, a missing entry file.
    Input,
    /// The type checker (`plcc check`).
    Typecheck,
    /// Converting between notations and ladder dialects (`plcc convert`).
    Convert,
    /// Code generation and the device manifest a build targets (`plcc compile`).
    Codegen,
}

/// A position in a file. `line` and `col` are 1-based; `col` counts UTF-16 code
/// units, as JavaScript strings and browser editors do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Position {
    pub line: u32,
    pub col: u32,
    /// Byte offset into the file's UTF-8 text.
    pub offset: u32,
    /// Offset in UTF-16 code units (a JavaScript string index).
    pub utf16: u32,
}

/// A source range with an optional label.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Label {
    pub start: Position,
    pub end: Position,
    pub message: Option<String>,
}

/// One diagnostic.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    /// Path of the file, as the caller named it; `None` when the diagnostic is
    /// about the request as a whole.
    pub file: Option<String>,
    pub severity: Severity,
    pub stage: Stage,
    /// The diagnostic's code, when it has one.
    pub code: Option<String>,
    pub message: String,
    pub help: Option<String>,
    /// The primary range (the first label), when the diagnostic has one.
    pub span: Option<Label>,
    /// Every labelled range, the primary one first.
    pub labels: Vec<Label>,
    /// For a ladder model input: the program, routine, rung and element the
    /// diagnostic is about. `span` is then a range in the rung's text or in
    /// the ST box's code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ladder: Option<crate::ladder::LadderRef>,
}

impl Diagnostic {
    /// A diagnostic without a source location.
    pub fn plain(file: Option<&str>, stage: Stage, severity: Severity, message: String) -> Self {
        Diagnostic {
            file: file.map(str::to_string),
            severity,
            stage,
            code: None,
            message,
            help: None,
            span: None,
            labels: Vec::new(),
            ladder: None,
        }
    }

    /// Convert a miette diagnostic whose spans are byte offsets into `source`.
    pub fn from_miette(
        d: &dyn miette::Diagnostic,
        file: &str,
        source: &str,
        stage: Stage,
        severity: Option<Severity>,
    ) -> Self {
        let index = LineIndex::new(source);
        let severity = severity.unwrap_or(match d.severity() {
            Some(miette::Severity::Warning) => Severity::Warning,
            Some(miette::Severity::Advice) => Severity::Advice,
            _ => Severity::Error,
        });
        let labels: Vec<Label> = d
            .labels()
            .map(|it| {
                it.map(|l| {
                    let start = l.offset();
                    let end = start + l.len();
                    Label {
                        start: index.position(start),
                        end: index.position(end),
                        message: l.label().map(str::to_string),
                    }
                })
                .collect()
            })
            .unwrap_or_default();
        Diagnostic {
            file: Some(file.to_string()),
            severity,
            stage,
            code: d.code().map(|c| c.to_string()),
            message: d.to_string(),
            help: d.help().map(|h| h.to_string()),
            span: labels.first().cloned(),
            labels,
            ladder: None,
        }
    }
}

/// Byte offset → line / UTF-16 column.
pub struct LineIndex<'a> {
    source: &'a str,
    line_starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    pub fn new(source: &'a str) -> Self {
        let mut line_starts = vec![0];
        line_starts.extend(source.match_indices('\n').map(|(i, _)| i + 1));
        LineIndex {
            source,
            line_starts,
        }
    }

    /// The position of byte `offset`, clamped into the text and back to a
    /// character boundary.
    pub fn position(&self, offset: usize) -> Position {
        let mut offset = offset.min(self.source.len());
        while !self.source.is_char_boundary(offset) {
            offset -= 1;
        }
        let line = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let line_start = self.line_starts[line];
        let col: usize = self.source[line_start..offset]
            .chars()
            .map(char::len_utf16)
            .sum();
        let utf16: usize = self.source[..offset].chars().map(char::len_utf16).sum();
        Position {
            line: (line + 1) as u32,
            col: (col + 1) as u32,
            offset: offset as u32,
            utf16: utf16 as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_count_utf16_columns() {
        let src = "a\nxé𝄞b";
        let ix = LineIndex::new(src);
        let p = ix.position(src.find('b').unwrap());
        assert_eq!((p.line, p.col), (2, 5)); // x, é (1), 𝄞 (2) → col 5
        assert_eq!(p.utf16, 6);
        // Past the end and inside a character are clamped.
        assert_eq!(ix.position(1000).offset as usize, src.len());
        assert_eq!(ix.position(4).offset, 3);
    }
}
