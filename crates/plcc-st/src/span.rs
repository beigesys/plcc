// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Serialize};

/// Byte-offset span into source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub fn merge(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    pub fn empty() -> Self {
        Self { start: 0, end: 0 }
    }
}

impl From<std::ops::Range<usize>> for Span {
    fn from(r: std::ops::Range<usize>) -> Self {
        Self {
            start: r.start,
            end: r.end,
        }
    }
}

impl From<Span> for miette::SourceSpan {
    fn from(s: Span) -> Self {
        miette::SourceSpan::new(s.start.into(), s.end - s.start)
    }
}
