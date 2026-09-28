// SPDX-License-Identifier: MPL-2.0

//! LD and FBD bodies → ST statements.

use crate::project::{Lower, PouKind};
use crate::xml::{self, XNode};
use plcc_st::ast::{Statement, VarBlock};

pub(crate) fn lower_body(
    lower: &mut Lower,
    body: XNode,
    _kind: PouKind,
    _var_blocks: &mut Vec<VarBlock>,
) -> Vec<Statement> {
    lower.err(
        format!("<{}> bodies are not supported yet", xml::name(body)),
        xml::tag_span(lower.src, body),
    );
    Vec::new()
}
