// SPDX-License-Identifier: MPL-2.0

//! Standard functions written in ST and compiled into a module only when it uses
//! them: REAL/LREAL ⇄ STRING (`real_string.st`). Written in ST because they are
//! string manipulation, which the compiler already lowers; compiled on demand so
//! a program that does not use them carries none of their code; given internal
//! linkage so two plcc objects linked together do not clash.

use super::*;

const REAL_STRING_SRC: &str = include_str!("real_string.st");

/// The names the on-demand sources define that user code may call.
const PUBLIC: &[&str] = &[
    "REAL_TO_STRING",
    "LREAL_TO_STRING",
    "STRING_TO_REAL",
    "STRING_TO_LREAL",
];

fn declared_name(d: &Declaration) -> Option<String> {
    match d {
        Declaration::Function(f) => Some(f.name.name.to_uppercase()),
        Declaration::FunctionBlock(f) => Some(f.name.name.to_uppercase()),
        Declaration::Program(p) => Some(p.name.name.to_uppercase()),
        _ => None,
    }
}

/// `unit` plus the on-demand helpers it calls (unless it defines them itself),
/// and the names of the helper FUNCTIONs added. `None` when none are needed.
pub(super) fn add_on_demand_helpers(
    unit: &CompilationUnit,
) -> Result<Option<(CompilationUnit, Vec<String>)>, CodegenError> {
    // A cheap, conservative use test: the name appears anywhere in the unit.
    let text = serde_json::to_string(unit)
        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
        .to_uppercase();
    let user: std::collections::HashSet<String> =
        unit.declarations.iter().filter_map(declared_name).collect();
    let wanted: Vec<&str> = PUBLIC
        .iter()
        .copied()
        .filter(|n| text.contains(&format!("\"{n}\"")) && !user.contains(*n))
        .collect();
    if wanted.is_empty() {
        return Ok(None);
    }
    let (helpers, errors) = plcc_st::parse(REAL_STRING_SRC);
    if !errors.is_empty() {
        return Err(CodegenError::LlvmError(format!(
            "internal: the on-demand REAL/STRING helpers failed to parse: {errors:?}"
        )));
    }
    let mut out = unit.clone();
    let mut added = Vec::new();
    for d in helpers.declarations {
        let Some(name) = declared_name(&d) else {
            continue;
        };
        if user.contains(&name) {
            continue;
        }
        added.push(name.to_lowercase());
        out.declarations.push(d);
    }
    Ok(Some((out, added)))
}
