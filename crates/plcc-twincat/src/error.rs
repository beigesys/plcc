// SPDX-License-Identifier: MPL-2.0

use miette::{Diagnostic, LabeledSpan, Severity, SourceSpan};
use plcc_st::Span;
use thiserror::Error;

/// A diagnostic about a TwinCAT file. The span is a byte range of that file
/// (for ST code: the text inside its CDATA section), so line and column are the
/// ones the TwinCAT XML has.
#[derive(Debug, Clone, Error)]
#[error("{message}")]
pub struct TwinCatError {
    pub message: String,
    pub span: SourceSpan,
    pub help: Option<String>,
    /// A warning is reported but does not stop the build (e.g. an ignored
    /// SFC transition).
    pub warning: bool,
}

impl TwinCatError {
    pub fn new(message: impl Into<String>, span: Span) -> Self {
        TwinCatError {
            message: message.into(),
            span: span.into(),
            help: None,
            warning: false,
        }
    }

    pub fn warning(message: impl Into<String>, span: Span) -> Self {
        TwinCatError {
            warning: true,
            ..TwinCatError::new(message, span)
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

impl Diagnostic for TwinCatError {
    fn severity(&self) -> Option<Severity> {
        Some(if self.warning {
            Severity::Warning
        } else {
            Severity::Error
        })
    }

    fn help<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        self.help
            .as_ref()
            .map(|h| Box::new(h) as Box<dyn std::fmt::Display>)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        Some(Box::new(std::iter::once(LabeledSpan::new_with_span(
            Some("here".to_string()),
            self.span,
        ))))
    }
}
