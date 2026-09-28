// SPDX-License-Identifier: MPL-2.0

//! Named constants in types: `ARRAY[1..N] OF INT`, `STRING(LEN)`, `INT(0..MAX)`,
//! where `N`, `LEN`, `MAX` are `VAR_GLOBAL CONSTANT` / `VAR CONSTANT` integers.
//!
//! Type resolution only understood literal bounds. A named one fell back to the
//! lower bound — `ARRAY[1..N]` became a single element, silently, and every
//! access past it hit the wrong memory — and `STRING(LEN)` fell back to the
//! default length. This pass folds every such bound to an integer literal before
//! types are resolved (by both the type checker and codegen), and reports any
//! bound it cannot fold.

use plcc_st::Span;
use plcc_st::ast::*;
use std::collections::HashMap;

type Table = HashMap<String, i128>;

fn eval(e: &Expression, t: &Table) -> Option<i128> {
    match &e.kind {
        ExpressionKind::IntegerLiteral(v) => Some(*v),
        ExpressionKind::Parenthesized(inner) => eval(inner, t),
        ExpressionKind::TypedLiteral { value, .. } => eval(value, t),
        ExpressionKind::Identifier(id) => t.get(&id.name.to_uppercase()).copied(),
        ExpressionKind::UnaryOp {
            op: UnaryOp::Neg,
            operand,
        } => eval(operand, t)?.checked_neg(),
        ExpressionKind::BinaryOp { op, left, right } => {
            let (l, r) = (eval(left, t)?, eval(right, t)?);
            match op {
                BinaryOp::Add => l.checked_add(r),
                BinaryOp::Sub => l.checked_sub(r),
                BinaryOp::Mul => l.checked_mul(r),
                BinaryOp::Div => l.checked_div(r),
                BinaryOp::Mod => l.checked_rem(r),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Add the integer constants declared in `blocks` to `t`, to a fixed point (a
/// constant may be defined from another declared after it).
fn add_constants(blocks: &[&VarBlock], t: &mut Table) {
    loop {
        let mut changed = false;
        for b in blocks.iter().filter(|b| b.is_constant) {
            for d in &b.declarations {
                let key = d.name.name.to_uppercase();
                if t.contains_key(&key) {
                    continue;
                }
                if let Some(v) = d.initializer.as_ref().and_then(|i| eval(i, t)) {
                    t.insert(key, v);
                    changed = true;
                }
            }
        }
        if !changed {
            return;
        }
    }
}

struct Folder {
    changed: bool,
    unresolved: Vec<(usize, Span, String)>,
    current: usize,
}

impl Folder {
    fn expr(&mut self, e: &mut Expression, t: &Table, what: &str) {
        if matches!(e.kind, ExpressionKind::IntegerLiteral(_)) {
            return;
        }
        match eval(e, t) {
            Some(v) => {
                e.kind = ExpressionKind::IntegerLiteral(v);
                self.changed = true;
            }
            None => self.unresolved.push((self.current, e.span, what.to_string())),
        }
    }

    fn spec(&mut self, s: &mut TypeSpec, t: &Table) {
        match &mut s.kind {
            TypeSpecKind::Array { ranges, base } => {
                for r in ranges {
                    self.expr(&mut r.low, t, "array bound");
                    self.expr(&mut r.high, t, "array bound");
                }
                self.spec(base, t);
            }
            TypeSpecKind::StringType {
                length: Some(len), ..
            } => self.expr(len, t, "string length"),
            TypeSpecKind::Subrange { low, high, .. } => {
                self.expr(low, t, "subrange bound");
                self.expr(high, t, "subrange bound");
            }
            TypeSpecKind::Pointer(b) | TypeSpecKind::Reference(b) => self.spec(b, t),
            TypeSpecKind::Struct(fields) | TypeSpecKind::Union(fields) => {
                for f in fields {
                    self.spec(&mut f.type_spec, t);
                }
            }
            _ => {}
        }
    }

    fn blocks(&mut self, blocks: &mut [VarBlock], t: &Table) {
        for b in blocks {
            for d in &mut b.declarations {
                self.spec(&mut d.type_spec, t);
            }
        }
    }
}

/// Fold named integer constants in every type of `unit`. Returns the rewritten
/// unit when anything was folded, and every bound that is not a constant
/// expression (with where it is and what it is).
pub fn fold_type_constants(
    unit: &CompilationUnit,
) -> (Option<CompilationUnit>, Vec<(usize, Span, String)>) {
    let mut globals = Table::new();
    let global_blocks: Vec<&VarBlock> = unit
        .declarations
        .iter()
        .flat_map(|d| match d {
            Declaration::GlobalVarDecl(b) => vec![b],
            Declaration::Configuration(c) => c
                .global_vars
                .iter()
                .chain(c.resources.iter().flat_map(|r| r.global_vars.iter()))
                .collect(),
            _ => Vec::new(),
        })
        .collect();
    add_constants(&global_blocks, &mut globals);

    let mut out = unit.clone();
    let mut f = Folder {
        changed: false,
        unresolved: Vec::new(),
        current: 0,
    };
    let scoped = |blocks: &[VarBlock], outer: &Table| {
        let mut t = outer.clone();
        let refs: Vec<&VarBlock> = blocks.iter().collect();
        add_constants(&refs, &mut t);
        t
    };
    for (i, d) in out.declarations.iter_mut().enumerate() {
        f.current = i;
        match d {
            Declaration::TypeDecl(td) => f.spec(&mut td.type_spec, &globals),
            Declaration::GlobalVarDecl(b) => f.blocks(std::slice::from_mut(b), &globals),
            Declaration::Configuration(c) => {
                f.blocks(&mut c.global_vars, &globals);
                for r in &mut c.resources {
                    f.blocks(&mut r.global_vars, &globals);
                }
            }
            Declaration::Program(p) => {
                let t = scoped(&p.var_blocks, &globals);
                f.blocks(&mut p.var_blocks, &t);
            }
            Declaration::Function(func) => {
                let t = scoped(&func.var_blocks, &globals);
                f.blocks(&mut func.var_blocks, &t);
                if let Some(rt) = &mut func.return_type {
                    f.spec(rt, &t);
                }
            }
            Declaration::FunctionBlock(fb) => {
                let t = scoped(&fb.var_blocks, &globals);
                f.blocks(&mut fb.var_blocks, &t);
                for m in &mut fb.methods {
                    let mt = scoped(&m.var_blocks, &t);
                    f.blocks(&mut m.var_blocks, &mt);
                    if let Some(rt) = &mut m.return_type {
                        f.spec(rt, &mt);
                    }
                }
            }
            Declaration::Class(c) => {
                let t = scoped(&c.var_blocks, &globals);
                f.blocks(&mut c.var_blocks, &t);
                for m in &mut c.methods {
                    let mt = scoped(&m.var_blocks, &t);
                    f.blocks(&mut m.var_blocks, &mt);
                    if let Some(rt) = &mut m.return_type {
                        f.spec(rt, &mt);
                    }
                }
            }
            _ => {}
        }
    }
    (f.changed.then_some(out), f.unresolved)
}
