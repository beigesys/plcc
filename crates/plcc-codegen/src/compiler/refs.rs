// SPDX-License-Identifier: MPL-2.0

//! CODESYS `REFERENCE TO`.
//!
//! A reference is stored as an address, like a POINTER, but it is used as the
//! value it refers to: `r := r + 10` adds to the referenced variable, `w := r`
//! reads it. `r REF= x` binds it (the parser turns that into `r := __REF_OF(x)`),
//! and `__ISVALIDREF(r)` tests it for NULL.
//!
//! `REFERENCE TO` used to be parsed as `POINTER TO`, so `r := r + 10` was pointer
//! arithmetic on the address and `w := r` stored the address — silently — and
//! `REF=` did not parse at all.
//!
//! This pass rewrites every use of a reference variable `r` in a POU body into
//! `r^`, except where the address itself is meant (the target of `REF=`, the
//! argument of `__ISVALIDREF`). Codegen then needs nothing special beyond
//! lowering `__REF_OF` as `ADR`.

use super::*;
use std::collections::HashSet;

fn refs_of(blocks: &[VarBlock]) -> (HashSet<String>, HashSet<String>) {
    let mut refs = HashSet::new();
    let mut others = HashSet::new();
    for b in blocks {
        for d in &b.declarations {
            let name = d.name.name.to_uppercase();
            if matches!(d.type_spec.kind, TypeSpecKind::Reference(_)) {
                refs.insert(name);
            } else {
                others.insert(name);
            }
        }
    }
    (refs, others)
}

/// `outer` refs, shadowed by and extended with `blocks`' own declarations.
fn scope(outer: &HashSet<String>, blocks: &[VarBlock]) -> HashSet<String> {
    let (refs, others) = refs_of(blocks);
    outer
        .iter()
        .filter(|n| !others.contains(*n))
        .cloned()
        .chain(refs)
        .collect()
}

fn is_call_to(e: &Expression, name: &str) -> bool {
    matches!(&e.kind, ExpressionKind::FunctionCall { callee, .. }
        if matches!(&callee.kind, ExpressionKind::Identifier(id) if id.name.eq_ignore_ascii_case(name)))
}

fn expr(e: &mut Expression, refs: &HashSet<String>) {
    if let ExpressionKind::Identifier(id) = &e.kind {
        if refs.contains(&id.name.to_uppercase()) {
            let inner = e.clone();
            e.kind = ExpressionKind::Dereference(Box::new(inner));
        }
        return;
    }
    match &mut e.kind {
        ExpressionKind::FunctionCall { callee, args } => {
            let keep_address = matches!(&callee.kind, ExpressionKind::Identifier(id)
                if id.name.eq_ignore_ascii_case("__ISVALIDREF"));
            if !keep_address {
                if !matches!(callee.kind, ExpressionKind::Identifier(_)) {
                    expr(callee, refs);
                }
                for a in args {
                    expr(&mut a.value, refs);
                }
            }
        }
        ExpressionKind::BinaryOp { left, right, .. } => {
            expr(left, refs);
            expr(right, refs);
        }
        ExpressionKind::UnaryOp { operand, .. } => expr(operand, refs),
        ExpressionKind::MemberAccess { object, .. } => expr(object, refs),
        ExpressionKind::ArrayIndex { array, indices } => {
            expr(array, refs);
            for i in indices {
                expr(i, refs);
            }
        }
        ExpressionKind::Dereference(inner) | ExpressionKind::Parenthesized(inner) => {
            expr(inner, refs)
        }
        ExpressionKind::TypedLiteral { .. } => {}
        _ => {}
    }
}

fn stmts(body: &mut [Statement], refs: &HashSet<String>) {
    for s in body {
        match &mut s.kind {
            StatementKind::Assignment { target, value } => {
                // `r REF= x` rebinds r itself.
                if !is_call_to(value, "__REF_OF") {
                    expr(target, refs);
                }
                expr(value, refs);
            }
            StatementKind::FunctionCall { callee, args } => {
                if !matches!(callee.kind, ExpressionKind::Identifier(_))
                    || matches!(&callee.kind, ExpressionKind::Identifier(id)
                        if refs.contains(&id.name.to_uppercase()))
                {
                    expr(callee, refs);
                }
                for a in args {
                    expr(&mut a.value, refs);
                }
            }
            StatementKind::If {
                condition,
                then_body,
                elsif_branches,
                else_body,
            } => {
                expr(condition, refs);
                stmts(then_body, refs);
                for b in elsif_branches {
                    expr(&mut b.condition, refs);
                    stmts(&mut b.body, refs);
                }
                if let Some(e) = else_body {
                    stmts(e, refs);
                }
            }
            StatementKind::Case {
                selector,
                branches,
                else_body,
            } => {
                expr(selector, refs);
                for b in branches {
                    for l in &mut b.labels {
                        match l {
                            CaseLabel::Value(v) => expr(v, refs),
                            CaseLabel::Range(a, b) => {
                                expr(a, refs);
                                expr(b, refs);
                            }
                        }
                    }
                    stmts(&mut b.body, refs);
                }
                if let Some(e) = else_body {
                    stmts(e, refs);
                }
            }
            StatementKind::For {
                from, to, by, body, ..
            } => {
                expr(from, refs);
                expr(to, refs);
                if let Some(b) = by {
                    expr(b, refs);
                }
                stmts(body, refs);
            }
            StatementKind::While { condition, body } => {
                expr(condition, refs);
                stmts(body, refs);
            }
            StatementKind::Repeat { body, until } => {
                stmts(body, refs);
                expr(until, refs);
            }
            StatementKind::Return { value: Some(v) } => expr(v, refs),
            _ => {}
        }
    }
}

fn methods(ms: &mut [MethodDecl], outer: &HashSet<String>) {
    for m in ms {
        let s = scope(outer, &m.var_blocks);
        stmts(&mut m.body, &s);
    }
}

fn has_reference(unit: &CompilationUnit) -> bool {
    let any = |blocks: &[VarBlock]| {
        blocks.iter().any(|b| {
            b.declarations
                .iter()
                .any(|d| matches!(d.type_spec.kind, TypeSpecKind::Reference(_)))
        })
    };
    unit.declarations.iter().any(|d| match d {
        Declaration::Program(p) => any(&p.var_blocks),
        Declaration::Function(f) => any(&f.var_blocks),
        Declaration::FunctionBlock(fb) => {
            any(&fb.var_blocks) || fb.methods.iter().any(|m| any(&m.var_blocks))
        }
        Declaration::Class(c) => {
            any(&c.var_blocks) || c.methods.iter().any(|m| any(&m.var_blocks))
        }
        Declaration::GlobalVarDecl(b) => any(std::slice::from_ref(b)),
        _ => false,
    })
}

/// The unit with every use of a REFERENCE variable made explicit (`r` → `r^`),
/// or `None` when it declares no references.
pub(super) fn desugar_references(unit: &CompilationUnit) -> Option<CompilationUnit> {
    if !has_reference(unit) {
        return None;
    }
    let mut out = unit.clone();
    let globals: HashSet<String> = out
        .declarations
        .iter()
        .filter_map(|d| match d {
            Declaration::GlobalVarDecl(b) => Some(refs_of(std::slice::from_ref(b)).0),
            _ => None,
        })
        .flatten()
        .collect();
    for d in &mut out.declarations {
        match d {
            Declaration::Program(p) => {
                let s = scope(&globals, &p.var_blocks);
                stmts(&mut p.body, &s);
            }
            Declaration::Function(f) => {
                let s = scope(&globals, &f.var_blocks);
                stmts(&mut f.body, &s);
            }
            Declaration::FunctionBlock(fb) => {
                let s = scope(&globals, &fb.var_blocks);
                stmts(&mut fb.body, &s);
                methods(&mut fb.methods, &s);
            }
            Declaration::Class(c) => {
                let s = scope(&globals, &c.var_blocks);
                methods(&mut c.methods, &s);
            }
            _ => {}
        }
    }
    Some(out)
}

/// Every `REFERENCE TO` VAR_INPUT of an FB or CLASS, as (POU, input), uppercase.
pub(super) fn reference_inputs(unit: &CompilationUnit) -> HashSet<(String, String)> {
    let mut out = HashSet::new();
    for d in &unit.declarations {
        let (name, blocks) = match d {
            Declaration::FunctionBlock(fb) => (&fb.name.name, &fb.var_blocks),
            Declaration::Class(c) => (&c.name.name, &c.var_blocks),
            _ => continue,
        };
        for b in blocks.iter().filter(|b| b.kind == VarBlockKind::VarInput) {
            for v in &b.declarations {
                if matches!(v.type_spec.kind, TypeSpecKind::Reference(_)) {
                    out.insert((name.to_uppercase(), v.name.name.to_uppercase()));
                }
            }
        }
    }
    out
}
