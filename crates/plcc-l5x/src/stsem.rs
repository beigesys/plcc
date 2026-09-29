// SPDX-License-Identifier: MPL-2.0

//! Logix numeric semantics for Structured Text, applied to the parsed AST.
//!
//! IEC and Logix ST read the same, but two rules differ and are rewritten here
//! (on every POU lowered from the L5X; generated ladder code never contains the
//! patterns, it already calls the prelude):
//!
//! * integer `/` and `MOD`: "the DIV instruction and the operator '/'" share
//!   one definition (1756-RM003 DIV): truncating, and a zero divisor yields
//!   Source A (MOD: 0) instead of a trap. Integer operands are promoted to at
//!   least DINT ("Optimal data type DINT", 1756-RM003 "Structured Text
//!   Components: Expressions").
//! * REAL → integer assignment rounds half to even (1756-RM003 "Data
//!   conversions"), where plcc (like CODESYS) rounds half away from zero.

use crate::types::{Elem, Ty, TypeEnv};
use plcc_st::Span;
use plcc_st::ast::*;
use std::collections::HashMap;

type Vars = HashMap<String, Ty>;

pub(crate) fn apply(decls: &mut [Declaration], env: &TypeEnv) {
    let mut globals = Vars::new();
    for d in decls.iter() {
        if let Declaration::GlobalVarDecl(b) = d {
            add_block(&mut globals, b, env);
        }
    }
    for d in decls.iter_mut() {
        match d {
            Declaration::FunctionBlock(fb) => {
                let mut vars = globals.clone();
                for b in &fb.var_blocks {
                    add_block(&mut vars, b, env);
                }
                let cx = Cx { vars: &vars, env };
                stmts(&mut fb.body, &cx);
                for m in &mut fb.methods {
                    let mut mv = vars.clone();
                    for b in &m.var_blocks {
                        add_block(&mut mv, b, env);
                    }
                    let cx = Cx { vars: &mv, env };
                    stmts(&mut m.body, &cx);
                }
            }
            Declaration::Program(p) => {
                let mut vars = globals.clone();
                for b in &p.var_blocks {
                    add_block(&mut vars, b, env);
                }
                let cx = Cx { vars: &vars, env };
                stmts(&mut p.body, &cx);
            }
            _ => {}
        }
    }
}

fn add_block(vars: &mut Vars, b: &VarBlock, env: &TypeEnv) {
    for v in &b.declarations {
        if let Some(t) = ty_of_spec(&v.type_spec, env) {
            vars.insert(v.name.name.to_ascii_lowercase(), t);
        }
    }
}

fn ty_of_spec(t: &TypeSpec, env: &TypeEnv) -> Option<Ty> {
    match &t.kind {
        TypeSpecKind::Named(n) => {
            if let Some(e) = Elem::parse(&n.name) {
                return Some(Ty::Elem(e));
            }
            env.structs
                .iter()
                .position(|s| s.st.eq_ignore_ascii_case(&n.name))
                .map(Ty::Struct)
        }
        TypeSpecKind::Array { ranges, base } => {
            let b = ty_of_spec(base, env)?;
            Some(Ty::Array(Box::new(b), vec![0; ranges.len()]))
        }
        _ => None,
    }
}

struct Cx<'a> {
    vars: &'a Vars,
    env: &'a TypeEnv,
}

fn promote(a: Elem, b: Elem) -> Option<Elem> {
    if !a.is_num() || !b.is_num() {
        return None;
    }
    Some(if a == Elem::Lreal || b == Elem::Lreal {
        Elem::Lreal
    } else if a == Elem::Real || b == Elem::Real {
        Elem::Real
    } else {
        let w = if a.rank() >= b.rank() { a } else { b };
        if w.rank() < Elem::Dint.rank() {
            Elem::Dint
        } else {
            w
        }
    })
}

impl Cx<'_> {
    fn ty(&self, e: &Expression) -> Option<Ty> {
        match &e.kind {
            ExpressionKind::IntegerLiteral(_) => Some(Ty::Elem(Elem::Dint)),
            ExpressionKind::RealLiteral(_) => Some(Ty::Elem(Elem::Real)),
            ExpressionKind::BoolLiteral(_) => Some(Ty::Elem(Elem::Bool)),
            ExpressionKind::Identifier(i) => self.vars.get(&i.name.to_ascii_lowercase()).cloned(),
            ExpressionKind::Parenthesized(x) => self.ty(x),
            ExpressionKind::MemberAccess { object, member } => {
                if member.name.chars().all(|c| c.is_ascii_digit()) {
                    return Some(Ty::Elem(Elem::Bool));
                }
                match self.ty(object)? {
                    Ty::Struct(i) => self
                        .env
                        .get(i)
                        .fields
                        .iter()
                        .find(|f| f.st.eq_ignore_ascii_case(&member.name))
                        .map(|f| f.ty.clone()),
                    _ => None,
                }
            }
            ExpressionKind::ArrayIndex { array, .. } => match self.ty(array)? {
                Ty::Array(e, _) => Some(*e),
                _ => None,
            },
            ExpressionKind::UnaryOp { op, operand } => {
                let t = self.ty(operand)?;
                match (op, t.elem()) {
                    (UnaryOp::Neg, Some(e)) => promote(e, e).map(Ty::Elem),
                    (UnaryOp::Not, Some(Elem::Bool)) => Some(t),
                    (UnaryOp::Not, Some(e)) => promote(e, e).map(Ty::Elem),
                    _ => None,
                }
            }
            ExpressionKind::BinaryOp { op, left, right } => {
                let a = self.ty(left)?.elem()?;
                let b = self.ty(right)?.elem()?;
                match op {
                    BinaryOp::Equal
                    | BinaryOp::NotEqual
                    | BinaryOp::Less
                    | BinaryOp::LessEqual
                    | BinaryOp::Greater
                    | BinaryOp::GreaterEqual => Some(Ty::Elem(Elem::Bool)),
                    BinaryOp::And | BinaryOp::Or | BinaryOp::Xor
                        if a == Elem::Bool && b == Elem::Bool =>
                    {
                        Some(Ty::Elem(Elem::Bool))
                    }
                    BinaryOp::Power => Some(Ty::Elem(Elem::Real)),
                    _ => promote(a, b).map(Ty::Elem),
                }
            }
            ExpressionKind::FunctionCall { callee, args } => {
                let ExpressionKind::Identifier(f) = &callee.kind else {
                    return None;
                };
                let name = f.name.to_ascii_uppercase();
                let arg = || args.first().and_then(|a| self.ty(&a.value));
                match name.as_str() {
                    "ABS" => arg(),
                    "SQRT" | "SIN" | "COS" | "TAN" | "ASIN" | "ACOS" | "ATAN" | "LN" | "LOG"
                    | "EXP" => match arg()?.elem()? {
                        Elem::Lreal => Some(Ty::Elem(Elem::Lreal)),
                        _ => Some(Ty::Elem(Elem::Real)),
                    },
                    "TRUNC" => Some(Ty::Elem(Elem::Dint)),
                    n if n.starts_with("LX__PUT_") => {
                        let t = n.trim_start_matches("LX__PUT_");
                        let t = t.rsplit_once('_').map_or(t, |(a, _)| a);
                        Elem::parse(t).map(Ty::Elem)
                    }
                    n if n.contains("_TO_") => {
                        n.rsplit("_TO_").next().and_then(Elem::parse).map(Ty::Elem)
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

fn ident_expr(name: &str, span: Span) -> Expression {
    Expression {
        kind: ExpressionKind::Identifier(Ident::new(name, span)),
        span,
    }
}

fn call(name: &str, args: Vec<Expression>, span: Span) -> Expression {
    Expression {
        kind: ExpressionKind::FunctionCall {
            callee: Box::new(ident_expr(name, span)),
            args: args
                .into_iter()
                .map(|value| CallArg {
                    name: None,
                    span: value.span,
                    value,
                    is_output: false,
                    negated: false,
                })
                .collect(),
        },
        span,
    }
}

fn to_lint(e: Expression, from: Elem) -> Expression {
    if from == Elem::Lint {
        return e;
    }
    let sp = e.span;
    call(&format!("{}_TO_LINT", from.st()), vec![e], sp)
}

fn stmts(list: &mut [Statement], cx: &Cx) {
    for s in list {
        stmt(s, cx);
    }
}

fn stmt(s: &mut Statement, cx: &Cx) {
    match &mut s.kind {
        StatementKind::Assignment { target, value } => {
            expr(target, cx);
            expr(value, cx);
            let tt = cx.ty(target).and_then(|t| t.elem());
            let vt = cx.ty(value).and_then(|t| t.elem());
            if let (Some(t), Some(v)) = (tt, vt)
                && t.is_int()
                && v.is_real()
            {
                let sp = value.span;
                let inner = std::mem::replace(value, ident_expr("lx__placeholder", sp));
                let rounded = call("lx__r2l", vec![call("lx__round", vec![inner], sp)], sp);
                *value = if t == Elem::Lint {
                    rounded
                } else {
                    call(&format!("LINT_TO_{}", t.st()), vec![rounded], sp)
                };
            }
        }
        StatementKind::FunctionCall { callee, args } => {
            expr(callee, cx);
            for a in args {
                expr(&mut a.value, cx);
            }
        }
        StatementKind::If {
            condition,
            then_body,
            elsif_branches,
            else_body,
        } => {
            expr(condition, cx);
            stmts(then_body, cx);
            for b in elsif_branches {
                expr(&mut b.condition, cx);
                stmts(&mut b.body, cx);
            }
            if let Some(e) = else_body {
                stmts(e, cx);
            }
        }
        StatementKind::Case {
            selector,
            branches,
            else_body,
        } => {
            expr(selector, cx);
            for b in branches {
                stmts(&mut b.body, cx);
            }
            if let Some(e) = else_body {
                stmts(e, cx);
            }
        }
        StatementKind::For {
            from, to, by, body, ..
        } => {
            expr(from, cx);
            expr(to, cx);
            if let Some(b) = by {
                expr(b, cx);
            }
            stmts(body, cx);
        }
        StatementKind::While { condition, body } => {
            expr(condition, cx);
            stmts(body, cx);
        }
        StatementKind::Repeat { body, until } => {
            stmts(body, cx);
            expr(until, cx);
        }
        StatementKind::Return { value: Some(v) } => expr(v, cx),
        _ => {}
    }
}

fn expr(e: &mut Expression, cx: &Cx) {
    match &mut e.kind {
        ExpressionKind::BinaryOp { left, right, .. } => {
            expr(left, cx);
            expr(right, cx);
        }
        ExpressionKind::UnaryOp { operand, .. } => expr(operand, cx),
        ExpressionKind::Parenthesized(x) => expr(x, cx),
        ExpressionKind::FunctionCall { args, .. } => {
            for a in args {
                expr(&mut a.value, cx);
            }
        }
        ExpressionKind::ArrayIndex { array, indices } => {
            expr(array, cx);
            for i in indices {
                expr(i, cx);
            }
        }
        ExpressionKind::MemberAccess { object, .. } => expr(object, cx),
        _ => {}
    }
    // Integer division and modulo.
    let ExpressionKind::BinaryOp { op, left, right } = &e.kind else {
        return;
    };
    if !matches!(op, BinaryOp::Div | BinaryOp::Mod) {
        return;
    }
    let (Some(a), Some(b)) = (
        cx.ty(left).and_then(|t| t.elem()),
        cx.ty(right).and_then(|t| t.elem()),
    ) else {
        return;
    };
    if !(a.is_int() && b.is_int()) {
        return;
    }
    let Some(result) = promote(a, b) else { return };
    let f = if *op == BinaryOp::Div {
        "lx__div_i"
    } else {
        "lx__mod_i"
    };
    let sp = e.span;
    let placeholder = ident_expr("lx__placeholder", sp);
    let old = std::mem::replace(e, placeholder);
    let ExpressionKind::BinaryOp { left, right, .. } = old.kind else {
        return;
    };
    let inner = call(f, vec![to_lint(*left, a), to_lint(*right, b)], sp);
    *e = if result == Elem::Lint {
        inner
    } else {
        call(&format!("LINT_TO_{}", result.st()), vec![inner], sp)
    };
}
