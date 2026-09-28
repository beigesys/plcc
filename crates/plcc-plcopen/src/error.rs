// SPDX-License-Identifier: MPL-2.0

use miette::{Diagnostic, SourceSpan};
use plcc_st::Span;
use thiserror::Error;

/// A diagnostic about a PLCopen XML file. The span is a byte range of the XML
/// source, normally the element (`localId`) the problem is about.
#[derive(Debug, Clone, Error, Diagnostic)]
#[error("{message}")]
pub struct PlcOpenError {
    pub message: String,
    #[label("here")]
    pub span: SourceSpan,
    #[help]
    pub help: Option<String>,
}

impl PlcOpenError {
    pub fn new(message: impl Into<String>, span: Span) -> Self {
        PlcOpenError {
            message: message.into(),
            span: span.into(),
            help: None,
        }
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }
}
