// SPDX-License-Identifier: MPL-2.0

//! A dialect-neutral ladder model and the translations around it.
//!
//! * [`model`]: projects → POUs → routines → rungs → a series/parallel tree
//!   of contacts, coils, blocks (FBs, functions, Logix instructions), jumps,
//!   returns and ST boxes; serde/JSON with stable ids, for an editor.
//! * [`rll`]: Rockwell rung neutral text ↔ model (Logix dialect).
//! * [`lower`]: model (IEC dialect) → Structured Text AST.
//! * [`catalog`]: the pins of IEC function blocks and Logix instructions.
//!
//! PLCopen XML (IEC LD) is read and written by `plcc-plcopen`, L5X by
//! `plcc-l5x`; both go through this model. `docs/ladder-translation.md`
//! describes the model, the dialect mapping and the round-trip guarantees.

pub mod catalog;
pub mod from_st;
pub mod lower;
pub mod model;
pub mod rll;
pub mod translate;

pub use model::*;

use plcc_st::ast::*;

/// The Structured Text declaration of a variable list, one block per section
/// (`VAR_INPUT ... END_VAR`), as text.
pub fn var_blocks_text(vars: &[Variable]) -> String {
    let order = [
        (VarSection::Input, "VAR_INPUT"),
        (VarSection::Output, "VAR_OUTPUT"),
        (VarSection::InOut, "VAR_IN_OUT"),
        (VarSection::External, "VAR_EXTERNAL"),
        (VarSection::Global, "VAR_GLOBAL"),
        (VarSection::Local, "VAR"),
        (VarSection::Temp, "VAR_TEMP"),
    ];
    let mut out = String::new();
    for (section, kw) in order {
        for (constant, retain) in [(false, false), (true, false), (false, true)] {
            let list: Vec<&Variable> = vars
                .iter()
                .filter(|v| v.section == section && v.constant == constant && v.retain == retain)
                .collect();
            if list.is_empty() {
                continue;
            }
            out.push_str(kw);
            if constant {
                out.push_str(" CONSTANT");
            }
            if retain {
                out.push_str(" RETAIN");
            }
            out.push('\n');
            for v in list {
                out.push_str("    ");
                out.push_str(&v.name);
                if let Some(a) = &v.address {
                    out.push_str(" AT ");
                    out.push_str(a);
                }
                out.push_str(" : ");
                out.push_str(&v.data_type);
                if let Some(i) = &v.initial {
                    out.push_str(" := ");
                    out.push_str(i);
                }
                out.push(';');
                if let Some(c) = &v.comment {
                    out.push_str(&format!(" (* {} *)", c.replace("*)", "* )")));
                }
                out.push('\n');
            }
            out.push_str("END_VAR\n");
        }
    }
    out
}

/// An IEC-dialect project as a Structured Text compilation unit: each POU
/// with its variables and its first routine as the body (further routines
/// become ACTIONs of a PROGRAM or FUNCTION_BLOCK); globals as a VAR_GLOBAL
/// block. Errors name the element.
pub fn to_unit(project: &Project, annotate: bool) -> (CompilationUnit, Vec<lower::LowerError>) {
    let mut decls = Vec::new();
    let mut errors = Vec::new();
    if !project.globals.is_empty() {
        let mut g = project.globals.clone();
        for v in &mut g {
            v.section = VarSection::Global;
        }
        let (u, errs) = plcc_st::parse(&var_blocks_text(&g));
        push_parse_errors(&mut errors, 0, "globals", &errs);
        decls.extend(u.declarations);
    }
    for pou in &project.pous {
        let (d, errs) = pou_decl_in(pou, annotate, &project.globals);
        errors.extend(errs);
        decls.extend(d);
    }
    for text in &project.declarations {
        let (u, errs) = plcc_st::parse(text);
        push_parse_errors(&mut errors, 0, "declaration", &errs);
        decls.extend(u.declarations);
    }
    (
        CompilationUnit {
            declarations: decls,
            span: plcc_st::Span::empty(),
        },
        errors,
    )
}

fn push_parse_errors(
    errors: &mut Vec<lower::LowerError>,
    id: Id,
    what: &str,
    errs: &[plcc_st::ParseError],
) {
    for e in errs {
        errors.push(lower::LowerError {
            element: id,
            message: format!("{what}: {e}"),
        });
    }
}

/// One POU as a declaration.
pub fn pou_decl(pou: &Pou, annotate: bool) -> (Option<Declaration>, Vec<lower::LowerError>) {
    pou_decl_in(pou, annotate, &[])
}

/// [`pou_decl`] for a POU of a project with `globals`: a block whose
/// instance is a global (a Logix controller-scoped TIMER translated to a TON)
/// calls that global, not a hidden local of the same name that would shadow
/// it.
pub fn pou_decl_in(
    pou: &Pou,
    annotate: bool,
    globals: &[Variable],
) -> (Option<Declaration>, Vec<lower::LowerError>) {
    let mut errors = Vec::new();
    let (kw, end) = match pou.kind {
        PouKind::Program => ("PROGRAM", "END_PROGRAM"),
        PouKind::FunctionBlock => ("FUNCTION_BLOCK", "END_FUNCTION_BLOCK"),
        PouKind::Function => ("FUNCTION", "END_FUNCTION"),
    };
    let ret = match (&pou.kind, &pou.return_type) {
        (PouKind::Function, Some(t)) => format!(" : {t}"),
        (PouKind::Function, None) => " : BOOL".to_string(),
        _ => String::new(),
    };
    let text = format!(
        "{kw} {}{ret}\n{}{end}\n",
        pou.name,
        var_blocks_text(&pou.variables)
    );
    let (u, errs) = plcc_st::parse(&text);
    push_parse_errors(&mut errors, pou.id, &format!("POU {}", pou.name), &errs);
    let Some(mut decl) = u.declarations.into_iter().next() else {
        return (None, errors);
    };
    let declared = pou
        .variables
        .iter()
        .chain(globals)
        .map(|v| v.name.clone())
        .collect();
    let opts = lower::Options {
        annotate,
        in_function: pou.kind == PouKind::Function,
        declared,
    };
    let mut hidden = Vec::new();
    let mut bodies = Vec::new();
    let mut opts_k = opts.clone();
    for r in &pou.routines {
        let l = lower::lower_routine(r, &opts_k);
        errors.extend(l.errors);
        for h in &l.hidden {
            opts_k.declared.insert(h.name.name.clone());
        }
        hidden.extend(l.hidden);
        bodies.push((r.name.clone(), l.body));
    }
    let hidden_block = (!hidden.is_empty()).then(|| VarBlock {
        kind: VarBlockKind::Var,
        list_name: None,
        is_constant: false,
        is_retain: false,
        is_non_retain: false,
        declarations: hidden,
        span: plcc_st::Span::empty(),
    });
    // Members carried as ST.
    let (mut methods, mut properties, mut member_actions) = (Vec::new(), Vec::new(), Vec::new());
    if !pou.members.is_empty() {
        let (u, errs) = plcc_st::parse(&format!(
            "FUNCTION_BLOCK _\n{}\nEND_FUNCTION_BLOCK\n",
            pou.members
        ));
        push_parse_errors(
            &mut errors,
            pou.id,
            &format!("POU {} members", pou.name),
            &errs,
        );
        if let Some(Declaration::FunctionBlock(f)) = u.declarations.into_iter().next() {
            methods = f.methods;
            properties = f.properties;
            member_actions = f.actions;
        }
    }
    let mut bodies = bodies.into_iter();
    let main = bodies.next().map(|(_, b)| b).unwrap_or_default();
    let mut actions: Vec<ActionDecl> = bodies
        .map(|(name, body)| ActionDecl {
            name: Ident::new(name, plcc_st::Span::empty()),
            body,
            span: plcc_st::Span::empty(),
        })
        .collect();
    actions.extend(member_actions);
    match &mut decl {
        Declaration::Program(p) => {
            p.body = main;
            p.var_blocks.extend(hidden_block);
            p.actions = actions;
            p.methods = methods;
            p.properties = properties;
        }
        Declaration::FunctionBlock(f) => {
            f.body = main;
            f.var_blocks.extend(hidden_block);
            f.actions = actions;
            f.methods = methods;
            f.properties = properties;
        }
        Declaration::Function(f) => {
            f.body = main;
            f.var_blocks.extend(hidden_block);
            if !actions.is_empty() {
                errors.push(lower::LowerError {
                    element: pou.id,
                    message: "a FUNCTION has one routine".into(),
                });
            }
        }
        _ => {}
    }
    (Some(decl), errors)
}
