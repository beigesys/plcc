// SPDX-License-Identifier: MPL-2.0

//! Standard functions written in ST and compiled into a module only when it uses
//! them: REAL/LREAL ⇄ STRING (`real_string.st`) and TIME/date → STRING
//! (`time_string.st`). Written in ST because they are string manipulation,
//! which the compiler already lowers; compiled on demand so a program that does
//! not use them carries none of their code; given internal linkage so two plcc
//! objects linked together do not clash.

use super::*;

/// Each on-demand source and the names in it that user code may call.
const SOURCES: &[(&str, &[&str])] = &[
    (
        include_str!("real_string.st"),
        &[
            "REAL_TO_STRING",
            "LREAL_TO_STRING",
            "STRING_TO_REAL",
            "STRING_TO_LREAL",
        ],
    ),
    (
        include_str!("time_string.st"),
        &[
            "TIME_TO_STRING",
            "LTIME_TO_STRING",
            "DATE_TO_STRING",
            "TOD_TO_STRING",
            "DT_TO_STRING",
        ],
    ),
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
    let mut out: Option<CompilationUnit> = None;
    let mut added = Vec::new();
    for (src, public) in SOURCES {
        let wanted = public
            .iter()
            .any(|n| text.contains(&format!("\"{n}\"")) && !user.contains(*n));
        if !wanted {
            continue;
        }
        let (helpers, errors) = plcc_st::parse(src);
        if !errors.is_empty() {
            return Err(CodegenError::LlvmError(format!(
                "internal: an on-demand standard-function source failed to parse: {errors:?}"
            )));
        }
        let target = out.get_or_insert_with(|| unit.clone());
        for d in helpers.declarations {
            let Some(name) = declared_name(&d) else {
                continue;
            };
            if user.contains(&name) || added.contains(&name.to_lowercase()) {
                continue;
            }
            added.push(name.to_lowercase());
            target.declarations.push(d);
        }
    }
    Ok(out.map(|u| (u, added)))
}
