// SPDX-License-Identifier: MPL-2.0

//! CODESYS / TwinCAT object-oriented sugar, lowered to constructs the type
//! checker and codegen already understand. Both run this pass first, on the
//! merged unit, so they see the same program.
//!
//! * **PROPERTY** `P : T` of an FB, CLASS or INTERFACE becomes the methods
//!   `__get_P : T` (from GET; inside it `P` is the return value) and
//!   `__set_P` with `VAR_INPUT P : T` (from SET). A read of `obj.P` (or of
//!   `P` / `THIS^.P` inside the POU) is the call `obj.__get_P()`; an
//!   assignment `obj.P := v` is the call `obj.__set_P(v)`. Which `.P` is a
//!   property is decided from the declared type of `obj`: variables, struct
//!   fields, array elements, pointers and references are followed, as are
//!   EXTENDS chains and interfaces.
//! * **ACTION** `A` of an FB becomes a method `A` without parameters, so
//!   `inst.A()` and `A()` inside the FB are ordinary method calls.
//! * **VAR_STAT** variables become VAR_GLOBALs named `__STAT_<POU>_<name>`
//!   (one variable shared by every instance, as in CODESYS).
//! * **Qualified names**: `GVL.x` for a global variable list `GVL` is `x`;
//!   a library namespace (`Tc2_Standard.TON`, `Lib.F(..)`) is dropped.
//! * **STRUCT EXTENDS**: a derived structure gets its base's fields first.

use plcc_st::Span;
use plcc_st::ast::*;
use std::collections::{HashMap, HashSet};

/// A problem found while lowering, with the index of the declaration it is in.
pub type DesugarError = (usize, Span, String);

#[derive(Clone)]
struct Prop {
    name: String,
    ty: TypeSpec,
    get: bool,
    set: bool,
}

#[derive(Default, Clone)]
struct Pou {
    /// Base POU (FB/CLASS EXTENDS) or base interfaces (INTERFACE EXTENDS).
    bases: Vec<String>,
    vars: HashMap<String, TypeSpec>,
    props: HashMap<String, Prop>,
    methods: HashMap<String, Option<TypeSpec>>,
}

#[derive(Default)]
struct Env {
    pous: HashMap<String, Pou>,
    structs: HashMap<String, Vec<StructField>>,
    aliases: HashMap<String, TypeSpec>,
    /// Type names that are enumerations (`E.Value` is not a namespace).
    enums: HashSet<String>,
    globals: HashMap<String, TypeSpec>,
    gvls: HashSet<String>,
    functions: HashMap<String, Option<TypeSpec>>,
}

fn up(s: &str) -> String {
    s.to_uppercase()
}

/// `Lib.Name` → `Name`: the last component of a qualified name.
fn unqualified(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

fn named(name: &str, span: Span) -> TypeSpec {
    TypeSpec {
        kind: TypeSpecKind::Named(Ident::new(name.to_string(), span)),
        span,
    }
}

fn ident_expr(name: &str, span: Span) -> Expression {
    Expression {
        kind: ExpressionKind::Identifier(Ident::new(name.to_string(), span)),
        span,
    }
}

fn this_expr(span: Span) -> Expression {
    Expression {
        kind: ExpressionKind::Dereference(Box::new(ident_expr("THIS", span))),
        span,
    }
}

fn member(object: Expression, name: &str, span: Span) -> Expression {
    Expression {
        kind: ExpressionKind::MemberAccess {
            object: Box::new(object),
            member: Ident::new(name.to_string(), span),
        },
        span,
    }
}

fn call(callee: Expression, args: Vec<CallArg>, span: Span) -> Expression {
    Expression {
        kind: ExpressionKind::FunctionCall {
            callee: Box::new(callee),
            args,
        },
        span,
    }
}

pub fn getter_name(prop: &str) -> String {
    format!("__get_{prop}")
}

pub fn setter_name(prop: &str) -> String {
    format!("__set_{prop}")
}

impl Env {
    fn build(unit: &CompilationUnit) -> Env {
        let mut env = Env::default();
        let vars_of = |blocks: &[VarBlock]| -> HashMap<String, TypeSpec> {
            blocks
                .iter()
                .filter(|b| b.kind != VarBlockKind::VarStat)
                .flat_map(|b| b.declarations.iter())
                .map(|d| (up(&d.name.name), d.type_spec.clone()))
                .collect()
        };
        let props_of = |props: &[PropertyDecl]| -> HashMap<String, Prop> {
            props
                .iter()
                .map(|p| {
                    (
                        up(&p.name.name),
                        Prop {
                            name: p.name.name.clone(),
                            ty: p.type_spec.clone(),
                            get: p.get.is_some(),
                            set: p.set.is_some(),
                        },
                    )
                })
                .collect()
        };
        let methods_of = |methods: &[MethodDecl]| -> HashMap<String, Option<TypeSpec>> {
            methods
                .iter()
                .map(|m| (up(&m.name.name), m.return_type.clone()))
                .collect()
        };
        for decl in &unit.declarations {
            match decl {
                Declaration::FunctionBlock(fb) => {
                    let mut methods = methods_of(&fb.methods);
                    for a in &fb.actions {
                        methods.insert(up(&a.name.name), None);
                    }
                    env.pous.insert(
                        up(&fb.name.name),
                        Pou {
                            bases: fb
                                .extends
                                .iter()
                                .map(|b| up(unqualified(&b.name)))
                                .collect(),
                            vars: vars_of(&fb.var_blocks),
                            props: props_of(&fb.properties),
                            methods,
                        },
                    );
                }
                Declaration::Class(c) => {
                    env.pous.insert(
                        up(&c.name.name),
                        Pou {
                            bases: c.extends.iter().map(|b| up(unqualified(&b.name))).collect(),
                            vars: vars_of(&c.var_blocks),
                            props: props_of(&c.properties),
                            methods: methods_of(&c.methods),
                        },
                    );
                }
                Declaration::Interface(i) => {
                    env.pous.insert(
                        up(&i.name.name),
                        Pou {
                            bases: i.extends.iter().map(|b| up(unqualified(&b.name))).collect(),
                            vars: HashMap::new(),
                            props: props_of(&i.properties),
                            methods: methods_of(&i.methods),
                        },
                    );
                }
                Declaration::Program(p) => {
                    let mut methods = methods_of(&p.methods);
                    for a in &p.actions {
                        methods.insert(up(&a.name.name), None);
                    }
                    env.pous.insert(
                        up(&p.name.name),
                        Pou {
                            vars: vars_of(&p.var_blocks),
                            props: props_of(&p.properties),
                            methods,
                            ..Pou::default()
                        },
                    );
                }
                Declaration::Function(f) => {
                    env.functions
                        .insert(up(&f.name.name), f.return_type.clone());
                }
                Declaration::TypeDecl(t) => {
                    let key = up(&t.name.name);
                    match &t.type_spec.kind {
                        TypeSpecKind::Struct(fields) | TypeSpecKind::Union(fields) => {
                            env.structs.insert(key, fields.clone());
                        }
                        TypeSpecKind::Enum(_) => {
                            env.enums.insert(key);
                        }
                        _ => {
                            env.aliases.insert(key, t.type_spec.clone());
                        }
                    }
                }
                Declaration::GlobalVarDecl(b) => {
                    if let Some(n) = &b.list_name {
                        env.gvls.insert(up(&n.name));
                    }
                    for d in &b.declarations {
                        env.globals.insert(up(&d.name.name), d.type_spec.clone());
                    }
                }
                Declaration::Configuration(c) => {
                    for b in c
                        .global_vars
                        .iter()
                        .chain(c.resources.iter().flat_map(|r| r.global_vars.iter()))
                    {
                        for d in &b.declarations {
                            env.globals.insert(up(&d.name.name), d.type_spec.clone());
                        }
                    }
                }
            }
        }
        env
    }

    /// The POU (FB, CLASS, INTERFACE, PROGRAM) a type names, through aliases
    /// and references.
    fn class_of(&self, ts: &TypeSpec) -> Option<String> {
        self.class_of_depth(ts, 0)
    }

    fn class_of_depth(&self, ts: &TypeSpec, depth: usize) -> Option<String> {
        if depth > 32 {
            return None;
        }
        match &ts.kind {
            TypeSpecKind::Named(id) => {
                let key = up(unqualified(&id.name));
                if self.pous.contains_key(&key) {
                    Some(key)
                } else {
                    let alias = self.aliases.get(&key)?;
                    self.class_of_depth(alias, depth + 1)
                }
            }
            TypeSpecKind::Reference(inner) => self.class_of_depth(inner, depth + 1),
            _ => None,
        }
    }

    /// Strip aliases and (auto-dereferenced) references.
    fn resolve<'a>(&'a self, ts: &'a TypeSpec) -> &'a TypeSpec {
        let mut t = ts;
        for _ in 0..32 {
            match &t.kind {
                TypeSpecKind::Named(id) => match self.aliases.get(&up(unqualified(&id.name))) {
                    Some(a) => t = a,
                    None => return t,
                },
                TypeSpecKind::Reference(inner) => t = inner,
                _ => return t,
            }
        }
        t
    }

    /// A property of `class` or of one of its bases.
    fn find_prop(&self, class: &str, name: &str) -> Option<Prop> {
        let mut seen = HashSet::new();
        let mut stack = vec![class.to_string()];
        while let Some(c) = stack.pop() {
            if !seen.insert(c.clone()) {
                continue;
            }
            let Some(pou) = self.pous.get(&c) else {
                continue;
            };
            if let Some(p) = pou.props.get(&up(name)) {
                return Some(p.clone());
            }
            stack.extend(pou.bases.iter().cloned());
        }
        None
    }

    /// A variable of `class` (own or inherited).
    fn find_var(&self, class: &str, name: &str) -> Option<TypeSpec> {
        let mut seen = HashSet::new();
        let mut c = Some(class.to_string());
        while let Some(cur) = c.take() {
            if !seen.insert(cur.clone()) {
                break;
            }
            let pou = self.pous.get(&cur)?;
            if let Some(t) = pou.vars.get(&up(name)) {
                return Some(t.clone());
            }
            c = pou.bases.first().cloned();
        }
        None
    }

    fn find_method(&self, class: &str, name: &str) -> Option<Option<TypeSpec>> {
        let mut seen = HashSet::new();
        let mut stack = vec![class.to_string()];
        while let Some(c) = stack.pop() {
            if !seen.insert(c.clone()) {
                continue;
            }
            let Some(pou) = self.pous.get(&c) else {
                continue;
            };
            if let Some(m) = pou.methods.get(&up(name)) {
                return Some(m.clone());
            }
            stack.extend(pou.bases.iter().cloned());
        }
        None
    }

    /// The type of `obj.name`.
    fn member_type(&self, ts: &TypeSpec, name: &str) -> Option<TypeSpec> {
        if let Some(class) = self.class_of(ts) {
            return self
                .find_var(&class, name)
                .or_else(|| self.find_prop(&class, name).map(|p| p.ty));
        }
        match &self.resolve(ts).kind {
            TypeSpecKind::Named(id) => {
                let fields = self.structs.get(&up(unqualified(&id.name)))?;
                fields
                    .iter()
                    .find(|f| f.name.name.eq_ignore_ascii_case(name))
                    .map(|f| f.type_spec.clone())
            }
            TypeSpecKind::Struct(fields) | TypeSpecKind::Union(fields) => fields
                .iter()
                .find(|f| f.name.name.eq_ignore_ascii_case(name))
                .map(|f| f.type_spec.clone()),
            _ => None,
        }
    }

    fn is_type_name(&self, name: &str) -> bool {
        let k = up(name);
        self.pous.contains_key(&k)
            || self.structs.contains_key(&k)
            || self.aliases.contains_key(&k)
            || self.enums.contains(&k)
            || self.functions.contains_key(&k)
    }
}

/// Library namespaces whose members plcc cannot provide by that name are
/// still stripped, so the missing-library diagnostic names the member.
fn looks_like_library_namespace(name: &str) -> bool {
    let u = up(name);
    u.starts_with("TC2_")
        || u.starts_with("TC3_")
        || u.starts_with("TCO")
        || u == "TCUNIT"
        || u == "__SYSTEM"
        || u == "STANDARD"
        || u == "SYSMEM"
}

/// Scope of the body being rewritten.
#[derive(Default, Clone)]
struct Ctx {
    /// Upper-case name of the POU whose body this is.
    pou: Option<String>,
    /// Method / accessor locals (upper-case name → type).
    locals: HashMap<String, TypeSpec>,
    /// Inside the GET of property `P`: `P` means the getter's return value.
    getter: Option<(String, String)>,
    /// VAR_STAT renames of this POU / method.
    stat: HashMap<String, String>,
}

struct Rewriter<'e> {
    env: &'e Env,
    ctx: Ctx,
    errors: Vec<(Span, String)>,
    changed: bool,
}

impl Rewriter<'_> {
    fn var_type(&self, name: &str) -> Option<TypeSpec> {
        let k = up(name);
        if let Some(t) = self.ctx.locals.get(&k) {
            return Some(t.clone());
        }
        if let Some(pou) = &self.ctx.pou
            && let Some(t) = self.env.find_var(pou, name)
        {
            return Some(t);
        }
        self.env.globals.get(&k).cloned()
    }

    fn is_var(&self, name: &str) -> bool {
        let k = up(name);
        self.ctx.locals.contains_key(&k)
            || self
                .ctx
                .pou
                .as_ref()
                .is_some_and(|p| self.env.find_var(p, name).is_some())
            || self.ctx.stat.contains_key(&k)
    }

    /// The declared type of an expression, as far as it can be followed.
    fn type_of(&self, e: &Expression) -> Option<TypeSpec> {
        match &e.kind {
            ExpressionKind::Identifier(id) => {
                let k = up(&id.name);
                if k == "THIS" {
                    let pou = self.ctx.pou.as_ref()?;
                    return Some(TypeSpec {
                        kind: TypeSpecKind::Pointer(Box::new(named(pou, e.span))),
                        span: e.span,
                    });
                }
                if k == "SUPER" {
                    let pou = self.ctx.pou.as_ref()?;
                    let base = self.env.pous.get(pou)?.bases.first()?;
                    return Some(TypeSpec {
                        kind: TypeSpecKind::Pointer(Box::new(named(base, e.span))),
                        span: e.span,
                    });
                }
                if let Some(t) = self.var_type(&id.name) {
                    return Some(t);
                }
                // A PROGRAM's name: `MAIN.fbX`.
                if self.env.pous.contains_key(&k) {
                    return Some(named(&id.name, e.span));
                }
                None
            }
            ExpressionKind::MemberAccess { object, member } => {
                let t = self.type_of(object)?;
                self.env.member_type(&t, &member.name)
            }
            ExpressionKind::ArrayIndex { array, .. } => {
                let t = self.type_of(array)?;
                match &self.env.resolve(&t).kind {
                    TypeSpecKind::Array { base, .. } => Some((**base).clone()),
                    // Indexing a pointer (`p[i]`) or a string: no class inside.
                    _ => None,
                }
            }
            ExpressionKind::Dereference(inner) => {
                let t = self.type_of(inner)?;
                match &self.env.resolve(&t).kind {
                    TypeSpecKind::Pointer(base) => Some((**base).clone()),
                    _ => None,
                }
            }
            ExpressionKind::Parenthesized(inner) => self.type_of(inner),
            ExpressionKind::FunctionCall { callee, .. } => match &callee.kind {
                ExpressionKind::Identifier(id) => {
                    if let Some(r) = self.env.functions.get(&up(&id.name)) {
                        return r.clone();
                    }
                    let pou = self.ctx.pou.as_ref()?;
                    self.env.find_method(pou, &id.name)?
                }
                ExpressionKind::MemberAccess { object, member } => {
                    let t = self.type_of(object)?;
                    let class = self.env.class_of(&t)?;
                    let name = member.name.strip_prefix("__get_").unwrap_or(&member.name);
                    if member.name.starts_with("__get_") {
                        return self.env.find_prop(&class, name).map(|p| p.ty);
                    }
                    self.env.find_method(&class, &member.name)?
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// `A.m` where `A` is a global variable list or a library namespace
    /// rather than a variable: the plain name `m`.
    fn namespace_member(&self, object: &Expression, member: &Ident) -> Option<Ident> {
        let ExpressionKind::Identifier(ns) = &object.kind else {
            return None;
        };
        if self.is_var(&ns.name) || self.env.globals.contains_key(&up(&ns.name)) {
            return None;
        }
        let k = up(&ns.name);
        if self.env.gvls.contains(&k) {
            return Some(member.clone());
        }
        if self.env.is_type_name(&ns.name) {
            return None; // `E.Value`, `PRG.x`, `FB.method`
        }
        // An unknown prefix is a namespace when what follows it is something
        // plcc knows, when plcc provides that library (Tc2_Standard's LEN), or
        // when the member is a known name of a missing library (reported by
        // that name). `Tc2_Xyz.Other` stays qualified: the missing library
        // `Tc2_Xyz` is reported.
        if self.env.is_type_name(&member.name)
            || self.env.globals.contains_key(&up(&member.name))
            || crate::libraries::provided(&ns.name)
            || (looks_like_library_namespace(&ns.name)
                && crate::libraries::library_of(&member.name).is_some())
        {
            return Some(member.clone());
        }
        None
    }

    /// The property `obj.name` refers to, if `obj`'s type has one.
    fn property_of(&self, object: &Expression, name: &str) -> Option<Prop> {
        let t = self.type_of(object)?;
        let class = self.env.class_of(&t)?;
        // A variable of that name wins (a property never shadows one).
        if self.env.find_var(&class, name).is_some() {
            return None;
        }
        self.env.find_prop(&class, name)
    }

    /// A bare name that is a property of the current POU.
    fn own_property(&self, name: &str) -> Option<Prop> {
        if self.is_var(name) {
            return None;
        }
        let pou = self.ctx.pou.as_ref()?;
        self.env.find_prop(pou, name)
    }

    fn rename(&mut self, id: &Ident) -> Option<Ident> {
        let k = up(&id.name);
        if let Some((p, getter)) = &self.ctx.getter
            && *p == k
            && !self.ctx.locals.contains_key(&k)
        {
            return Some(Ident::new(getter.clone(), id.span));
        }
        if let Some(new) = self.ctx.stat.get(&k)
            && !self.ctx.locals.contains_key(&k)
        {
            return Some(Ident::new(new.clone(), id.span));
        }
        None
    }

    fn get_call(&mut self, object: Expression, p: &Prop, span: Span) -> Expression {
        if !p.get {
            self.errors.push((
                span,
                format!(
                    "property `{}` has no GET accessor; it cannot be read",
                    p.name
                ),
            ));
        }
        self.changed = true;
        let value = call(
            member(object, &getter_name(&p.name), span),
            Vec::new(),
            span,
        );
        // A `REFERENCE TO T` property: GET returns the address; the property
        // means the referenced T.
        if matches!(p.ty.kind, TypeSpecKind::Reference(_)) {
            return Expression {
                kind: ExpressionKind::Dereference(Box::new(value)),
                span,
            };
        }
        value
    }

    /// Rewrite a value (read) expression.
    fn expr(&mut self, e: &Expression) -> Expression {
        let span = e.span;
        let kind = match &e.kind {
            ExpressionKind::Identifier(id) => {
                if let Some(new) = self.rename(id) {
                    self.changed = true;
                    ExpressionKind::Identifier(new)
                } else if let Some(p) = self.own_property(&id.name) {
                    return self.get_call(this_expr(span), &p, span);
                } else {
                    return e.clone();
                }
            }
            ExpressionKind::MemberAccess { object, member: m } => {
                if let Some(plain) = self.namespace_member(object, m) {
                    self.changed = true;
                    let id = Expression {
                        kind: ExpressionKind::Identifier(plain),
                        span,
                    };
                    return self.expr(&id);
                }
                if let Some(p) = self.property_of(object, &m.name) {
                    let obj = self.expr(object);
                    return self.get_call(obj, &p, span);
                }
                ExpressionKind::MemberAccess {
                    object: Box::new(self.expr(object)),
                    member: m.clone(),
                }
            }
            ExpressionKind::FunctionCall { callee, args } => {
                let mut callee = self.callee(callee);
                let args = self.args(args);
                // The overloaded `TO_STRING(x)` / `TO_WSTRING(x)` (IEC 61131-3
                // 3rd ed.) is `<type of x>_TO_STRING(x)` when x's declared type
                // is elementary.
                if let ExpressionKind::Identifier(id) = &callee.kind
                    && matches!(up(&id.name).as_str(), "TO_STRING" | "TO_WSTRING")
                    && let [arg] = args.as_slice()
                    && arg.name.is_none()
                    && let Some(t) = self.type_of(&arg.value)
                    && let TypeSpecKind::Named(tn) = &self.env.resolve(&t).kind
                    && crate::types::resolve_type_name(&tn.name)
                        .is_some_and(|ty| ty.is_any_elementary() && !ty.is_any_string())
                {
                    let name = format!("{}_{}", up(&tn.name), up(&id.name));
                    callee = ident_expr(&name, callee.span);
                    self.changed = true;
                }
                ExpressionKind::FunctionCall {
                    callee: Box::new(callee),
                    args,
                }
            }
            ExpressionKind::BinaryOp { op, left, right } => ExpressionKind::BinaryOp {
                op: *op,
                left: Box::new(self.expr(left)),
                right: Box::new(self.expr(right)),
            },
            ExpressionKind::UnaryOp { op, operand } => ExpressionKind::UnaryOp {
                op: *op,
                operand: Box::new(self.expr(operand)),
            },
            ExpressionKind::ArrayIndex { array, indices } => ExpressionKind::ArrayIndex {
                array: Box::new(self.expr(array)),
                indices: indices.iter().map(|i| self.expr(i)).collect(),
            },
            ExpressionKind::Dereference(inner) => {
                ExpressionKind::Dereference(Box::new(self.expr(inner)))
            }
            ExpressionKind::Parenthesized(inner) => {
                ExpressionKind::Parenthesized(Box::new(self.expr(inner)))
            }
            ExpressionKind::TypedLiteral { type_name, value } => ExpressionKind::TypedLiteral {
                type_name: Ident::new(unqualified(&type_name.name).to_string(), type_name.span),
                value: value.clone(),
            },
            ExpressionKind::ArrayInitializer(elems) => ExpressionKind::ArrayInitializer(
                elems
                    .iter()
                    .map(|el| ArrayInitElement {
                        repeat: el.repeat.clone(),
                        value: Box::new(self.expr(&el.value)),
                        span: el.span,
                    })
                    .collect(),
            ),
            ExpressionKind::StructInitializer(fields) => ExpressionKind::StructInitializer(
                fields
                    .iter()
                    .map(|f| StructInitField {
                        name: f.name.clone(),
                        value: self.expr(&f.value),
                        span: f.span,
                    })
                    .collect(),
            ),
            _ => return e.clone(),
        };
        Expression { kind, span }
    }

    /// Rewrite a callee: its object is a value, but its last name is a
    /// function, FB instance, method or action — never a property read.
    fn callee(&mut self, e: &Expression) -> Expression {
        match &e.kind {
            ExpressionKind::Identifier(id) => match self.rename(id) {
                Some(new) => {
                    self.changed = true;
                    Expression {
                        kind: ExpressionKind::Identifier(new),
                        span: e.span,
                    }
                }
                None => e.clone(),
            },
            ExpressionKind::MemberAccess { object, member: m } => {
                if let Some(plain) = self.namespace_member(object, m) {
                    self.changed = true;
                    return self.callee(&Expression {
                        kind: ExpressionKind::Identifier(plain),
                        span: e.span,
                    });
                }
                Expression {
                    kind: ExpressionKind::MemberAccess {
                        object: Box::new(self.expr(object)),
                        member: m.clone(),
                    },
                    span: e.span,
                }
            }
            _ => self.lvalue(e),
        }
    }

    /// Rewrite an assignable location: names are renamed and namespaces
    /// dropped, but the location itself is not turned into a GET call.
    fn lvalue(&mut self, e: &Expression) -> Expression {
        let span = e.span;
        let kind = match &e.kind {
            ExpressionKind::Identifier(id) => match self.rename(id) {
                Some(new) => {
                    self.changed = true;
                    ExpressionKind::Identifier(new)
                }
                None => return e.clone(),
            },
            ExpressionKind::MemberAccess { object, member: m } => {
                if let Some(plain) = self.namespace_member(object, m) {
                    self.changed = true;
                    return self.lvalue(&Expression {
                        kind: ExpressionKind::Identifier(plain),
                        span,
                    });
                }
                ExpressionKind::MemberAccess {
                    object: Box::new(self.lvalue_object(object)),
                    member: m.clone(),
                }
            }
            ExpressionKind::ArrayIndex { array, indices } => ExpressionKind::ArrayIndex {
                array: Box::new(self.lvalue_object(array)),
                indices: indices.iter().map(|i| self.expr(i)).collect(),
            },
            ExpressionKind::Dereference(inner) => {
                ExpressionKind::Dereference(Box::new(self.expr(inner)))
            }
            ExpressionKind::Parenthesized(inner) => {
                ExpressionKind::Parenthesized(Box::new(self.lvalue(inner)))
            }
            _ => return self.expr(e),
        };
        Expression { kind, span }
    }

    /// The object part of a location (`a` in `a.b := ..`): a property there
    /// is read (`fb.Prop.field := ..` reads the property), anything else is a
    /// location.
    fn lvalue_object(&mut self, e: &Expression) -> Expression {
        match &e.kind {
            ExpressionKind::Identifier(id) if self.rename(id).is_none() => {
                if self.own_property(&id.name).is_some() {
                    self.expr(e)
                } else {
                    e.clone()
                }
            }
            ExpressionKind::MemberAccess { object, member: m }
                if self.namespace_member(object, m).is_none()
                    && self.property_of(object, &m.name).is_some() =>
            {
                self.expr(e)
            }
            _ => self.lvalue(e),
        }
    }

    fn args(&mut self, args: &[CallArg]) -> Vec<CallArg> {
        args.iter()
            .map(|a| CallArg {
                name: a.name.clone(),
                value: if a.is_output {
                    self.lvalue(&a.value)
                } else {
                    self.expr(&a.value)
                },
                is_output: a.is_output,
                negated: a.negated,
                span: a.span,
            })
            .collect()
    }

    /// `target := value` where target is a property: the SET call.
    fn setter_call(
        &mut self,
        target: &Expression,
        value: Expression,
        span: Span,
    ) -> Option<Statement> {
        let (object, prop) = match &target.kind {
            ExpressionKind::Identifier(id) if self.rename(id).is_none() => {
                (this_expr(target.span), self.own_property(&id.name)?)
            }
            ExpressionKind::MemberAccess { object, member: m }
                if self.namespace_member(object, m).is_none() =>
            {
                let p = self.property_of(object, &m.name)?;
                (self.expr(object), p)
            }
            _ => return None,
        };
        // No SET, but GET returns a REFERENCE: the assignment writes through it
        // (`obj.Flag := TRUE` with `PROPERTY Flag : REFERENCE TO BOOL`).
        if !prop.set && prop.get && matches!(prop.ty.kind, TypeSpecKind::Reference(_)) {
            let target = self.get_call(object, &prop, target.span);
            return Some(Statement {
                kind: StatementKind::Assignment { target, value },
                span,
            });
        }
        if !prop.set {
            self.errors.push((
                target.span,
                format!(
                    "property `{}` has no SET accessor; it cannot be assigned",
                    prop.name
                ),
            ));
        }
        self.changed = true;
        let arg_span = value.span;
        Some(Statement {
            kind: StatementKind::FunctionCall {
                callee: member(object, &setter_name(&prop.name), target.span),
                args: vec![CallArg {
                    name: None,
                    value,
                    is_output: false,
                    negated: false,
                    span: arg_span,
                }],
            },
            span,
        })
    }

    fn stmts(&mut self, stmts: &[Statement]) -> Vec<Statement> {
        stmts.iter().map(|s| self.stmt(s)).collect()
    }

    fn stmt(&mut self, s: &Statement) -> Statement {
        let span = s.span;
        let kind = match &s.kind {
            StatementKind::Assignment { target, value } => {
                let value = self.expr(value);
                if let Some(set) = self.setter_call(target, value.clone(), span) {
                    return set;
                }
                StatementKind::Assignment {
                    target: self.lvalue(target),
                    value,
                }
            }
            StatementKind::FunctionCall { callee, args } => StatementKind::FunctionCall {
                callee: self.callee(callee),
                args: self.args(args),
            },
            StatementKind::If {
                condition,
                then_body,
                elsif_branches,
                else_body,
            } => StatementKind::If {
                condition: self.expr(condition),
                then_body: self.stmts(then_body),
                elsif_branches: elsif_branches
                    .iter()
                    .map(|b| ElsifBranch {
                        condition: self.expr(&b.condition),
                        body: self.stmts(&b.body),
                        span: b.span,
                    })
                    .collect(),
                else_body: else_body.as_ref().map(|b| self.stmts(b)),
            },
            StatementKind::Case {
                selector,
                branches,
                else_body,
            } => StatementKind::Case {
                selector: self.expr(selector),
                branches: branches
                    .iter()
                    .map(|b| CaseBranch {
                        labels: b
                            .labels
                            .iter()
                            .map(|l| match l {
                                CaseLabel::Value(v) => CaseLabel::Value(self.expr(v)),
                                CaseLabel::Range(a, b) => {
                                    CaseLabel::Range(self.expr(a), self.expr(b))
                                }
                            })
                            .collect(),
                        body: self.stmts(&b.body),
                        span: b.span,
                    })
                    .collect(),
                else_body: else_body.as_ref().map(|b| self.stmts(b)),
            },
            StatementKind::For {
                variable,
                from,
                to,
                by,
                body,
            } => StatementKind::For {
                variable: match self.rename(variable) {
                    Some(v) => {
                        self.changed = true;
                        v
                    }
                    None => variable.clone(),
                },
                from: self.expr(from),
                to: self.expr(to),
                by: by.as_ref().map(|b| self.expr(b)),
                body: self.stmts(body),
            },
            StatementKind::While { condition, body } => StatementKind::While {
                condition: self.expr(condition),
                body: self.stmts(body),
            },
            StatementKind::Repeat { body, until } => StatementKind::Repeat {
                body: self.stmts(body),
                until: self.expr(until),
            },
            StatementKind::Return { value } => StatementKind::Return {
                value: value.as_ref().map(|v| self.expr(v)),
            },
            other => other.clone(),
        };
        Statement { kind, span }
    }
}

/// Which qualified names lose their namespace: every one whose last name is
/// declared in the unit, or whose namespace plcc provides (Tc2_Standard), or
/// that names a known member of a missing library (so the diagnostic names the
/// member). Any other `Tc2_Xyz.Name` keeps its namespace, so the missing
/// library is reported by name.
struct Strip {
    changed: bool,
    declared: HashSet<String>,
}

impl Strip {
    fn strips(&self, qualified: &str) -> bool {
        let Some((ns, _)) = qualified.rsplit_once('.') else {
            return false;
        };
        let bare = unqualified(qualified);
        self.declared.contains(&up(bare))
            || crate::libraries::provided(ns)
            || crate::libraries::library_of(bare).is_some()
            || !looks_like_library_namespace(ns.split('.').next().unwrap_or(ns))
    }
}

/// Strip a namespace from a type name (`Tc2_Standard.TON` → `TON`).
fn strip_type(ts: &mut TypeSpec, st: &mut Strip) {
    match &mut ts.kind {
        TypeSpecKind::Named(id) => strip_ident(id, st),
        TypeSpecKind::Array { base, .. }
        | TypeSpecKind::Pointer(base)
        | TypeSpecKind::Reference(base) => strip_type(base, st),
        TypeSpecKind::Struct(fields) | TypeSpecKind::Union(fields) => {
            for f in fields {
                strip_type(&mut f.type_spec, st);
            }
        }
        _ => {}
    }
}

fn strip_ident(id: &mut Ident, st: &mut Strip) {
    if id.name.contains('.') && st.strips(&id.name) {
        id.name = unqualified(&id.name).to_string();
        st.changed = true;
    }
}

/// `__SYSTEM.IQueryInterface` is CODESYS's built-in root interface; plcc has
/// no such interface to implement or extend, and nothing is lost without it.
fn is_system_interface(id: &Ident) -> bool {
    let u = up(&id.name);
    u == "__SYSTEM.IQUERYINTERFACE" || u == "IQUERYINTERFACE"
}

fn locals_of(blocks: &[VarBlock]) -> HashMap<String, TypeSpec> {
    blocks
        .iter()
        .filter(|b| b.kind != VarBlockKind::VarStat)
        .flat_map(|b| b.declarations.iter())
        .map(|d| (up(&d.name.name), d.type_spec.clone()))
        .collect()
}

/// Lower the sugar in `unit`. `None` when there is nothing to lower (the unit
/// is used as it is). Errors carry the index of their declaration.
pub fn desugar(unit: &CompilationUnit) -> (Option<CompilationUnit>, Vec<DesugarError>) {
    let mut out = unit.clone();
    let mut changed = false;
    let mut errors: Vec<DesugarError> = Vec::new();

    // Namespaces in type names, EXTENDS and IMPLEMENTS, everywhere.
    let declared: HashSet<String> = out
        .declarations
        .iter()
        .filter_map(|d| match d {
            Declaration::Program(p) => Some(up(&p.name.name)),
            Declaration::Function(f) => Some(up(&f.name.name)),
            Declaration::FunctionBlock(f) => Some(up(&f.name.name)),
            Declaration::Class(c) => Some(up(&c.name.name)),
            Declaration::Interface(i) => Some(up(&i.name.name)),
            Declaration::TypeDecl(t) => Some(up(&t.name.name)),
            _ => None,
        })
        .collect();
    let mut st = Strip {
        changed: false,
        declared,
    };
    for decl in &mut out.declarations {
        strip_decl_types(decl, &mut st);
    }
    changed |= st.changed;

    // STRUCT EXTENDS: base fields first (bases resolved recursively).
    let structs: HashMap<String, (Option<Ident>, Vec<StructField>)> = out
        .declarations
        .iter()
        .filter_map(|d| match d {
            Declaration::TypeDecl(t) => match &t.type_spec.kind {
                TypeSpecKind::Struct(f) => Some((up(&t.name.name), (t.extends.clone(), f.clone()))),
                _ => None,
            },
            _ => None,
        })
        .collect();
    for (i, decl) in out.declarations.iter_mut().enumerate() {
        let Declaration::TypeDecl(t) = decl else {
            continue;
        };
        let Some(base) = t.extends.take() else {
            continue;
        };
        changed = true;
        let mut prefix = Vec::new();
        let mut cur = Some(base.clone());
        let mut seen = HashSet::new();
        while let Some(b) = cur.take() {
            let key = up(unqualified(&b.name));
            if !seen.insert(key.clone()) {
                errors.push((i, b.span, format!("`{}` extends itself", t.name.name)));
                break;
            }
            match structs.get(&key) {
                Some((next, fields)) => {
                    let mut f = fields.clone();
                    f.extend(prefix);
                    prefix = f;
                    cur = next.clone();
                }
                None => errors.push((
                    i,
                    b.span,
                    format!(
                        "`{}` EXTENDS `{}`, which is not a STRUCT in this program",
                        t.name.name, b.name
                    ),
                )),
            }
        }
        if let TypeSpecKind::Struct(fields) = &mut t.type_spec.kind {
            prefix.append(fields);
            *fields = prefix;
        } else {
            errors.push((
                i,
                base.span,
                format!("`{}` has EXTENDS but is not a STRUCT", t.name.name),
            ));
        }
    }

    let env = Env::build(&out);

    // VAR_STAT → VAR_GLOBAL `__STAT_<POU>[_<METHOD>]_<name>`.
    let mut stat_globals: Vec<VarDecl> = Vec::new();
    let mut stat_maps: HashMap<(usize, Option<String>), HashMap<String, String>> = HashMap::new();
    for (i, decl) in out.declarations.iter_mut().enumerate() {
        let (pou_name, blocks, methods): (&str, &mut Vec<VarBlock>, Option<&mut Vec<MethodDecl>>) =
            match decl {
                Declaration::FunctionBlock(fb) => {
                    (&fb.name.name, &mut fb.var_blocks, Some(&mut fb.methods))
                }
                Declaration::Class(c) => (&c.name.name, &mut c.var_blocks, Some(&mut c.methods)),
                Declaration::Function(f) => (&f.name.name, &mut f.var_blocks, None),
                Declaration::Program(p) => (&p.name.name, &mut p.var_blocks, Some(&mut p.methods)),
                _ => continue,
            };
        let pou_name = pou_name.to_string();
        let mut hoist = |blocks: &mut Vec<VarBlock>, prefix: String| -> HashMap<String, String> {
            let mut map = HashMap::new();
            blocks.retain(|b| {
                if b.kind != VarBlockKind::VarStat {
                    return true;
                }
                for d in &b.declarations {
                    let new = format!("{prefix}_{}", d.name.name);
                    map.insert(up(&d.name.name), new.clone());
                    let mut g = d.clone();
                    g.name = Ident::new(new, d.name.span);
                    stat_globals.push(g);
                }
                false
            });
            map
        };
        let own = hoist(blocks, format!("__STAT_{pou_name}"));
        if !own.is_empty() {
            changed = true;
            stat_maps.insert((i, None), own);
        }
        if let Some(methods) = methods {
            for m in methods.iter_mut() {
                let map = hoist(
                    &mut m.var_blocks,
                    format!("__STAT_{pou_name}_{}", m.name.name),
                );
                if !map.is_empty() {
                    changed = true;
                    stat_maps.insert((i, Some(up(&m.name.name))), map);
                }
            }
        }
    }

    // Bodies.
    for (i, decl) in out.declarations.iter_mut().enumerate() {
        let own_stat = stat_maps.get(&(i, None)).cloned().unwrap_or_default();
        let mut rw = Rewriter {
            env: &env,
            ctx: Ctx::default(),
            errors: Vec::new(),
            changed: false,
        };
        match decl {
            Declaration::Program(p) => {
                let pou = up(&p.name.name);
                rw.ctx.pou = Some(pou.clone());
                rw.ctx.stat = own_stat.clone();
                p.body = rw.stmts(&p.body);
                init_exprs(&mut rw, &mut p.var_blocks);
                lower_members(
                    &mut rw,
                    &pou,
                    &mut p.methods,
                    std::mem::take(&mut p.properties),
                    &own_stat,
                    &stat_maps,
                    i,
                    false,
                );
                lower_actions(&mut rw, &mut p.methods, std::mem::take(&mut p.actions), &own_stat);
            }
            Declaration::Function(f) => {
                rw.ctx.locals = locals_of(&f.var_blocks);
                rw.ctx.stat = own_stat;
                f.body = rw.stmts(&f.body);
                init_exprs(&mut rw, &mut f.var_blocks);
            }
            Declaration::FunctionBlock(fb) => {
                let pou = up(&fb.name.name);
                rw.ctx.pou = Some(pou.clone());
                rw.ctx.stat = own_stat.clone();
                fb.body = rw.stmts(&fb.body);
                init_exprs(&mut rw, &mut fb.var_blocks);
                lower_members(
                    &mut rw,
                    &pou,
                    &mut fb.methods,
                    std::mem::take(&mut fb.properties),
                    &own_stat,
                    &stat_maps,
                    i,
                    false,
                );
                lower_actions(&mut rw, &mut fb.methods, std::mem::take(&mut fb.actions), &own_stat);
                for v in &mut fb.var_blocks {
                    for d in &mut v.declarations {
                        let args = std::mem::take(&mut d.init_args);
                        d.init_args = rw.args(&args);
                    }
                }
            }
            Declaration::Class(c) => {
                let pou = up(&c.name.name);
                rw.ctx.pou = Some(pou.clone());
                init_exprs(&mut rw, &mut c.var_blocks);
                lower_members(
                    &mut rw,
                    &pou,
                    &mut c.methods,
                    std::mem::take(&mut c.properties),
                    &own_stat,
                    &stat_maps,
                    i,
                    false,
                );
            }
            Declaration::Interface(itf) => {
                let pou = up(&itf.name.name);
                lower_members(
                    &mut rw,
                    &pou,
                    &mut itf.methods,
                    std::mem::take(&mut itf.properties),
                    &own_stat,
                    &stat_maps,
                    i,
                    true,
                );
            }
            Declaration::GlobalVarDecl(b) => init_exprs(&mut rw, std::slice::from_mut(b)),
            Declaration::TypeDecl(_) | Declaration::Configuration(_) => {}
        }
        changed |= rw.changed;
        errors.extend(rw.errors.into_iter().map(|(s, m)| (i, s, m)));
    }

    if !stat_globals.is_empty() {
        let span = stat_globals[0].span;
        out.declarations.push(Declaration::GlobalVarDecl(VarBlock {
            list_name: None,
            kind: VarBlockKind::VarGlobal,
            is_constant: false,
            is_retain: false,
            is_non_retain: false,
            declarations: stat_globals,
            span,
        }));
    }

    (changed.then_some(out), errors)
}

/// Actions become parameterless methods.
fn lower_actions(
    rw: &mut Rewriter,
    methods: &mut Vec<MethodDecl>,
    actions: Vec<ActionDecl>,
    own_stat: &HashMap<String, String>,
) {
    for a in actions {
        rw.changed = true;
        rw.ctx.locals = HashMap::new();
        rw.ctx.getter = None;
        rw.ctx.stat = own_stat.clone();
        let body = rw.stmts(&a.body);
        methods.push(MethodDecl {
            name: a.name,
            access: None,
            is_override: false,
            is_abstract: false,
            is_final: false,
            return_type: None,
            var_blocks: Vec::new(),
            body,
            span: a.span,
        });
    }
}

fn init_exprs(rw: &mut Rewriter, blocks: &mut [VarBlock]) {
    for b in blocks {
        for d in &mut b.declarations {
            if let Some(init) = &d.initializer {
                d.initializer = Some(rw.expr(init));
            }
        }
    }
}

/// Methods' bodies, and the properties turned into GET/SET methods.
#[allow(clippy::too_many_arguments)]
fn lower_members(
    rw: &mut Rewriter,
    pou: &str,
    methods: &mut Vec<MethodDecl>,
    properties: Vec<PropertyDecl>,
    own_stat: &HashMap<String, String>,
    stat_maps: &HashMap<(usize, Option<String>), HashMap<String, String>>,
    index: usize,
    prototypes_only: bool,
) {
    rw.ctx.pou = Some(pou.to_string());
    for m in methods.iter_mut() {
        rw.ctx.locals = locals_of(&m.var_blocks);
        rw.ctx.getter = None;
        let mut stat = own_stat.clone();
        if let Some(ms) = stat_maps.get(&(index, Some(up(&m.name.name)))) {
            stat.extend(ms.clone());
        }
        rw.ctx.stat = stat;
        m.body = rw.stmts(&m.body);
    }
    for p in properties {
        rw.changed = true;
        // Names are case-insensitive: `PROPERTY Level` and `level : REAL` in
        // one POU are the same name (CODESYS: duplicate definition).
        if rw.env.find_var(pou, &p.name.name).is_some() {
            rw.errors.push((
                p.name.span,
                format!(
                    "property `{}` has the name of a variable of `{pou}` (names are not \
                     case-sensitive)",
                    p.name.name
                ),
            ));
        }
        let prop_ty = p.type_spec.clone();
        if let Some(get) = p.get {
            let name = getter_name(&p.name.name);
            rw.ctx.locals = locals_of(&get.var_blocks);
            rw.ctx.getter = Some((up(&p.name.name), name.clone()));
            rw.ctx.stat = own_stat.clone();
            let body = if prototypes_only {
                Vec::new()
            } else {
                rw.stmts(&get.body)
            };
            methods.push(MethodDecl {
                name: Ident::new(name, p.name.span),
                access: p.access,
                is_override: false,
                is_abstract: p.is_abstract,
                is_final: p.is_final,
                return_type: Some(prop_ty.clone()),
                var_blocks: get.var_blocks,
                body,
                span: get.span,
            });
        }
        if let Some(set) = p.set {
            let input = VarBlock {
                list_name: None,
                kind: VarBlockKind::VarInput,
                is_constant: false,
                is_retain: false,
                is_non_retain: false,
                declarations: vec![VarDecl {
                    name: p.name.clone(),
                    type_spec: prop_ty.clone(),
                    at_address: None,
                    edge: None,
                    initializer: None,
                    init_args: Vec::new(),
                    span: p.name.span,
                }],
                span: p.name.span,
            };
            let mut var_blocks = vec![input];
            var_blocks.extend(set.var_blocks);
            rw.ctx.locals = locals_of(&var_blocks);
            rw.ctx.getter = None;
            rw.ctx.stat = own_stat.clone();
            let body = if prototypes_only {
                Vec::new()
            } else {
                rw.stmts(&set.body)
            };
            methods.push(MethodDecl {
                name: Ident::new(setter_name(&p.name.name), p.name.span),
                access: p.access,
                is_override: false,
                is_abstract: p.is_abstract,
                is_final: p.is_final,
                return_type: None,
                var_blocks,
                body,
                span: set.span,
            });
        }
    }
    rw.ctx.getter = None;
    rw.ctx.locals = HashMap::new();
}

fn strip_decl_types(decl: &mut Declaration, st: &mut Strip) {
    let blocks: Vec<&mut VarBlock> = match decl {
        Declaration::Program(p) => p.var_blocks.iter_mut().collect(),
        Declaration::Function(f) => {
            if let Some(r) = &mut f.return_type {
                strip_type(r, st);
            }
            f.var_blocks.iter_mut().collect()
        }
        Declaration::FunctionBlock(fb) => {
            // `FUNCTION_BLOCK TcoContext EXTENDS TcoCore.TcoContext`: a library
            // base of the same name keeps its namespace (it is not itself).
            if let Some(e) = &mut fb.extends
                && !unqualified(&e.name).eq_ignore_ascii_case(&fb.name.name)
            {
                strip_ident(e, st);
            }
            let before = fb.implements.len();
            fb.implements.retain(|i| !is_system_interface(i));
            st.changed |= before != fb.implements.len();
            for i in &mut fb.implements {
                strip_ident(i, st);
            }
            for p in &mut fb.properties {
                strip_type(&mut p.type_spec, st);
                for acc in p.get.iter_mut().chain(p.set.iter_mut()) {
                    for b in &mut acc.var_blocks {
                        for d in &mut b.declarations {
                            strip_type(&mut d.type_spec, st);
                        }
                    }
                }
            }
            for m in &mut fb.methods {
                strip_method(m, st);
            }
            fb.var_blocks.iter_mut().collect()
        }
        Declaration::Class(c) => {
            if let Some(e) = &mut c.extends {
                strip_ident(e, st);
            }
            c.implements.retain(|i| !is_system_interface(i));
            for i in &mut c.implements {
                strip_ident(i, st);
            }
            for p in &mut c.properties {
                strip_type(&mut p.type_spec, st);
            }
            for m in &mut c.methods {
                strip_method(m, st);
            }
            c.var_blocks.iter_mut().collect()
        }
        Declaration::Interface(i) => {
            let before = i.extends.len();
            i.extends.retain(|e| !is_system_interface(e));
            st.changed |= before != i.extends.len();
            for e in &mut i.extends {
                strip_ident(e, st);
            }
            for p in &mut i.properties {
                strip_type(&mut p.type_spec, st);
            }
            for m in &mut i.methods {
                strip_method(m, st);
            }
            Vec::new()
        }
        Declaration::TypeDecl(t) => {
            strip_type(&mut t.type_spec, st);
            if let Some(e) = &mut t.extends {
                strip_ident(e, st);
            }
            Vec::new()
        }
        Declaration::GlobalVarDecl(b) => vec![b],
        Declaration::Configuration(_) => Vec::new(),
    };
    for b in blocks {
        for d in &mut b.declarations {
            strip_type(&mut d.type_spec, st);
        }
    }
}

fn strip_method(m: &mut MethodDecl, st: &mut Strip) {
    if let Some(r) = &mut m.return_type {
        strip_type(r, st);
    }
    for b in &mut m.var_blocks {
        for d in &mut b.declarations {
            strip_type(&mut d.type_spec, st);
        }
    }
}
