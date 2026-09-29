// SPDX-License-Identifier: MPL-2.0

use miette::{Diagnostic, LabeledSpan, Severity, SourceSpan};
use plcc_st::Span;
use std::fmt;

/// A diagnostic about an L5X file. The span is a byte range of the L5X source:
/// the element, attribute, rung text or operand the problem is about.
#[derive(Debug, Clone)]
pub struct L5xError {
    pub message: String,
    pub span: SourceSpan,
    pub help: Option<String>,
    /// Warnings do not stop a build: they report something plcc skipped or
    /// approximated (an unused tag of a type it cannot model, a controller
    /// setting with no equivalent).
    pub warning: bool,
}

impl L5xError {
    pub fn new(message: impl Into<String>, span: Span) -> Self {
        L5xError {
            message: message.into(),
            span: span.into(),
            help: None,
            warning: false,
        }
    }

    pub fn warning(message: impl Into<String>, span: Span) -> Self {
        L5xError {
            warning: true,
            ..L5xError::new(message, span)
        }
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    pub fn is_warning(&self) -> bool {
        self.warning
    }
}

impl fmt::Display for L5xError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for L5xError {}

impl Diagnostic for L5xError {
    fn severity(&self) -> Option<Severity> {
        Some(if self.warning {
            Severity::Warning
        } else {
            Severity::Error
        })
    }

    fn help<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        self.help
            .as_ref()
            .map(|h| Box::new(h) as Box<dyn fmt::Display + 'a>)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        Some(Box::new(std::iter::once(LabeledSpan::new_with_span(
            Some("here".into()),
            self.span,
        ))))
    }
}
