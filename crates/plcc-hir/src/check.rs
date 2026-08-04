// SPDX-License-Identifier: MPL-2.0
#![allow(unused_assignments, unused_variables)]

use crate::scope::{PouInfo, PouKind, Scope, SymbolTable, VarInfo};
use crate::types::{IecType, TypeRegistry};
use miette::Diagnostic;
use plcc_st::Span;
use plcc_st::ast::*;
use thiserror::Error;

#[derive(Debug, Clone, Error, Diagnostic)]
pub enum CheckError {
    #[error("undefined variable '{name}'")]
    UndefinedVariable {
        name: String,
        #[label("not defined")]
        span: miette::SourceSpan,
    },

    #[error("undefined type '{name}'")]
    UndefinedType {
        name: String,
        #[label("unknown type")]
        span: miette::SourceSpan,
    },

    #[error("type mismatch: expected {expected}, found {found}")]
    TypeMismatch {
        expected: String,
        found: String,
        #[label("type mismatch")]
        span: miette::SourceSpan,
    },

    #[error("cannot assign to constant '{name}'")]
    AssignToConstant {
        name: String,
        #[label("constant")]
        span: miette::SourceSpan,
    },

    #[error("undefined function or function block '{name}'")]
    UndefinedPou {
        name: String,
        #[label("not defined")]
        span: miette::SourceSpan,
    },

    #[error("{message}")]
    General {
        message: String,
        #[label("{message}")]
        span: miette::SourceSpan,
    },
}

pub struct TypeChecker {
    pub symbols: SymbolTable,
    pub types: TypeRegistry,
    pub errors: Vec<CheckError>,
}

impl TypeChecker {
    pub fn new() -> Self {
        Self {
            symbols: SymbolTable::new(),
            types: TypeRegistry::new(),
            errors: Vec::new(),
        }
    }

    pub fn check(mut self, unit: &CompilationUnit) -> (SymbolTable, Vec<CheckError>) {
        // First pass: register all POUs and types
        for decl in &unit.declarations {
            self.register_declaration(decl);
        }

        // Second pass: type-check bodies
        for decl in &unit.declarations {
            self.check_declaration(decl);
        }

        (self.symbols, self.errors)
    }

    fn register_declaration(&mut self, decl: &Declaration) {
        match decl {
            Declaration::Program(p) => {
                let info = self.build_pou_info(PouKind::Program, &p.name, None, &p.var_blocks);
                self.symbols.register_pou(info);
            }
            Declaration::Function(f) => {
                let ret = f.return_type.as_ref().map(|t| self.resolve_type_spec(t));
                let info =
                    self.build_pou_info(PouKind::Function, &f.name, ret.as_ref(), &f.var_blocks);
                self.symbols.register_pou(info);
            }
            Declaration::FunctionBlock(fb) => {
                let info =
                    self.build_pou_info(PouKind::FunctionBlock, &fb.name, None, &fb.var_blocks);
                self.symbols.register_pou(info);
                // Register FB as a type
                self.types.register(
                    fb.name.name.to_uppercase(),
                    IecType::FbInstance(fb.name.name.clone()),
                );
            }
            Declaration::TypeDecl(td) => {
                let ty = self.resolve_type_spec(&td.type_spec);
                self.types.register(td.name.name.to_uppercase(), ty);
            }
            _ => {}
        }
    }

    fn build_pou_info(
        &mut self,
        kind: PouKind,
        name: &Ident,
        return_type: Option<&IecType>,
        var_blocks: &[VarBlock],
    ) -> PouInfo {
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        let mut in_outs = Vec::new();
        let mut locals = Vec::new();

        for block in var_blocks {
            for decl in &block.declarations {
                let ty = self.resolve_type_spec(&decl.type_spec);
                let pair = (decl.name.name.clone(), ty);
                match block.kind {
                    VarBlockKind::VarInput => inputs.push(pair),
                    VarBlockKind::VarOutput => outputs.push(pair),
                    VarBlockKind::VarInOut => in_outs.push(pair),
                    _ => locals.push(pair),
                }
            }
        }

        PouInfo {
            kind,
            name: name.name.clone(),
            return_type: return_type.cloned(),
            inputs,
            outputs,
            in_outs,
            locals,
        }
    }

    fn check_declaration(&mut self, decl: &Declaration) {
        match decl {
            Declaration::Program(p) => {
                let mut scope = self.build_scope(&p.var_blocks);
                // Program name can be assigned to (for return value pattern)
                self.check_statement_list(&p.body, &mut scope);
            }
            Declaration::Function(f) => {
                let mut scope = self.build_scope(&f.var_blocks);
                // Function name is the return variable
                if let Some(ret_type) = f.return_type.as_ref().map(|t| self.resolve_type_spec(t)) {
                    scope.define(
                        f.name.name.clone(),
                        VarInfo {
                            ty: ret_type,
                            is_constant: false,
                            is_input: false,
                            is_output: false,
                            is_in_out: false,
                        },
                    );
                }
                self.check_statement_list(&f.body, &mut scope);
            }
            Declaration::FunctionBlock(fb) => {
                let mut scope = self.build_scope(&fb.var_blocks);
                self.check_statement_list(&fb.body, &mut scope);
            }
            _ => {}
        }
    }

    fn build_scope(&mut self, var_blocks: &[VarBlock]) -> Scope {
        let mut scope = Scope::new();
        for block in var_blocks {
            for decl in &block.declarations {
                let ty = self.resolve_type_spec(&decl.type_spec);
                scope.define(
                    decl.name.name.clone(),
                    VarInfo {
                        ty,
                        is_constant: block.is_constant,
                        is_input: block.kind == VarBlockKind::VarInput,
                        is_output: block.kind == VarBlockKind::VarOutput,
                        is_in_out: block.kind == VarBlockKind::VarInOut,
                    },
                );
            }
        }
        scope
    }

    fn check_statement_list(&mut self, stmts: &[Statement], scope: &mut Scope) {
        for stmt in stmts {
            self.check_statement(stmt, scope);
        }
    }

    fn check_statement(&mut self, stmt: &Statement, scope: &mut Scope) {
        match &stmt.kind {
            StatementKind::Assignment { target, value } => {
                let target_ty = self.check_expression(target, scope);
                let value_ty = self.check_expression(value, scope);

                // Check assignability
                if let ExpressionKind::Identifier(ident) = &target.kind {
                    if let Some(info) = scope.lookup(&ident.name) {
                        if info.is_constant {
                            self.errors.push(CheckError::AssignToConstant {
                                name: ident.name.clone(),
                                span: target.span.into(),
                            });
                        }
                    }
                }

                // Check type compatibility
                if target_ty != IecType::Void
                    && value_ty != IecType::Void
                    && target_ty != value_ty
                    && !value_ty.can_implicit_convert_to(&target_ty)
                {
                    self.errors.push(CheckError::TypeMismatch {
                        expected: target_ty.to_string(),
                        found: value_ty.to_string(),
                        span: value.span.into(),
                    });
                }
            }
            StatementKind::If {
                condition,
                then_body,
                elsif_branches,
                else_body,
            } => {
                let cond_ty = self.check_expression(condition, scope);
                if cond_ty != IecType::Void && cond_ty != IecType::Bool {
                    self.errors.push(CheckError::TypeMismatch {
                        expected: "BOOL".into(),
                        found: cond_ty.to_string(),
                        span: condition.span.into(),
                    });
                }
                self.check_statement_list(then_body, scope);
                for branch in elsif_branches {
                    let ety = self.check_expression(&branch.condition, scope);
                    if ety != IecType::Void && ety != IecType::Bool {
                        self.errors.push(CheckError::TypeMismatch {
                            expected: "BOOL".into(),
                            found: ety.to_string(),
                            span: branch.condition.span.into(),
                        });
                    }
                    self.check_statement_list(&branch.body, scope);
                }
                if let Some(body) = else_body {
                    self.check_statement_list(body, scope);
                }
            }
            StatementKind::For {
                variable,
                from,
                to,
                by,
                body,
            } => {
                // Variable should be numeric
                if let Some(info) = scope.lookup(&variable.name) {
                    if !info.ty.is_any_num() {
                        self.errors.push(CheckError::TypeMismatch {
                            expected: "numeric type".into(),
                            found: info.ty.to_string(),
                            span: variable.span.into(),
                        });
                    }
                }
                self.check_expression(from, scope);
                self.check_expression(to, scope);
                if let Some(by_expr) = by {
                    self.check_expression(by_expr, scope);
                }
                self.check_statement_list(body, scope);
            }
            StatementKind::While { condition, body } => {
                let cond_ty = self.check_expression(condition, scope);
                if cond_ty != IecType::Void && cond_ty != IecType::Bool {
                    self.errors.push(CheckError::TypeMismatch {
                        expected: "BOOL".into(),
                        found: cond_ty.to_string(),
                        span: condition.span.into(),
                    });
                }
                self.check_statement_list(body, scope);
            }
            StatementKind::Repeat { body, until } => {
                self.check_statement_list(body, scope);
                let until_ty = self.check_expression(until, scope);
                if until_ty != IecType::Void && until_ty != IecType::Bool {
                    self.errors.push(CheckError::TypeMismatch {
                        expected: "BOOL".into(),
                        found: until_ty.to_string(),
                        span: until.span.into(),
                    });
                }
            }
            StatementKind::Case {
                selector,
                branches,
                else_body,
            } => {
                self.check_expression(selector, scope);
                for branch in branches {
                    self.check_statement_list(&branch.body, scope);
                }
                if let Some(body) = else_body {
                    self.check_statement_list(body, scope);
                }
            }
            StatementKind::FunctionCall { callee, args } => {
                self.check_expression(callee, scope);
                for arg in args {
                    self.check_expression(&arg.value, scope);
                }
            }
            StatementKind::Return { value } => {
                if let Some(val) = value {
                    self.check_expression(val, scope);
                }
            }
            StatementKind::Exit | StatementKind::Continue | StatementKind::Empty => {}
        }
    }

    fn check_expression(&mut self, expr: &Expression, scope: &Scope) -> IecType {
        match &expr.kind {
            ExpressionKind::IntegerLiteral(v) => {
                // Use the smallest signed type that fits
                let v = *v;
                if v >= i8::MIN as i128 && v <= i8::MAX as i128 {
                    IecType::Sint
                } else if v >= i16::MIN as i128 && v <= i16::MAX as i128 {
                    IecType::Int
                } else if v >= i32::MIN as i128 && v <= i32::MAX as i128 {
                    IecType::Dint
                } else {
                    IecType::Lint
                }
            }
            ExpressionKind::RealLiteral(_) => IecType::Lreal,
            ExpressionKind::BoolLiteral(_) => IecType::Bool,
            ExpressionKind::StringLiteral(_) => IecType::StringType { max_len: None },
            ExpressionKind::WstringLiteral(_) => IecType::WstringType { max_len: None },
            ExpressionKind::TimeLiteral(_) => IecType::Time,
            ExpressionKind::DateLiteral(_) => IecType::Date,
            ExpressionKind::TodLiteral(_) => IecType::Tod,
            ExpressionKind::DtLiteral(_) => IecType::Dt,
            ExpressionKind::DirectVariable(_) => IecType::Void, // Needs context
            ExpressionKind::Identifier(ident) => {
                if let Some(info) = scope.lookup(&ident.name) {
                    info.ty.clone()
                } else if self.symbols.lookup_pou(&ident.name).is_some() {
                    IecType::Void // It's a POU name
                } else {
                    // Don't error for now — could be a FB instance member
                    IecType::Void
                }
            }
            ExpressionKind::TypedLiteral { type_name, value } => {
                self.check_expression(value, scope);
                self.types
                    .resolve(&type_name.name)
                    .unwrap_or(IecType::Unresolved(type_name.name.clone()))
            }
            ExpressionKind::BinaryOp { op, left, right } => {
                let left_ty = self.check_expression(left, scope);
                let right_ty = self.check_expression(right, scope);
                self.check_binary_op(*op, &left_ty, &right_ty, expr.span)
            }
            ExpressionKind::UnaryOp { op, operand } => {
                let ty = self.check_expression(operand, scope);
                match op {
                    UnaryOp::Not => {
                        if ty.is_any_bit() || ty == IecType::Bool || ty == IecType::Void {
                            ty
                        } else {
                            self.errors.push(CheckError::TypeMismatch {
                                expected: "ANY_BIT".into(),
                                found: ty.to_string(),
                                span: operand.span.into(),
                            });
                            IecType::Void
                        }
                    }
                    UnaryOp::Neg => {
                        if ty.is_any_num() || ty == IecType::Void {
                            ty
                        } else {
                            self.errors.push(CheckError::TypeMismatch {
                                expected: "ANY_NUM".into(),
                                found: ty.to_string(),
                                span: operand.span.into(),
                            });
                            IecType::Void
                        }
                    }
                }
            }
            ExpressionKind::FunctionCall { callee, args } => {
                for arg in args {
                    self.check_expression(&arg.value, scope);
                }
                // Return type of function call
                if let ExpressionKind::Identifier(ident) = &callee.kind {
                    if let Some(pou) = self.symbols.lookup_pou(&ident.name) {
                        pou.return_type.clone().unwrap_or(IecType::Void)
                    } else {
                        IecType::Void
                    }
                } else {
                    self.check_expression(callee, scope);
                    IecType::Void
                }
            }
            ExpressionKind::MemberAccess { object, member } => {
                let _obj_ty = self.check_expression(object, scope);
                // TODO: resolve struct/FB member types
                IecType::Void
            }
            ExpressionKind::ArrayIndex { array, indices } => {
                let arr_ty = self.check_expression(array, scope);
                for idx in indices {
                    self.check_expression(idx, scope);
                }
                match arr_ty {
                    IecType::Array { element_type, .. } => *element_type,
                    _ => IecType::Void,
                }
            }
            ExpressionKind::Dereference(inner) => {
                let ty = self.check_expression(inner, scope);
                match ty {
                    IecType::Pointer(base) => *base,
                    _ => IecType::Void,
                }
            }
            ExpressionKind::Parenthesized(inner) => self.check_expression(inner, scope),
            // An aggregate only ever appears as a declaration's initial value, where
            // the declared type governs; it has no type of its own. Its entries are
            // still checked so a bad expression inside one is still reported.
            ExpressionKind::ArrayInitializer(elements) => {
                for elem in elements {
                    self.check_expression(&elem.value, scope);
                }
                IecType::Void
            }
        }
    }

    fn check_binary_op(
        &mut self,
        op: BinaryOp,
        left: &IecType,
        right: &IecType,
        span: Span,
    ) -> IecType {
        // Skip check if either side is void (unknown/error)
        if *left == IecType::Void || *right == IecType::Void {
            return IecType::Void;
        }

        match op {
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => {
                if left.is_any_num() && right.is_any_num() {
                    // Return the wider type
                    if left.is_any_real() || right.is_any_real() {
                        if matches!(left, IecType::Lreal) || matches!(right, IecType::Lreal) {
                            IecType::Lreal
                        } else {
                            IecType::Real
                        }
                    } else {
                        // Both integers — return wider
                        if left.bit_size() >= right.bit_size() {
                            left.clone()
                        } else {
                            right.clone()
                        }
                    }
                } else {
                    self.errors.push(CheckError::TypeMismatch {
                        expected: "ANY_NUM".into(),
                        found: format!("{left} and {right}"),
                        span: span.into(),
                    });
                    IecType::Void
                }
            }
            BinaryOp::Power => {
                if left.is_any_real() || right.is_any_num() {
                    IecType::Lreal
                } else {
                    IecType::Void
                }
            }
            BinaryOp::Equal
            | BinaryOp::NotEqual
            | BinaryOp::Less
            | BinaryOp::LessEqual
            | BinaryOp::Greater
            | BinaryOp::GreaterEqual => IecType::Bool,
            BinaryOp::And | BinaryOp::Or | BinaryOp::Xor => {
                if (left.is_any_bit() || *left == IecType::Bool)
                    && (right.is_any_bit() || *right == IecType::Bool)
                {
                    if *left == IecType::Bool && *right == IecType::Bool {
                        IecType::Bool
                    } else {
                        left.clone()
                    }
                } else {
                    self.errors.push(CheckError::TypeMismatch {
                        expected: "ANY_BIT".into(),
                        found: format!("{left} and {right}"),
                        span: span.into(),
                    });
                    IecType::Void
                }
            }
        }
    }

    /// Fold a compile-time integer expression, for the places the grammar allows an
    /// expression but the type system needs a number: array bounds, subrange bounds,
    /// string lengths.
    ///
    /// Matching only a bare `IntegerLiteral` here was a memory-safety bug, not just a
    /// missing feature: `ARRAY[-2..2]`'s lower bound is a `UnaryOp`, so it silently
    /// became `0`. The array was then sized `2 - 0 + 1 = 3` instead of 5 *and* its
    /// indices were normalised against `0` instead of `-2`, so `a[-2] := x` stored
    /// two elements in front of the object.
    pub fn const_int_expr(expr: &Expression) -> Option<i64> {
        match &expr.kind {
            ExpressionKind::IntegerLiteral(v) => i64::try_from(*v).ok(),
            ExpressionKind::Parenthesized(inner) => Self::const_int_expr(inner),
            ExpressionKind::TypedLiteral { value, .. } => Self::const_int_expr(value),
            ExpressionKind::UnaryOp { op, operand } => match op {
                UnaryOp::Neg => Self::const_int_expr(operand)?.checked_neg(),
                UnaryOp::Not => None,
            },
            ExpressionKind::BinaryOp { op, left, right } => {
                let l = Self::const_int_expr(left)?;
                let r = Self::const_int_expr(right)?;
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

    pub fn resolve_type_spec(&mut self, spec: &TypeSpec) -> IecType {
        match &spec.kind {
            TypeSpecKind::Named(ident) => self
                .types
                .resolve(&ident.name)
                .unwrap_or(IecType::Unresolved(ident.name.clone())),
            TypeSpecKind::StringType { wide, length } => {
                let max_len = length.as_ref().and_then(|e| {
                    if let ExpressionKind::IntegerLiteral(v) = &e.kind {
                        Some(*v as usize)
                    } else {
                        None
                    }
                });
                if *wide {
                    IecType::WstringType { max_len }
                } else {
                    IecType::StringType { max_len }
                }
            }
            TypeSpecKind::Array { ranges, base } => {
                let mut resolved_ranges = Vec::new();
                for r in ranges {
                    let low = Self::const_int_expr(&r.low).unwrap_or(0);
                    // An unfoldable upper bound must not collapse the dimension to a
                    // single element — that under-allocates. Fall back to the lower
                    // bound's own value so the extent is at least 1.
                    let high = Self::const_int_expr(&r.high).unwrap_or(low);
                    resolved_ranges.push((low, high.max(low)));
                }
                IecType::Array {
                    ranges: resolved_ranges,
                    element_type: Box::new(self.resolve_type_spec(base)),
                }
            }
            TypeSpecKind::Pointer(base) => IecType::Pointer(Box::new(self.resolve_type_spec(base))),
            TypeSpecKind::Subrange { base, low, high } => {
                let base_ty = self.types.resolve(&base.name).unwrap_or(IecType::Int);
                let lo = Self::const_int_expr(low).unwrap_or(0);
                let hi = Self::const_int_expr(high).unwrap_or(0);
                IecType::Subrange {
                    base_type: Box::new(base_ty),
                    low: lo,
                    high: hi,
                }
            }
            TypeSpecKind::Struct(fields) => {
                let resolved: Vec<_> = fields
                    .iter()
                    .map(|f| (f.name.name.clone(), self.resolve_type_spec(&f.type_spec)))
                    .collect();
                IecType::Struct {
                    name: String::new(),
                    fields: resolved,
                }
            }
            TypeSpecKind::Enum(spec) => {
                let mut values = Vec::new();
                for (i, v) in spec.values.iter().enumerate() {
                    let val = v
                        .value
                        .as_ref()
                        .and_then(|e| {
                            if let ExpressionKind::IntegerLiteral(n) = &e.kind {
                                Some(*n as i64)
                            } else {
                                None
                            }
                        })
                        .unwrap_or(i as i64);
                    values.push((v.name.name.clone(), val));
                }
                IecType::Enum {
                    name: String::new(),
                    base_type: Box::new(IecType::Int),
                    values,
                }
            }
            TypeSpecKind::Union(fields) => {
                let resolved: Vec<_> = fields
                    .iter()
                    .map(|f| (f.name.name.clone(), self.resolve_type_spec(&f.type_spec)))
                    .collect();
                IecType::Struct {
                    name: String::new(),
                    fields: resolved,
                }
            }
        }
    }
}

/// Convenience: check a compilation unit.
pub fn check(unit: &CompilationUnit) -> (SymbolTable, Vec<CheckError>) {
    TypeChecker::new().check(unit)
}
