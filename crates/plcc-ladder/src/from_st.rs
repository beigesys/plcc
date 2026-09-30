// SPDX-License-Identifier: MPL-2.0

//! Structured Text → the ladder model (IEC dialect).

use crate::model::*;
use plcc_st::ast::*;

/// Every PROGRAM, FUNCTION_BLOCK and FUNCTION of `unit` as a POU whose body is
/// one ST box. Other declarations are reported and left out.
pub fn from_unit(unit: &CompilationUnit) -> (Project, Vec<String>) {
    let mut notes = Vec::new();
    let mut ids = Ids::new();
    let mut project = Project {
        dialect: Dialect::Iec,
        ..Default::default()
    };
    for d in &unit.declarations {
        let (name, kind, ret, blocks, body) = match d {
            Declaration::Program(p) => (&p.name, PouKind::Program, None, &p.var_blocks, &p.body),
            Declaration::FunctionBlock(f) => (
                &f.name,
                PouKind::FunctionBlock,
                None,
                &f.var_blocks,
                &f.body,
            ),
            Declaration::Function(f) => (
                &f.name,
                PouKind::Function,
                f.return_type.as_ref(),
                &f.var_blocks,
                &f.body,
            ),
            other => {
                notes.push(format!(
                    "{} is not a POU: left out of the ladder model",
                    decl_kind(other)
                ));
                continue;
            }
        };
        project.pous.push(Pou {
            id: ids.fresh(),
            name: name.name.clone(),
            kind,
            return_type: ret.map(plcc_st::printer::print_type_spec),
            variables: variables(blocks),
            routines: vec![Routine {
                id: ids.fresh(),
                name: name.name.clone(),
                rungs: vec![Rung {
                    id: ids.fresh(),
                    elements: vec![Element::St(StBox {
                        id: ids.fresh(),
                        code: plcc_st::print_statements(body, 0).trim_end().to_string(),
                        notes: Vec::new(),
                    })],
                    ..Default::default()
                }],
            }],
        });
    }
    (project, notes)
}

fn decl_kind(d: &Declaration) -> &'static str {
    match d {
        Declaration::TypeDecl(_) => "a TYPE",
        Declaration::GlobalVarDecl(_) => "a VAR_GLOBAL block",
        Declaration::Configuration(_) => "a CONFIGURATION",
        Declaration::Class(_) => "a CLASS",
        Declaration::Interface(_) => "an INTERFACE",
        _ => "a declaration",
    }
}

/// The variables of VAR blocks.
pub fn variables(blocks: &[VarBlock]) -> Vec<Variable> {
    let mut out = Vec::new();
    for b in blocks {
        let section = match b.kind {
            VarBlockKind::VarInput => VarSection::Input,
            VarBlockKind::VarOutput => VarSection::Output,
            VarBlockKind::VarInOut => VarSection::InOut,
            VarBlockKind::VarExternal => VarSection::External,
            VarBlockKind::VarTemp => VarSection::Temp,
            VarBlockKind::VarGlobal => VarSection::Global,
            _ => VarSection::Local,
        };
        for d in &b.declarations {
            out.push(Variable {
                name: d.name.name.clone(),
                data_type: plcc_st::printer::print_type_spec(&d.type_spec),
                section,
                initial: d.initializer.as_ref().map(plcc_st::print_expression),
                address: d.at_address.as_ref().map(|a| a.repr.clone()),
                comment: None,
                constant: b.is_constant,
                retain: b.is_retain,
            });
        }
    }
    out
}
