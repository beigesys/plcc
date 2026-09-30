// SPDX-License-Identifier: MPL-2.0

//! Structured Text emitter: any [`CompilationUnit`] (parsed from `.st`, or
//! lowered from PLCopen XML, L5X or TwinCAT) printed back as canonical ST.
//!
//! **Canonical form.** Keywords upper case; identifiers as written; four-space
//! indentation; one statement per line, each ending in `;`; one declaration
//! per line in VAR blocks (`a, b : INT` is printed as two lines, which parses to
//! the same AST); a blank line between top-level declarations; spaces around
//! binary operators and `:=`; call arguments separated by `, `. Parentheses the
//! source wrote are kept (they are `Parenthesized` nodes), and the printer adds
//! the ones precedence needs, so a synthesized AST prints as ST that parses
//! back to the same tree.
//!
//! **Round trip.** `parse(print(ast))` equals `ast` except for spans,
//! redundant parentheses the printer had to add around a synthesized
//! sub-expression, and a negative literal written as `-5` (read back as
//! negation of `5`). What the AST does not hold cannot be printed: source
//! comments (the parser drops them) and pragmas other than the `{attribute}`
//! names of a TYPE. [`StatementKind::Comment`] statements (made by the ladder
//! lowerings) print as `(* ... *)` and are dropped when the text is parsed
//! again. A TwinCAT global variable list's name (`VarBlock::list_name`) has
//! no ST syntax; it is printed as a comment.

use crate::ast::*;
use std::fmt::Write;

const INDENT: &str = "    ";

/// Print a whole compilation unit.
pub fn print_unit(unit: &CompilationUnit) -> String {
    let mut p = Printer::default();
    for (i, d) in unit.declarations.iter().enumerate() {
        if i > 0 {
            p.out.push('\n');
        }
        p.declaration(d);
    }
    p.out
}

/// Print one top-level declaration.
pub fn print_declaration(d: &Declaration) -> String {
    let mut p = Printer::default();
    p.declaration(d);
    p.out
}

/// Print a statement list at the given indentation depth.
pub fn print_statements(stmts: &[Statement], depth: usize) -> String {
    let mut p = Printer {
        depth,
        ..Default::default()
    };
    p.stmts(stmts);
    p.out
}

/// Print one expression.
pub fn print_expression(e: &Expression) -> String {
    expr(e)
}

/// Print a type specification (single line for all but STRUCT/UNION).
pub fn print_type_spec(t: &TypeSpec) -> String {
    let mut p = Printer::default();
    p.type_spec(t);
    p.out
}

#[derive(Default)]
struct Printer {
    out: String,
    depth: usize,
}

impl Printer {
    fn line(&mut self, s: &str) {
        for _ in 0..self.depth {
            self.out.push_str(INDENT);
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn start(&mut self) {
        for _ in 0..self.depth {
            self.out.push_str(INDENT);
        }
    }

    fn indented(&mut self, f: impl FnOnce(&mut Self)) {
        self.depth += 1;
        f(self);
        self.depth -= 1;
    }

    // ── Declarations ──

    fn declaration(&mut self, d: &Declaration) {
        match d {
            Declaration::Program(p) => {
                self.line(&format!("PROGRAM {}", p.name.name));
                self.var_blocks(&p.var_blocks);
                self.members(&p.methods, &p.properties, &p.actions);
                self.body(&p.body);
                self.line("END_PROGRAM");
            }
            Declaration::Function(f) => {
                let mut head = format!("FUNCTION {}", f.name.name);
                if let Some(t) = &f.return_type {
                    head.push_str(" : ");
                    head.push_str(&self.inline_type(t));
                }
                self.line(&head);
                self.var_blocks(&f.var_blocks);
                self.body(&f.body);
                self.line("END_FUNCTION");
            }
            Declaration::FunctionBlock(fb) => {
                let mut head = format!("FUNCTION_BLOCK {}", fb.name.name);
                if let Some(e) = &fb.extends {
                    let _ = write!(head, " EXTENDS {}", e.name);
                }
                if !fb.implements.is_empty() {
                    let _ = write!(head, " IMPLEMENTS {}", names(&fb.implements));
                }
                self.line(&head);
                self.var_blocks(&fb.var_blocks);
                self.members(&fb.methods, &fb.properties, &fb.actions);
                self.body(&fb.body);
                self.line("END_FUNCTION_BLOCK");
            }
            Declaration::Class(c) => {
                let mut head = String::new();
                if c.is_abstract {
                    head.push_str("ABSTRACT ");
                } else if c.is_final {
                    head.push_str("FINAL ");
                }
                let _ = write!(head, "CLASS {}", c.name.name);
                if let Some(e) = &c.extends {
                    let _ = write!(head, " EXTENDS {}", e.name);
                }
                if !c.implements.is_empty() {
                    let _ = write!(head, " IMPLEMENTS {}", names(&c.implements));
                }
                self.line(&head);
                self.var_blocks(&c.var_blocks);
                self.members(&c.methods, &c.properties, &[]);
                self.line("END_CLASS");
            }
            Declaration::Interface(i) => {
                let mut head = format!("INTERFACE {}", i.name.name);
                if !i.extends.is_empty() {
                    let _ = write!(head, " EXTENDS {}", names(&i.extends));
                }
                self.line(&head);
                self.members(&i.methods, &i.properties, &[]);
                self.line("END_INTERFACE");
            }
            Declaration::TypeDecl(t) => self.type_decl(t),
            Declaration::GlobalVarDecl(b) => self.var_block(b),
            Declaration::Configuration(c) => self.configuration(c),
        }
    }

    fn body(&mut self, body: &[Statement]) {
        self.indented(|p| p.stmts(body));
    }

    fn members(&mut self, methods: &[MethodDecl], props: &[PropertyDecl], actions: &[ActionDecl]) {
        for m in methods {
            self.method(m);
        }
        for pr in props {
            self.property(pr);
        }
        for a in actions {
            self.line(&format!("ACTION {}:", a.name.name));
            self.body(&a.body);
            self.line("END_ACTION");
        }
    }

    fn method(&mut self, m: &MethodDecl) {
        let mut head = String::from("METHOD");
        if let Some(a) = m.access {
            head.push(' ');
            head.push_str(access(a));
        }
        if m.is_abstract {
            head.push_str(" ABSTRACT");
        }
        if m.is_final {
            head.push_str(" FINAL");
        }
        if m.is_override {
            head.push_str(" OVERRIDE");
        }
        let _ = write!(head, " {}", m.name.name);
        if let Some(t) = &m.return_type {
            head.push_str(" : ");
            head.push_str(&self.inline_type(t));
        }
        self.line(&head);
        self.var_blocks(&m.var_blocks);
        self.body(&m.body);
        self.line("END_METHOD");
    }

    fn property(&mut self, pr: &PropertyDecl) {
        let mut head = String::from("PROPERTY");
        if let Some(a) = pr.access {
            head.push(' ');
            head.push_str(access(a));
        }
        if pr.is_abstract {
            head.push_str(" ABSTRACT");
        }
        if pr.is_final {
            head.push_str(" FINAL");
        }
        let _ = write!(
            head,
            " {} : {}",
            pr.name.name,
            self.inline_type(&pr.type_spec)
        );
        self.line(&head);
        for (kw, acc) in [("GET", &pr.get), ("SET", &pr.set)] {
            if let Some(acc) = acc {
                self.line(kw);
                self.var_blocks(&acc.var_blocks);
                self.body(&acc.body);
                self.line(&format!("END_{kw}"));
            }
        }
        self.line("END_PROPERTY");
    }

    fn type_decl(&mut self, t: &TypeDeclaration) {
        for a in &t.attributes {
            self.line(&format!("{{attribute '{a}'}}"));
        }
        self.line("TYPE");
        self.indented(|p| {
            p.start();
            let _ = write!(p.out, "{}", t.name.name);
            if let Some(e) = &t.extends {
                let _ = write!(p.out, " EXTENDS {}", e.name);
            }
            p.out.push_str(" : ");
            p.type_spec(&t.type_spec);
            if let Some(init) = &t.initializer {
                p.out.push_str(" := ");
                p.out.push_str(&expr(init));
            }
            if !matches!(
                t.type_spec.kind,
                TypeSpecKind::Struct(_) | TypeSpecKind::Union(_)
            ) || t.initializer.is_some()
            {
                p.out.push(';');
            }
            p.out.push('\n');
        });
        self.line("END_TYPE");
    }

    fn configuration(&mut self, c: &ConfigurationDecl) {
        self.line(&format!("CONFIGURATION {}", c.name.name));
        self.indented(|p| {
            p.var_blocks(&c.global_vars);
            for r in &c.resources {
                let mut head = format!("RESOURCE {}", r.name.name);
                if let Some(on) = &r.on {
                    let _ = write!(head, " ON {}", on.name);
                }
                p.line(&head);
                p.indented(|p| {
                    p.var_blocks(&r.global_vars);
                    for t in &r.tasks {
                        if t.properties.is_empty() {
                            p.line(&format!("TASK {};", t.name.name));
                        } else {
                            let props: Vec<String> = t
                                .properties
                                .iter()
                                .map(|(k, v)| format!("{} := {}", k.name, expr(v)))
                                .collect();
                            p.line(&format!("TASK {}({});", t.name.name, props.join(", ")));
                        }
                    }
                    for pc in &r.program_configs {
                        let mut s = format!("PROGRAM {}", pc.name.name);
                        if let Some(t) = &pc.task {
                            let _ = write!(s, " WITH {}", t.name);
                        }
                        let _ = write!(s, " : {}", pc.program_type.name);
                        if !pc.connections.is_empty() {
                            let _ = write!(s, "({})", args(&pc.connections));
                        }
                        s.push(';');
                        p.line(&s);
                    }
                });
                p.line("END_RESOURCE");
            }
        });
        self.line("END_CONFIGURATION");
    }

    // ── Variables ──

    fn var_blocks(&mut self, blocks: &[VarBlock]) {
        for b in blocks {
            self.var_block(b);
        }
    }

    fn var_block(&mut self, b: &VarBlock) {
        if let Some(list) = &b.list_name {
            self.line(&format!("(* global variable list {} *)", list.name));
        }
        let mut head = String::from(match b.kind {
            VarBlockKind::Var => "VAR",
            VarBlockKind::VarInput => "VAR_INPUT",
            VarBlockKind::VarOutput => "VAR_OUTPUT",
            VarBlockKind::VarInOut => "VAR_IN_OUT",
            VarBlockKind::VarGlobal => "VAR_GLOBAL",
            VarBlockKind::VarExternal => "VAR_EXTERNAL",
            VarBlockKind::VarTemp => "VAR_TEMP",
            VarBlockKind::VarAccess => "VAR_ACCESS",
            VarBlockKind::VarConfig => "VAR_CONFIG",
            VarBlockKind::VarInst => "VAR_INST",
            VarBlockKind::VarStat => "VAR_STAT",
        });
        if b.is_constant {
            head.push_str(" CONSTANT");
        }
        if b.is_retain {
            head.push_str(" RETAIN");
        } else if b.is_non_retain {
            head.push_str(" NON_RETAIN");
        }
        self.line(&head);
        self.indented(|p| {
            for d in &b.declarations {
                p.var_decl(d);
            }
        });
        self.line("END_VAR");
    }

    fn var_decl(&mut self, d: &VarDecl) {
        self.start();
        self.out.push_str(&d.name.name);
        if let Some(at) = &d.at_address {
            let _ = write!(self.out, " AT {}", at.repr);
        }
        self.out.push_str(" : ");
        self.type_spec(&d.type_spec);
        if !d.init_args.is_empty() {
            let _ = write!(self.out, "({})", args(&d.init_args));
        }
        match d.edge {
            Some(EdgeKind::Rising) => self.out.push_str(" R_EDGE"),
            Some(EdgeKind::Falling) => self.out.push_str(" F_EDGE"),
            None => {}
        }
        if let Some(init) = &d.initializer {
            self.out.push_str(" := ");
            self.out.push_str(&expr(init));
        }
        self.out.push_str(";\n");
    }

    // ── Types ──

    fn inline_type(&self, t: &TypeSpec) -> String {
        let mut p = Printer {
            depth: self.depth,
            ..Default::default()
        };
        p.type_spec(t);
        p.out
    }

    /// Print a type spec at the current position. STRUCT and UNION span lines;
    /// their closing keyword is left at the current indentation, without `;`.
    fn type_spec(&mut self, t: &TypeSpec) {
        match &t.kind {
            TypeSpecKind::Named(i) => self.out.push_str(&i.name),
            TypeSpecKind::StringType { wide, length } => {
                self.out.push_str(if *wide { "WSTRING" } else { "STRING" });
                if let Some(l) = length {
                    let _ = write!(self.out, "[{}]", expr(l));
                }
            }
            TypeSpecKind::Array { ranges, base } => {
                let rs: Vec<String> = ranges
                    .iter()
                    .map(|r| format!("{}..{}", expr(&r.low), expr(&r.high)))
                    .collect();
                let _ = write!(self.out, "ARRAY[{}] OF ", rs.join(", "));
                self.type_spec(base);
            }
            TypeSpecKind::VarLengthArray { dimensions, base } => {
                let stars = vec!["*"; *dimensions].join(", ");
                let _ = write!(self.out, "ARRAY[{stars}] OF ");
                self.type_spec(base);
            }
            TypeSpecKind::Pointer(b) => {
                self.out.push_str("POINTER TO ");
                self.type_spec(b);
            }
            TypeSpecKind::Reference(b) => {
                self.out.push_str("REFERENCE TO ");
                self.type_spec(b);
            }
            TypeSpecKind::Subrange { base, low, high } => {
                let _ = write!(self.out, "{}({}..{})", base.name, expr(low), expr(high));
            }
            TypeSpecKind::Struct(fields) | TypeSpecKind::Union(fields) => {
                let (open, close) = if matches!(t.kind, TypeSpecKind::Struct(_)) {
                    ("STRUCT", "END_STRUCT")
                } else {
                    ("UNION", "END_UNION")
                };
                self.out.push('\n');
                self.line(open);
                self.indented(|p| {
                    for f in fields {
                        p.start();
                        let _ = write!(p.out, "{} : ", f.name.name);
                        p.type_spec(&f.type_spec);
                        if let Some(init) = &f.initializer {
                            p.out.push_str(" := ");
                            p.out.push_str(&expr(init));
                        }
                        p.out.push_str(";\n");
                    }
                });
                self.start();
                self.out.push_str(close);
            }
            TypeSpecKind::Enum(e) => {
                let vals: Vec<String> = e
                    .values
                    .iter()
                    .map(|v| match &v.value {
                        Some(x) => format!("{} := {}", v.name.name, expr(x)),
                        None => v.name.name.clone(),
                    })
                    .collect();
                let _ = write!(self.out, "({})", vals.join(", "));
                if let Some(b) = &e.base_type {
                    let _ = write!(self.out, " {}", b.name);
                }
            }
        }
    }

    // ── Statements ──

    fn stmts(&mut self, stmts: &[Statement]) {
        for s in stmts {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, s: &Statement) {
        match &s.kind {
            StatementKind::Assignment { target, value } => {
                if let Some(r) = ref_of(value) {
                    self.line(&format!("{} REF= {};", expr(target), expr(r)));
                } else {
                    self.line(&format!("{} := {};", expr(target), expr(value)));
                }
            }
            StatementKind::FunctionCall { callee, args: a } => {
                self.line(&format!("{}({});", postfix_operand(callee), args(a)));
            }
            StatementKind::If {
                condition,
                then_body,
                elsif_branches,
                else_body,
            } => {
                self.line(&format!("IF {} THEN", expr(condition)));
                self.body(then_body);
                for b in elsif_branches {
                    self.line(&format!("ELSIF {} THEN", expr(&b.condition)));
                    self.body(&b.body);
                }
                if let Some(e) = else_body {
                    self.line("ELSE");
                    self.body(e);
                }
                self.line("END_IF;");
            }
            StatementKind::Case {
                selector,
                branches,
                else_body,
            } => {
                self.line(&format!("CASE {} OF", expr(selector)));
                self.indented(|p| {
                    for b in branches {
                        let labels: Vec<String> = b
                            .labels
                            .iter()
                            .map(|l| match l {
                                CaseLabel::Value(v) => expr(v),
                                CaseLabel::Range(a, b) => format!("{}..{}", expr(a), expr(b)),
                            })
                            .collect();
                        p.line(&format!("{}:", labels.join(", ")));
                        p.body(&b.body);
                    }
                    if let Some(e) = else_body {
                        p.line("ELSE");
                        p.body(e);
                    }
                });
                self.line("END_CASE;");
            }
            StatementKind::For {
                variable,
                from,
                to,
                by,
                body,
            } => {
                let mut head = format!("FOR {} := {} TO {}", expr(variable), expr(from), expr(to));
                if let Some(b) = by {
                    let _ = write!(head, " BY {}", expr(b));
                }
                head.push_str(" DO");
                self.line(&head);
                self.body(body);
                self.line("END_FOR;");
            }
            StatementKind::While { condition, body } => {
                self.line(&format!("WHILE {} DO", expr(condition)));
                self.body(body);
                self.line("END_WHILE;");
            }
            StatementKind::Repeat { body, until } => {
                self.line("REPEAT");
                self.body(body);
                self.line(&format!("UNTIL {}", expr(until)));
                self.line("END_REPEAT;");
            }
            StatementKind::Exit => self.line("EXIT;"),
            StatementKind::Continue => self.line("CONTINUE;"),
            StatementKind::Return { value } => match value {
                Some(v) => self.line(&format!("RETURN {};", expr(v))),
                None => self.line("RETURN;"),
            },
            StatementKind::Empty => self.line(";"),
            StatementKind::Comment(text) => {
                let text = comment_text(text);
                let mut lines = text.lines();
                let first = lines.next().unwrap_or("");
                let rest: Vec<&str> = lines.collect();
                if rest.is_empty() {
                    self.line(&format!("(* {first} *)"));
                } else {
                    self.line(&format!("(* {first}"));
                    for l in rest {
                        self.line(&format!("   {l}"));
                    }
                    self.line("*)");
                }
            }
        }
    }
}

/// Comment text that cannot close (or open a nested) comment early.
fn comment_text(s: &str) -> String {
    s.replace("(*", "( *").replace("*)", "* )")
}

/// `__REF_OF(x)` (what the parser makes of `r REF= x`) → `x`.
fn ref_of(value: &Expression) -> Option<&Expression> {
    if let ExpressionKind::FunctionCall { callee, args } = &value.kind
        && let ExpressionKind::Identifier(id) = &callee.kind
        && id.name == "__REF_OF"
        && let [a] = args.as_slice()
        && a.name.is_none()
        && !a.is_output
    {
        return Some(&a.value);
    }
    None
}

fn access(a: AccessModifier) -> &'static str {
    match a {
        AccessModifier::Public => "PUBLIC",
        AccessModifier::Private => "PRIVATE",
        AccessModifier::Protected => "PROTECTED",
        AccessModifier::Internal => "INTERNAL",
    }
}

fn names(ids: &[Ident]) -> String {
    ids.iter()
        .map(|i| i.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn args(a: &[CallArg]) -> String {
    a.iter().map(arg).collect::<Vec<_>>().join(", ")
}

fn arg(a: &CallArg) -> String {
    match &a.name {
        Some(n) if a.is_output => {
            let not = if a.negated { "NOT " } else { "" };
            format!("{not}{} => {}", n.name, expr(&a.value))
        }
        Some(n) => format!("{} := {}", n.name, expr(&a.value)),
        None => expr(&a.value),
    }
}

// ── Expressions ──

/// Binding strength, as the parser reads it: OR < XOR < AND < comparisons <
/// + - < * / MOD < ** < unary < postfix/primary.
fn prec(e: &Expression) -> u8 {
    match &e.kind {
        ExpressionKind::BinaryOp { op, .. } => binop_prec(*op),
        ExpressionKind::UnaryOp { .. } => 8,
        // A negative literal prints with a leading `-`: it reads back as a
        // negation, so it binds like one.
        ExpressionKind::IntegerLiteral(v) if *v < 0 => 8,
        ExpressionKind::RealLiteral(v) if v.is_sign_negative() => 8,
        _ => 10,
    }
}

fn binop_prec(op: BinaryOp) -> u8 {
    match op {
        BinaryOp::Or | BinaryOp::OrElse => 1,
        BinaryOp::Xor => 2,
        BinaryOp::And | BinaryOp::AndThen => 3,
        BinaryOp::Equal
        | BinaryOp::NotEqual
        | BinaryOp::Less
        | BinaryOp::LessEqual
        | BinaryOp::Greater
        | BinaryOp::GreaterEqual => 4,
        BinaryOp::Add | BinaryOp::Sub => 5,
        BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => 6,
        BinaryOp::Power => 7,
    }
}

fn binop_str(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Mod => "MOD",
        BinaryOp::Power => "**",
        BinaryOp::And => "AND",
        BinaryOp::Or => "OR",
        BinaryOp::Xor => "XOR",
        BinaryOp::AndThen => "AND_THEN",
        BinaryOp::OrElse => "OR_ELSE",
        BinaryOp::Equal => "=",
        BinaryOp::NotEqual => "<>",
        BinaryOp::Less => "<",
        BinaryOp::LessEqual => "<=",
        BinaryOp::Greater => ">",
        BinaryOp::GreaterEqual => ">=",
    }
}

fn paren_if(e: &Expression, cond: bool) -> String {
    if cond {
        format!("({})", expr(e))
    } else {
        expr(e)
    }
}

/// An expression in postfix-object position (`x.m`, `x[i]`, `f(..)`, `p^`).
fn postfix_operand(e: &Expression) -> String {
    paren_if(e, prec(e) < 10)
}

fn real(v: f64) -> String {
    if !v.is_finite() {
        // No ST literal: a constant expression with that value.
        return if v.is_nan() {
            "(0.0 / 0.0)".into()
        } else if v > 0.0 {
            "(1.0 / 0.0)".into()
        } else {
            "(-1.0 / 0.0)".into()
        };
    }
    let s = format!("{v:?}");
    match s.split_once('e') {
        Some((m, e)) => {
            let m = if m.contains('.') {
                m.to_string()
            } else {
                format!("{m}.0")
            };
            format!("{m}E{e}")
        }
        None if s.contains('.') => s,
        None => format!("{s}.0"),
    }
}

fn expr(e: &Expression) -> String {
    match &e.kind {
        ExpressionKind::IntegerLiteral(v) => v.to_string(),
        ExpressionKind::RealLiteral(v) => real(*v),
        ExpressionKind::StringLiteral(s) => format!("'{s}'"),
        ExpressionKind::WstringLiteral(s) => format!("\"{s}\""),
        ExpressionKind::BoolLiteral(b) => (if *b { "TRUE" } else { "FALSE" }).into(),
        ExpressionKind::TimeLiteral(s)
        | ExpressionKind::DateLiteral(s)
        | ExpressionKind::TodLiteral(s)
        | ExpressionKind::DtLiteral(s)
        | ExpressionKind::DirectVariable(s) => s.clone(),
        ExpressionKind::TypedLiteral { type_name, value } => {
            format!("{}#{}", type_name.name, expr(value))
        }
        ExpressionKind::Identifier(i) => i.name.clone(),
        ExpressionKind::BinaryOp { op, left, right } => {
            let p = binop_prec(*op);
            // For the reader: an AND (or XOR) under an OR, and an AND under an
            // XOR, is parenthesized although precedence does not need it.
            let clarify = |e: &Expression| {
                let q = prec(e);
                (p == 1 || p == 2) && q > p && q <= 3
            };
            format!(
                "{} {} {}",
                paren_if(left, prec(left) < p || clarify(left)),
                binop_str(*op),
                paren_if(right, prec(right) <= p || clarify(right))
            )
        }
        ExpressionKind::UnaryOp { op, operand } => {
            let inner = paren_if(operand, prec(operand) < 8);
            match op {
                UnaryOp::Not => format!("NOT {inner}"),
                UnaryOp::Neg if inner.starts_with('-') => format!("- {inner}"),
                UnaryOp::Neg => format!("-{inner}"),
            }
        }
        ExpressionKind::FunctionCall { callee, args: a } => {
            format!("{}({})", postfix_operand(callee), args(a))
        }
        ExpressionKind::MemberAccess { object, member } => {
            format!("{}.{}", postfix_operand(object), member.name)
        }
        ExpressionKind::ArrayIndex { array, indices } => {
            let ix: Vec<String> = indices.iter().map(expr).collect();
            format!("{}[{}]", postfix_operand(array), ix.join(", "))
        }
        ExpressionKind::Dereference(inner) => format!("{}^", postfix_operand(inner)),
        ExpressionKind::Parenthesized(inner) => format!("({})", expr(inner)),
        ExpressionKind::ArrayInitializer(elems) => {
            let es: Vec<String> = elems
                .iter()
                .map(|el| match &el.repeat {
                    Some(n) => format!("{}({})", expr(n), expr(&el.value)),
                    None => expr(&el.value),
                })
                .collect();
            format!("[{}]", es.join(", "))
        }
        ExpressionKind::StructInitializer(fields) => {
            let fs: Vec<String> = fields
                .iter()
                .map(|f| format!("{} := {}", f.name.name, expr(&f.value)))
                .collect();
            format!("({})", fs.join(", "))
        }
    }
}

/// The AST as JSON with everything a print → parse round trip may change
/// removed: spans, `Parenthesized` wrappers, the sign of a negative literal
/// (`-5` reads back as `-(5)`), `Comment` statements and TwinCAT list names.
/// Two units that normalize equal print to the same text. Used by the
/// round-trip tests of every front end.
pub fn normalized<T: serde::Serialize>(value: &T) -> serde_json::Value {
    use serde_json::Value;
    fn walk(v: &mut Value) {
        match v {
            Value::Object(map) => {
                map.remove("span");
                if map.contains_key("list_name") {
                    map.insert("list_name".into(), Value::Null);
                }
                for (_, c) in map.iter_mut() {
                    walk(c);
                }
                // Expression { kind: Parenthesized(e) } → e
                if let Some(Value::Object(k)) = map.get("kind")
                    && let Some(inner) = k.get("Parenthesized")
                {
                    let inner = inner.clone();
                    *v = inner;
                    return;
                }
                // UnaryOp Neg of a literal → the negative literal.
                let mut lit = None;
                if let Some(Value::Object(k)) = map.get("kind")
                    && let Some(Value::Object(u)) = k.get("UnaryOp")
                    && u.get("op").and_then(Value::as_str) == Some("Neg")
                    && let Some(Value::Object(operand)) = u.get("operand")
                    && let Some(Value::Object(ok)) = operand.get("kind")
                {
                    if let Some(n) = ok.get("IntegerLiteral").and_then(Value::as_i64) {
                        lit = Some(("IntegerLiteral", Value::from(-n)));
                    } else if let Some(n) = ok.get("RealLiteral").and_then(Value::as_f64) {
                        lit = Some(("RealLiteral", Value::from(-n)));
                    }
                }
                if let Some((k, n)) = lit {
                    let mut kind = serde_json::Map::new();
                    kind.insert(k.into(), n);
                    map.insert("kind".into(), Value::Object(kind));
                }
            }
            Value::Array(items) => {
                items.retain(|i| {
                    !matches!(i.get("kind"), Some(Value::Object(k)) if k.contains_key("Comment"))
                });
                items.iter_mut().for_each(walk);
            }
            _ => {}
        }
    }
    let mut v = serde_json::to_value(value).unwrap_or(Value::Null);
    walk(&mut v);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    fn round(src: &str) -> String {
        let (u, errs) = parse(src);
        assert!(errs.is_empty(), "{errs:?}");
        print_unit(&u)
    }

    #[test]
    fn prints_canonical_program() {
        let out = round(
            "program p var a,b:int:=1; s:string[10]; END_VAR a:=(b+1)*2; if a>1 then b:=-a; elsif a=0 then ; else b:=a**2; end_if end_program",
        );
        assert_eq!(
            out,
            "PROGRAM p\nVAR\n    a : INT := 1;\n    b : INT := 1;\n    s : STRING[10];\nEND_VAR\n    a := (b + 1) * 2;\n    IF a > 1 THEN\n        b := -a;\n    ELSIF a = 0 THEN\n        ;\n    ELSE\n        b := a ** 2;\n    END_IF;\nEND_PROGRAM\n"
        );
        assert_eq!(round(&out), out, "printing is idempotent");
    }

    #[test]
    fn adds_parentheses_precedence_needs() {
        let sp = crate::Span::empty();
        let id = |n: &str| Expression {
            kind: ExpressionKind::Identifier(Ident::new(n, sp)),
            span: sp,
        };
        let bin = |op, l, r| Expression {
            kind: ExpressionKind::BinaryOp {
                op,
                left: Box::new(l),
                right: Box::new(r),
            },
            span: sp,
        };
        let e = bin(
            BinaryOp::And,
            bin(BinaryOp::Or, id("a"), id("b")),
            Expression {
                kind: ExpressionKind::UnaryOp {
                    op: UnaryOp::Not,
                    operand: Box::new(bin(
                        BinaryOp::Sub,
                        id("c"),
                        bin(BinaryOp::Sub, id("d"), id("e")),
                    )),
                },
                span: sp,
            },
        );
        assert_eq!(print_expression(&e), "(a OR b) AND NOT (c - (d - e))");
    }

    #[test]
    fn reals_always_lex_as_reals() {
        assert_eq!(real(1e20), "1.0E20");
        assert_eq!(real(2.5e-7), "2.5E-7");
        assert_eq!(real(100.0), "100.0");
        assert_eq!(real(0.1), "0.1");
    }
}
