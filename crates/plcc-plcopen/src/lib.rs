// SPDX-License-Identifier: MPL-2.0

//! PLCopen XML (IEC 61131-10, TC6 XML v2.01) front end.
//!
//! Reads a PLCopen `<project>` and lowers it into the same [`plcc_st`] AST the
//! Structured Text parser produces, so type checking, codegen, the process image,
//! CONFIGURATION/tasks and every runtime work unchanged:
//!
//! * `<dataTypes>` → `TYPE` declarations (struct, enum, array, subrange, alias,
//!   string, pointer);
//! * `<pous>` → PROGRAM / FUNCTION_BLOCK / FUNCTION with their `<interface>`
//!   variable sections (`address=` becomes `AT`, `initialValue`, `retain`,
//!   `constant`);
//! * bodies: `<ST>` is handed to the ST parser; `<LD>` and `<FBD>` networks are
//!   lowered to ST statements (see `docs/ladder.md`);
//! * `<instances><configurations>` → CONFIGURATION / RESOURCE / TASK / program
//!   instances.
//!
//! Every span in the result is a byte range of the XML file, so diagnostics from
//! any later stage point at the element they are about.

mod embed;
mod error;
mod graph;
mod ladder_read;
mod ladder_write;
mod project;
mod xml;

/// PLCopen LD ↔ the ladder model of `plcc-ladder`.
pub mod ladder {
    pub use crate::ladder_read::read;
    pub use crate::ladder_write::{WriteError, write};
}

pub use error::PlcOpenError;
use plcc_st::{CompilationUnit, Span};

/// Whether `source` looks like a PLCopen XML document (root element `<project>`).
///
/// Cheap sniffing, for inputs whose file extension says nothing: skips the XML
/// declaration, comments and processing instructions, then checks the first
/// element's local name.
pub fn is_plcopen(source: &str) -> bool {
    let mut s = source.trim_start_matches('\u{feff}').trim_start();
    loop {
        if let Some(rest) = s.strip_prefix("<?") {
            match rest.find("?>") {
                Some(p) => s = rest[p + 2..].trim_start(),
                None => return false,
            }
        } else if let Some(rest) = s.strip_prefix("<!--") {
            match rest.find("-->") {
                Some(p) => s = rest[p + 3..].trim_start(),
                None => return false,
            }
        } else if let Some(rest) = s.strip_prefix('<') {
            let name: String = rest
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != '>' && *c != '/')
                .collect();
            return name.rsplit(':').next() == Some("project");
        } else {
            return false;
        }
    }
}

/// Options for [`parse_with`].
#[derive(Default, Clone, Debug)]
pub struct Options {
    /// Put a `(* rung N *)` comment (`network N` in FBD) before the statements
    /// each network lowers to, for printing the lowered ST (`plcc convert`).
    pub annotate_rungs: bool,
}

/// Parse a PLCopen XML project into a compilation unit.
///
/// Like [`plcc_st::parse`], this recovers and reports as many problems as it can;
/// the unit holds everything that lowered cleanly.
pub fn parse(source: &str) -> (CompilationUnit, Vec<PlcOpenError>) {
    parse_with(source, &Options::default())
}

/// [`parse`] with options.
pub fn parse_with(source: &str, opts: &Options) -> (CompilationUnit, Vec<PlcOpenError>) {
    let empty = |errors| {
        (
            CompilationUnit {
                declarations: Vec::new(),
                span: Span::new(0, source.len()),
            },
            errors,
        )
    };
    let doc = match roxmltree::Document::parse(source) {
        Ok(d) => d,
        Err(e) => {
            let at = byte_offset(source, e.pos());
            return empty(vec![PlcOpenError::new(
                format!("malformed XML: {e}"),
                Span::new(at, at),
            )]);
        }
    };
    let root = doc.root_element();
    if xml::name(root) != "project" {
        return empty(vec![PlcOpenError::new(
            format!(
                "not a PLCopen XML project: the root element is <{}>, expected <project>",
                xml::name(root)
            ),
            xml::tag_span(source, root),
        )]);
    }
    let mut lower = project::Lower::new(source);
    lower.annotate = opts.annotate_rungs;
    let declarations = lower.project(root);
    (
        CompilationUnit {
            declarations,
            span: Span::new(0, source.len()),
        },
        lower.errors,
    )
}

fn byte_offset(src: &str, pos: roxmltree::TextPos) -> usize {
    let mut line = 1;
    let mut offset = 0;
    for l in src.split_inclusive('\n') {
        if line == pos.row {
            let col = (pos.col as usize).saturating_sub(1);
            return offset + l.char_indices().nth(col).map_or(l.len(), |(i, _)| i);
        }
        offset += l.len();
        line += 1;
    }
    src.len()
}
