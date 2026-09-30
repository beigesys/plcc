// SPDX-License-Identifier: MPL-2.0

//! IEC ladder ↔ Logix ladder.

use crate::model::*;

/// Translate `p` to the dialect `to`; the warnings name each element whose
/// behaviour differs between the dialects.
pub fn translate(p: &Project, to: Dialect) -> (Project, Vec<String>) {
    if p.dialect == to {
        return (p.clone(), Vec::new());
    }
    (
        p.clone(),
        vec![format!(
            "translation from {:?} to {to:?} ladder is not available yet",
            p.dialect
        )],
    )
}
