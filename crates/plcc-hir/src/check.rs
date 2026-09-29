// SPDX-License-Identifier: MPL-2.0
#![allow(unused_assignments, unused_variables)]

use crate::scope::{PouInfo, PouKind, Scope, SymbolTable, VarInfo};
use crate::types::{IecType, TypeRegistry, resolve_type_name};
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

    /// A name from a vendor library plcc does not implement (Beckhoff
    /// Tc2_System, Tc2_Utilities, Tc2_MC2, ...): see [`crate::libraries`].
    #[error("`{name}` is part of the {library} library, which plcc does not provide")]
    #[diagnostic(help(
        "plcc implements the IEC 61131-3 standard functions and function blocks (and the \
         Tc2_Standard timers, counters and edge detectors of the same names); vendor \
         libraries that wrap runtime services are not available"
    ))]
    MissingLibrary {
        name: String,
        library: String,
        #[label("from {library}")]
        span: miette::SourceSpan,
    },

    #[error("{message}")]
    General {
        message: String,
        #[label("{message}")]
        span: miette::SourceSpan,
    },

    /// A numeric conversion that may lose information or change sign, made
    /// implicitly: `i := r`, `b := i`, `r := lr`. IEC 61131-3 only converts
    /// implicitly when no information is lost; CODESYS compiles these and reports
    /// "Implicit conversion from 'X' to 'Y': possible loss of information" (C0197) /
    /// "possible change of sign" (C0195). plcc follows CODESYS: a warning, not an
    /// error.
    #[error("implicit conversion from {from} to {to}: possible loss of information or change of sign")]
    #[diagnostic(severity(Warning))]
    ImplicitConversion {
        from: String,
        to: String,
        #[label("implicitly converted to {to}")]
        span: miette::SourceSpan,
    },
}

impl CheckError {
    /// A diagnostic that does not stop compilation.
    pub fn is_warning(&self) -> bool {
        matches!(self, CheckError::ImplicitConversion { .. })
    }
}

/// An untyped numeric literal, or a constant expression made only of them
/// (`5`, `-1`, `2.0`, `(3 * 4)`). IEC 61131-3 gives such a literal no fixed type:
/// it takes the type its context needs, so `r := 2.0 * r` is REAL arithmetic and
/// `b := 5` stores into a BYTE. Typing literals eagerly as SINT/LREAL made the
/// checker reject exactly that everyday code.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Literal {
    Int(i128),
    Real,
}

fn untyped_literal(expr: &Expression) -> Option<Literal> {
    match &expr.kind {
        ExpressionKind::IntegerLiteral(v) => Some(Literal::Int(*v)),
        ExpressionKind::RealLiteral(_) => Some(Literal::Real),
        ExpressionKind::Parenthesized(inner) => untyped_literal(inner),
        ExpressionKind::UnaryOp {
            op: UnaryOp::Neg,
            operand,
        } => match untyped_literal(operand)? {
            Literal::Int(v) => Some(Literal::Int(-v)),
            Literal::Real => Some(Literal::Real),
        },
        ExpressionKind::BinaryOp { op, left, right } => {
            let (l, r) = (untyped_literal(left)?, untyped_literal(right)?);
            match (l, r) {
                // `**` is EXPT, whose result is always real.
                _ if *op == BinaryOp::Power => Some(Literal::Real),
                (Literal::Int(_), Literal::Int(_)) => {
                    // Fold what we can; an unfoldable one is still an integer.
                    let v = TypeChecker::const_int_expr(expr).unwrap_or(0);
                    matches!(
                        op,
                        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod
                    )
                    .then_some(Literal::Int(v as i128))
                }
                _ if matches!(
                    op,
                    BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div
                ) =>
                {
                    Some(Literal::Real)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// Can the untyped literal `lit` stand for a value of type `ty` without loss?
fn literal_fits(lit: Literal, ty: &IecType) -> bool {
    let ty = ty.base();
    match lit {
        Literal::Real => ty.is_any_real(),
        Literal::Int(v) => {
            if ty.is_any_real() {
                return true;
            }
            // BOOL accepts 0 and 1 — `q := 0;` is common CODESYS/OSCAT code.
            if *ty == IecType::Bool {
                return v == 0 || v == 1;
            }
            let Some(bits) = ty.bit_size() else {
                return false;
            };
            if ty.is_any_signed() {
                let max = (1i128 << (bits - 1)) - 1;
                (-max - 1..=max).contains(&v)
            } else if ty.is_any_unsigned() || ty.is_any_bit() {
                // A bit string also takes a negative literal of its width, as the
                // two's-complement pattern (`w := -1` is 16#FFFF), as CODESYS does.
                let max = (1i128 << bits) - 1;
                (-(1i128 << (bits - 1))..=max).contains(&v)
            } else {
                false
            }
        }
    }
}

/// Whether a date/time literal is written with an `L` (64-bit) prefix.
fn long_prefix(literal: &str) -> bool {
    literal.trim_start().starts_with(['L', 'l'])
}

/// Numeric in the loose sense CODESYS arithmetic uses: ANY_NUM plus the bit strings
/// wider than BOOL (`BYTE + 1`, `DWORD * 2` are accepted and computed unsigned).
fn is_arith(ty: &IecType) -> bool {
    let ty = ty.base();
    ty.is_any_num() || (ty.is_any_bit() && *ty != IecType::Bool)
}

pub struct TypeChecker {
    pub symbols: SymbolTable,
    pub types: TypeRegistry,
    pub errors: Vec<CheckError>,
    /// Uppercase names that are defined outside any POU's own variables: every
    /// VAR_GLOBAL (top level, CONFIGURATION, RESOURCE) and every enumerator. An
    /// identifier that is none of these, not a variable in scope, not a POU and
    /// not a type is undefined.
    known_names: std::collections::HashSet<String>,
    /// `FUNCTION_BLOCK X EXTENDS Y`: uppercase X → Y, so X's body sees Y's variables.
    fb_extends: std::collections::HashMap<String, String>,
}

impl TypeChecker {
    pub fn new() -> Self {
        Self {
            symbols: SymbolTable::new(),
            types: TypeRegistry::new(),
            errors: Vec::new(),
            known_names: std::collections::HashSet::new(),
            fb_extends: std::collections::HashMap::new(),
        }
    }

    pub fn check(self, unit: &CompilationUnit) -> (SymbolTable, Vec<CheckError>) {
        let (symbols, located) = self.check_located(unit);
        (symbols, located.into_iter().map(|(_, e)| e).collect())
    }

    /// Like [`Self::check`], but each diagnostic carries the index of the
    /// declaration in `unit.declarations` it was found in, so a caller that merged
    /// several files into one unit can say which file (and source) a span is in.
    pub fn check_located(
        mut self,
        unit: &CompilationUnit,
    ) -> (SymbolTable, Vec<(usize, CheckError)>) {
        // PROPERTY, ACTION, VAR_STAT, qualified names: see `crate::desugar`.
        let (desugared, sugar_errors) = crate::desugar::desugar(unit);
        let unit = desugared.as_ref().unwrap_or(unit);
        // Named constants in array bounds and string lengths, folded to literals.
        let (folded, unresolved) = crate::consts::fold_type_constants(unit);
        let unit = folded.as_ref().unwrap_or(unit);
        let mut located: Vec<(usize, CheckError)> = unresolved
            .into_iter()
            .map(|(i, span, what)| {
                (
                    i,
                    CheckError::General {
                        message: format!("{what} is not a constant integer expression"),
                        span: span.into(),
                    },
                )
            })
            .collect();
        located.extend(sugar_errors.into_iter().map(|(i, span, message)| {
            (
                i,
                CheckError::General {
                    message,
                    span: span.into(),
                },
            )
        }));

        // First pass: register all POUs and types
        for decl in &unit.declarations {
            self.register_declaration(decl);
        }

        // Second pass: type-check bodies
        for (i, decl) in unit.declarations.iter().enumerate() {
            self.check_library_types(decl);
            self.check_declaration(decl);
            located.extend(self.errors.drain(..).map(|e| (i, e)));
        }

        (self.symbols, located)
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
            Declaration::GlobalVarDecl(block) => self.register_globals(block),
            Declaration::Configuration(cfg) => {
                for block in &cfg.global_vars {
                    self.register_globals(block);
                }
                for res in &cfg.resources {
                    for block in &res.global_vars {
                        self.register_globals(block);
                    }
                }
            }
            _ => {}
        }
        if let Declaration::FunctionBlock(fb) = decl {
            if let Some(base) = &fb.extends {
                self.fb_extends
                    .insert(fb.name.name.to_uppercase(), base.name.clone());
            }
        }
    }

    fn register_globals(&mut self, block: &VarBlock) {
        for decl in &block.declarations {
            self.known_names.insert(decl.name.name.to_uppercase());
            // Resolving the type registers the enumerators of an inline enum.
            self.resolve_type_spec(&decl.type_spec);
        }
    }

    /// Whether `name` denotes something other than a variable in scope: a global,
    /// an enumerator, a POU, or a type (`Mode.Idle`, `SIZEOF(T)`).
    fn is_known_name(&self, name: &str) -> bool {
        let upper = name.to_uppercase();
        self.known_names.contains(&upper)
            || self.symbols.lookup_pou(name).is_some()
            || self.types.resolve(name).is_some()
            || resolve_type_name(name).is_some()
            || matches!(upper.as_str(), "THIS" | "SUPER")
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
                // Members inherited through EXTENDS, however deep.
                let mut base = self.fb_extends.get(&fb.name.name.to_uppercase()).cloned();
                let mut seen = 0;
                while let Some(b) = base.take() {
                    seen += 1;
                    if seen > 64 {
                        break;
                    }
                    if let Some(info) = self.symbols.lookup_pou(&b).cloned() {
                        for (name, ty) in info
                            .inputs
                            .iter()
                            .chain(&info.outputs)
                            .chain(&info.in_outs)
                            .chain(&info.locals)
                        {
                            if scope.lookup(name).is_none() {
                                scope.define(
                                    name.clone(),
                                    VarInfo {
                                        ty: ty.clone(),
                                        is_constant: false,
                                        is_input: false,
                                        is_output: false,
                                        is_in_out: false,
                                    },
                                );
                            }
                        }
                    }
                    base = self.fb_extends.get(&b.to_uppercase()).cloned();
                }
                self.check_statement_list(&fb.body, &mut scope);
            }
            _ => {}
        }
    }

    /// A call plcc cannot compile, reported here with its location: a function
    /// or FB of a vendor library plcc does not provide, or `__NEW` / `__DELETE`.
    /// Returns whether one was reported.
    fn unsupported_call(&mut self, callee: &Expression, scope: &Scope) -> bool {
        let ExpressionKind::Identifier(id) = &callee.kind else {
            return false;
        };
        let upper = id.name.to_uppercase();
        if matches!(upper.as_str(), "__NEW" | "__DELETE") {
            self.errors.push(CheckError::General {
                message: format!(
                    "`{}` (dynamic memory) is not supported: plcc allocates every \
                     variable statically",
                    id.name
                ),
                span: callee.span.into(),
            });
            return true;
        }
        if scope.lookup(&id.name).is_some() || self.symbols.lookup_pou(&id.name).is_some() {
            return false;
        }
        match crate::libraries::library_of(&id.name) {
            Some(library) => {
                self.errors.push(CheckError::MissingLibrary {
                    name: id.name.clone(),
                    library,
                    span: callee.span.into(),
                });
                true
            }
            None => false,
        }
    }

    /// Declared types from a vendor library plcc does not provide
    /// (`fbTime : FB_LocalSystemTime;`), in every variable of `decl`.
    fn check_library_types(&mut self, decl: &Declaration) {
        let mut blocks: Vec<&VarBlock> = Vec::new();
        match decl {
            Declaration::Program(p) => blocks.extend(&p.var_blocks),
            Declaration::Function(f) => blocks.extend(&f.var_blocks),
            Declaration::FunctionBlock(fb) => {
                blocks.extend(&fb.var_blocks);
                for m in &fb.methods {
                    blocks.extend(&m.var_blocks);
                }
            }
            Declaration::Class(c) => {
                blocks.extend(&c.var_blocks);
                for m in &c.methods {
                    blocks.extend(&m.var_blocks);
                }
            }
            Declaration::GlobalVarDecl(b) => blocks.push(b),
            Declaration::TypeDecl(t) => {
                if let TypeSpecKind::Struct(fields) = &t.type_spec.kind {
                    for f in fields {
                        self.check_library_type(&f.type_spec);
                    }
                }
            }
            _ => {}
        }
        for b in blocks {
            for d in &b.declarations {
                self.check_library_type(&d.type_spec);
            }
        }
    }

    fn check_library_type(&mut self, ts: &TypeSpec) {
        match &ts.kind {
            TypeSpecKind::Named(id) => {
                if resolve_type_name(&id.name).is_some()
                    || self.types.resolve(&id.name).is_some()
                    || self.symbols.lookup_pou(&id.name).is_some()
                {
                    return;
                }
                if let Some(library) = crate::libraries::library_of(&id.name) {
                    self.errors.push(CheckError::MissingLibrary {
                        name: id.name.clone(),
                        library,
                        span: id.span.into(),
                    });
                }
            }
            TypeSpecKind::Array { base, .. }
            | TypeSpecKind::Pointer(base)
            | TypeSpecKind::Reference(base) => self.check_library_type(base),
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

                self.check_assignable(&target_ty, &value_ty, value);
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
                // Variable should be an integer; CODESYS also accepts a bit string
                // (`FOR b := 0 TO 7` with `b : BYTE`).
                if let Some(info) = scope.lookup(&variable.name) {
                    if !(info.ty.base().is_any_int()
                        || (info.ty.base().is_any_bit() && *info.ty.base() != IecType::Bool)
                        || info.ty.base().is_any_real()
                        || Self::unknown(&info.ty))
                    {
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
                // A bare callee is a FUNCTION, an FB instance or a builtin (PRINT,
                // MEMCPY, ...); an unknown one is reported by codegen as an unknown
                // function, so only a callee expression (`a.b(..)`, `arr[i](..)`) is
                // checked here.
                if self.unsupported_call(callee, scope) {
                    return;
                }
                if !matches!(callee.kind, ExpressionKind::Identifier(_)) {
                    self.check_expression(callee, scope);
                }
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

    /// Can a value of `value_ty` (the expression `value`) be stored into `target_ty`?
    /// Exact and lossless-implicit conversions pass; an untyped literal passes when it
    /// fits; any other numeric conversion is a warning (as in CODESYS); everything
    /// else is a type mismatch.
    fn check_assignable(&mut self, target_ty: &IecType, value_ty: &IecType, value: &Expression) {
        if Self::unknown(target_ty) || Self::unknown(value_ty) {
            return;
        }
        let (t, v) = (target_ty.base(), value_ty.base());
        if t == v || v.can_implicit_convert_to(t) {
            return;
        }
        if untyped_literal(value).is_some_and(|lit| literal_fits(lit, t)) {
            return;
        }
        // An integer literal stored into a TIME (`et := 0`): not IEC, and not
        // confirmed for CODESYS either, so it is flagged but does not stop the build.
        if t.is_any_duration() && matches!(untyped_literal(value), Some(Literal::Int(_))) {
            self.errors.push(CheckError::ImplicitConversion {
                from: "an integer literal".into(),
                to: target_ty.to_string(),
                span: value.span.into(),
            });
            return;
        }
        // Numbers, bit strings and addresses convert into one another in CODESYS,
        // with a warning when information may be lost.
        let numeric = |ty: &IecType| is_arith(ty) || matches!(ty, IecType::Pointer(_));
        if numeric(t) && numeric(v) {
            if !matches!((t, v), (IecType::Pointer(_), _) | (_, IecType::Pointer(_))) {
                self.errors.push(CheckError::ImplicitConversion {
                    from: value_ty.to_string(),
                    to: target_ty.to_string(),
                    span: value.span.into(),
                });
            }
            return;
        }
        // STRING of one length into STRING of another: truncating copy, allowed.
        if (t.is_any_string() && v.is_any_string())
            && matches!(
                (t, v),
                (IecType::StringType { .. }, IecType::StringType { .. })
                    | (IecType::WstringType { .. }, IecType::WstringType { .. })
            )
        {
            return;
        }
        self.errors.push(CheckError::TypeMismatch {
            expected: target_ty.to_string(),
            found: value_ty.to_string(),
            span: value.span.into(),
        });
    }

    /// A type the checker does not model yet (member access, unknown call results,
    /// enums, user types it could not resolve): no judgement is made on it.
    fn unknown(ty: &IecType) -> bool {
        matches!(
            ty,
            IecType::Void
                | IecType::Unresolved(_)
                | IecType::Enum { .. }
                | IecType::FbInstance(_)
        )
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
            // The `L` prefixes (`LTIME#`, `LT#`, `LDATE#`, `LD#`, `LTOD#`,
            // `LTIME_OF_DAY#`, `LDT#`, `LDATE_AND_TIME#`) are the 64-bit types.
            ExpressionKind::TimeLiteral(s) => {
                if long_prefix(s) { IecType::Ltime } else { IecType::Time }
            }
            ExpressionKind::DateLiteral(s) => {
                if long_prefix(s) { IecType::Ldate } else { IecType::Date }
            }
            ExpressionKind::TodLiteral(s) => {
                if long_prefix(s) { IecType::Ltod } else { IecType::Tod }
            }
            ExpressionKind::DtLiteral(s) => {
                if long_prefix(s) { IecType::Ldt } else { IecType::Dt }
            }
            ExpressionKind::DirectVariable(_) => IecType::Void, // Needs context
            ExpressionKind::Identifier(ident) => {
                if let Some(info) = scope.lookup(&ident.name) {
                    info.ty.clone()
                } else if self.is_known_name(&ident.name) {
                    // A global, an enumerator, a POU or a type name. Their types are
                    // not tracked here yet.
                    IecType::Void
                } else if let Some(library) = crate::libraries::library_of(&ident.name) {
                    self.errors.push(CheckError::MissingLibrary {
                        name: ident.name.clone(),
                        library,
                        span: expr.span.into(),
                    });
                    IecType::Void
                } else {
                    // CODESYS: "Identifier '<name>' not defined" (an error). Codegen
                    // would otherwise be the first to notice, with no location.
                    self.errors.push(CheckError::UndefinedVariable {
                        name: ident.name.clone(),
                        span: expr.span.into(),
                    });
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
                // An untyped literal operand takes the other operand's type:
                // `2.0 * r` is REAL, `b AND 16#0F` is BYTE.
                let left_lit = untyped_literal(left);
                let right_lit = untyped_literal(right);
                let left_ty = match (left_lit, right_lit) {
                    (Some(l), None) if literal_fits(l, &right_ty) => right_ty.clone(),
                    _ => left_ty,
                };
                let right_ty = match (left_lit, right_lit) {
                    (None, Some(r)) if literal_fits(r, &left_ty) => left_ty.clone(),
                    _ => right_ty,
                };
                self.check_binary_op(*op, &left_ty, &right_ty, expr.span)
            }
            ExpressionKind::UnaryOp { op, operand } => {
                let ty = self.check_expression(operand, scope);
                match op {
                    UnaryOp::Not => {
                        // NOT is bitwise on the integers too (CODESYS; codegen agrees).
                        if ty.is_any_bit() || ty.is_any_int() || Self::unknown(&ty) {
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
                        // Negating a bit string is accepted by CODESYS (OSCAT's
                        // COUNT_BR writes `-step` for a BYTE step).
                        if is_arith(&ty) || ty.is_any_duration() || Self::unknown(&ty) {
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
                if self.unsupported_call(callee, scope) {
                    return IecType::Void;
                }
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
            ExpressionKind::StructInitializer(fields) => {
                for f in fields {
                    self.check_expression(&f.value, scope);
                }
                IecType::Void
            }
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
        // Skip check if either side is unknown
        if Self::unknown(left) || Self::unknown(right) {
            return IecType::Void;
        }
        let (left, right) = (left.base(), right.base());

        match op {
            // CODESYS pointer arithmetic: address ± integer, integer + address, and
            // the byte distance between two addresses.
            BinaryOp::Add | BinaryOp::Sub
                if matches!(left, IecType::Pointer(_)) && is_arith(right) =>
            {
                left.clone()
            }
            BinaryOp::Add if is_arith(left) && matches!(right, IecType::Pointer(_)) => {
                right.clone()
            }
            BinaryOp::Sub
                if matches!(left, IecType::Pointer(_)) && matches!(right, IecType::Pointer(_)) =>
            {
                IecType::Dint
            }
            // TIME/DATE arithmetic: TIME ± TIME, TIME * / number, DATE-family ± TIME,
            // DATE-family - DATE-family.
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div
                if left.is_any_duration() || left.is_any_date() || right.is_any_duration() =>
            {
                if (left.is_any_duration() || left.is_any_date())
                    && (right.is_any_duration() || right.is_any_date() || is_arith(right))
                {
                    if left.is_any_date() && right.is_any_date() {
                        IecType::Time
                    } else {
                        left.clone()
                    }
                } else if is_arith(left) && right.is_any_duration() {
                    right.clone()
                } else {
                    self.errors.push(CheckError::TypeMismatch {
                        expected: "ANY_MAGNITUDE".into(),
                        found: format!("{left} and {right}"),
                        span: span.into(),
                    });
                    IecType::Void
                }
            }
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => {
                if is_arith(left) && is_arith(right) {
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
            // `**` is EXPT (IEC 61131-3 Table 23/29): IN1 ANY_REAL, IN2 ANY_NUM, result
            // IN1's type. An integer base is implicitly converted — 8/16-bit to REAL,
            // wider to LREAL (Table 11) — so the result is never an integer. This
            // matches codegen's `compile_expt`.
            BinaryOp::Power => {
                if !(is_arith(left) && is_arith(right)) {
                    self.errors.push(CheckError::TypeMismatch {
                        expected: "ANY_NUM ** ANY_NUM".into(),
                        found: format!("{left} and {right}"),
                        span: span.into(),
                    });
                    return IecType::Void;
                }
                let base = if left.is_any_real() {
                    left.clone()
                } else if left.bit_size().is_some_and(|b| b <= 16) {
                    IecType::Real
                } else {
                    IecType::Lreal
                };
                if base == IecType::Lreal || *right == IecType::Lreal {
                    IecType::Lreal
                } else {
                    IecType::Real
                }
            }
            BinaryOp::Equal
            | BinaryOp::NotEqual
            | BinaryOp::Less
            | BinaryOp::LessEqual
            | BinaryOp::Greater
            | BinaryOp::GreaterEqual => IecType::Bool,
            BinaryOp::And | BinaryOp::Or | BinaryOp::Xor => {
                // ANY_BIT, and the integers too — CODESYS accepts `i AND 16#FF`.
                let bitwise = |t: &IecType| t.is_any_bit() || t.is_any_int();
                let bool_mix = (*left == IecType::Bool) != (*right == IecType::Bool);
                if bitwise(left) && bitwise(right) && !bool_mix {
                    if *left == IecType::Bool && *right == IecType::Bool {
                        IecType::Bool
                    } else if left.bit_size() >= right.bit_size() {
                        left.clone()
                    } else {
                        right.clone()
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
            // CODESYS short-circuit operators take BOOL operands only.
            BinaryOp::AndThen | BinaryOp::OrElse => {
                if *left != IecType::Bool || *right != IecType::Bool {
                    self.errors.push(CheckError::TypeMismatch {
                        expected: "BOOL".into(),
                        found: format!("{left} and {right}"),
                        span: span.into(),
                    });
                }
                IecType::Bool
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
            // A REFERENCE is used as the value it refers to, so that is its type
            // in every expression. (Codegen stores it as an address; see
            // `plcc_codegen`'s reference desugaring.)
            TypeSpecKind::Reference(base) => self.resolve_type_spec(base),
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
                // An enumerator without a value is the previous one plus one (the
                // first is 0), per IEC 61131-3 §6.4.4.3 and CODESYS:
                // `(Idle, Running := 5, Stopped)` makes Stopped 6. Numbering by
                // position instead made it 2.
                let mut values: Vec<(String, i64)> = Vec::new();
                let mut next = 0i64;
                for v in &spec.values {
                    let val = v
                        .value
                        .as_ref()
                        .and_then(|e| Self::enum_const(e, &values))
                        .unwrap_or(next);
                    values.push((v.name.name.clone(), val));
                    self.known_names.insert(v.name.name.to_uppercase());
                    next = val.wrapping_add(1);
                }
                // `TYPE Color : DINT (Red, Green);` — the base type, INT by default
                // (as in CODESYS).
                let base_type = spec
                    .base_type
                    .as_ref()
                    .and_then(|b| resolve_type_name(&b.name))
                    .filter(|t| t.is_any_int() || t.is_any_bit())
                    .unwrap_or(IecType::Int);
                IecType::Enum {
                    name: String::new(),
                    base_type: Box::new(base_type),
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

impl TypeChecker {
    /// The value of an enumerator's initializer: an integer literal, possibly
    /// negated, parenthesized or typed (`INT#5`, `16#FF`), or an earlier
    /// enumerator of the same type.
    fn enum_const(e: &Expression, earlier: &[(String, i64)]) -> Option<i64> {
        match &e.kind {
            ExpressionKind::IntegerLiteral(n) => i64::try_from(*n).ok(),
            ExpressionKind::Parenthesized(inner) => Self::enum_const(inner, earlier),
            ExpressionKind::TypedLiteral { value, .. } => Self::enum_const(value, earlier),
            ExpressionKind::UnaryOp {
                op: UnaryOp::Neg,
                operand,
            } => Self::enum_const(operand, earlier).map(i64::wrapping_neg),
            ExpressionKind::Identifier(id) => earlier
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(&id.name))
                .map(|(_, v)| *v),
            _ => None,
        }
    }
}

/// Convenience: check a compilation unit.
pub fn check(unit: &CompilationUnit) -> (SymbolTable, Vec<CheckError>) {
    TypeChecker::new().check(unit)
}
