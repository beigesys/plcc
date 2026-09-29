// SPDX-License-Identifier: MPL-2.0

//! `<project>` → declarations: data types, POUs and their interfaces,
//! configurations. Bodies are dispatched to [`crate::embed`] (ST) and
//! [`crate::graph`] (LD / FBD).

use crate::embed::{self, Fragment};
use crate::error::PlcOpenError;
use crate::graph;
use crate::xml::{self, XNode};
use plcc_st::Span;
use plcc_st::ast::*;

pub(crate) struct Lower<'s> {
    pub src: &'s str,
    pub errors: Vec<PlcOpenError>,
}

/// Which kind of POU a body belongs to (graph lowering needs to know whether
/// hidden state is allowed).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PouKind {
    Program,
    FunctionBlock,
    Function,
}

const ELEMENTARY: &[&str] = &[
    "BOOL",
    "BYTE",
    "WORD",
    "DWORD",
    "LWORD",
    "SINT",
    "INT",
    "DINT",
    "LINT",
    "USINT",
    "UINT",
    "UDINT",
    "ULINT",
    "REAL",
    "LREAL",
    "TIME",
    "LTIME",
    "DATE",
    "LDATE",
    "TIME_OF_DAY",
    "TOD",
    "LTOD",
    "DATE_AND_TIME",
    "DT",
    "LDT",
    "CHAR",
    "WCHAR",
];

impl<'s> Lower<'s> {
    pub fn new(src: &'s str) -> Self {
        Lower {
            src,
            errors: Vec::new(),
        }
    }

    pub fn err(&mut self, message: impl Into<String>, span: Span) {
        self.errors.push(PlcOpenError::new(message, span));
    }

    fn tag(&self, n: XNode) -> Span {
        xml::tag_span(self.src, n)
    }

    fn ident_attr(&mut self, n: XNode, a: &str) -> Option<Ident> {
        match xml::attr(n, a).map(str::trim).filter(|s| !s.is_empty()) {
            Some(v) => Some(Ident::new(v, xml::attr_span(self.src, n, a))),
            None => {
                self.err(
                    format!("<{}> is missing its `{a}` attribute", xml::name(n)),
                    self.tag(n),
                );
                None
            }
        }
    }

    pub fn expr_attr(&mut self, n: XNode, a: &str, what: &str) -> Option<Expression> {
        match xml::attr_fragment(self.src, n, a) {
            Some(f) => embed::parse_expression(&f, what, &mut self.errors),
            None => {
                self.err(
                    format!("<{}> is missing its `{a}` attribute", xml::name(n)),
                    self.tag(n),
                );
                None
            }
        }
    }

    pub fn expr_content(&mut self, n: XNode, what: &str) -> Option<Expression> {
        let f = xml::content_fragment(self.src, n);
        embed::parse_expression(&f, what, &mut self.errors)
    }

    // ── Project ──

    pub fn project(&mut self, root: XNode) -> Vec<Declaration> {
        let mut decls = Vec::new();
        if let Some(types) = xml::child(root, "types") {
            if let Some(dts) = xml::child(types, "dataTypes") {
                for dt in xml::children(dts, "dataType") {
                    if let Some(d) = self.data_type(dt) {
                        decls.push(Declaration::TypeDecl(d));
                    }
                }
            }
            if let Some(pous) = xml::child(types, "pous") {
                for pou in xml::children(pous, "pou") {
                    if let Some(d) = self.pou(pou) {
                        decls.push(d);
                    }
                }
            }
        }
        if let Some(inst) = xml::child(root, "instances")
            && let Some(cfgs) = xml::child(inst, "configurations")
        {
            for cfg in xml::children(cfgs, "configuration") {
                if let Some(c) = self.configuration(cfg) {
                    decls.push(Declaration::Configuration(c));
                }
            }
        }
        decls
    }

    // ── Types ──

    fn data_type(&mut self, dt: XNode) -> Option<TypeDeclaration> {
        let name = self.ident_attr(dt, "name")?;
        let Some(base) = xml::child(dt, "baseType") else {
            self.err("<dataType> has no <baseType>", self.tag(dt));
            return None;
        };
        let type_spec = self.type_of(base)?;
        let initializer = xml::child(dt, "initialValue").and_then(|iv| self.initial_value(iv));
        Some(TypeDeclaration {
            extends: None,
            name,
            type_spec,
            initializer,
            span: xml::span(dt),
        })
    }

    /// The type inside a `<type>`, `<baseType>` or `<returnType>` wrapper.
    pub fn type_of(&mut self, wrapper: XNode) -> Option<TypeSpec> {
        let mut inner = xml::elements(wrapper)
            .filter(|e| !matches!(xml::name(*e), "addData" | "documentation"));
        match inner.next() {
            Some(e) => self.type_elem(e),
            None => {
                self.err(
                    format!("<{}> names no type", xml::name(wrapper)),
                    self.tag(wrapper),
                );
                None
            }
        }
    }

    fn type_elem(&mut self, e: XNode) -> Option<TypeSpec> {
        let span = xml::span(e);
        let named = |n: &str, span| TypeSpec {
            kind: TypeSpecKind::Named(Ident::new(n, span)),
            span,
        };
        let kind = match xml::name(e) {
            "derived" => {
                let n = self.ident_attr(e, "name")?;
                return Some(TypeSpec {
                    span: n.span,
                    kind: TypeSpecKind::Named(n),
                });
            }
            "string" | "wstring" => {
                let length = match xml::attr(e, "length") {
                    Some(_) => Some(Box::new(self.expr_attr(e, "length", "string length")?)),
                    None => None,
                };
                TypeSpecKind::StringType {
                    wide: xml::name(e) == "wstring",
                    length,
                }
            }
            "array" => {
                let mut ranges = Vec::new();
                for d in xml::children(e, "dimension") {
                    let low = self.expr_attr(d, "lower", "array lower bound")?;
                    let high = self.expr_attr(d, "upper", "array upper bound")?;
                    ranges.push(SubrangeSpec {
                        low,
                        high,
                        span: xml::span(d),
                    });
                }
                if ranges.is_empty() {
                    self.err("<array> has no <dimension>", self.tag(e));
                    return None;
                }
                let base = self.base_type(e)?;
                TypeSpecKind::Array {
                    ranges,
                    base: Box::new(base),
                }
            }
            "pointer" => TypeSpecKind::Pointer(Box::new(self.base_type(e)?)),
            "enum" => {
                let mut values = Vec::new();
                if let Some(vs) = xml::child(e, "values") {
                    for v in xml::children(vs, "value") {
                        let name = self.ident_attr(v, "name")?;
                        let value = match xml::attr(v, "value") {
                            Some(_) => Some(self.expr_attr(v, "value", "enumerator value")?),
                            None => None,
                        };
                        values.push(EnumValue {
                            name,
                            value,
                            span: xml::span(v),
                        });
                    }
                }
                let base_type = match xml::child(e, "baseType") {
                    Some(b) => match self.type_of(b)?.kind {
                        TypeSpecKind::Named(id) => Some(id),
                        _ => {
                            self.err(
                                "an enum base type must be an elementary integer type",
                                xml::span(b),
                            );
                            return None;
                        }
                    },
                    None => None,
                };
                TypeSpecKind::Enum(EnumSpec {
                    base_type,
                    values,
                    span,
                })
            }
            "struct" => TypeSpecKind::Struct(self.struct_fields(e)?),
            "subrangeSigned" | "subrangeUnsigned" => {
                let Some(r) = xml::child(e, "range") else {
                    self.err("subrange without <range>", self.tag(e));
                    return None;
                };
                let low = self.expr_attr(r, "lower", "subrange lower bound")?;
                let high = self.expr_attr(r, "upper", "subrange upper bound")?;
                let base = match self.base_type(e)?.kind {
                    TypeSpecKind::Named(id) => id,
                    _ => {
                        self.err(
                            "a subrange base type must be an elementary integer type",
                            span,
                        );
                        return None;
                    }
                };
                TypeSpecKind::Subrange {
                    base,
                    low: Box::new(low),
                    high: Box::new(high),
                }
            }
            other => {
                let upper = other.to_ascii_uppercase();
                if !ELEMENTARY.contains(&upper.as_str()) {
                    self.err(format!("unsupported PLCopen type element <{other}>"), span);
                    return None;
                }
                let canonical = match upper.as_str() {
                    "DT" => "DATE_AND_TIME",
                    "TOD" => "TIME_OF_DAY",
                    u => u,
                };
                return Some(named(canonical, span));
            }
        };
        Some(TypeSpec { kind, span })
    }

    fn base_type(&mut self, e: XNode) -> Option<TypeSpec> {
        match xml::child(e, "baseType") {
            Some(b) => self.type_of(b),
            None => {
                self.err(format!("<{}> has no <baseType>", xml::name(e)), self.tag(e));
                None
            }
        }
    }

    fn struct_fields(&mut self, e: XNode) -> Option<Vec<StructField>> {
        let mut fields = Vec::new();
        for v in xml::children(e, "variable") {
            let name = self.ident_attr(v, "name")?;
            let Some(t) = xml::child(v, "type") else {
                self.err("struct member has no <type>", self.tag(v));
                return None;
            };
            let type_spec = self.type_of(t)?;
            let initializer = xml::child(v, "initialValue").and_then(|iv| self.initial_value(iv));
            fields.push(StructField {
                name,
                type_spec,
                initializer,
                span: xml::span(v),
            });
        }
        Some(fields)
    }

    /// `<initialValue>` → an initializer expression.
    pub fn initial_value(&mut self, iv: XNode) -> Option<Expression> {
        let v = xml::elements(iv).next()?;
        self.value(v)
    }

    fn value(&mut self, v: XNode) -> Option<Expression> {
        match xml::name(v) {
            "simpleValue" => self.expr_attr(v, "value", "initial value"),
            "arrayValue" => {
                let mut elems = Vec::new();
                for item in xml::children(v, "value") {
                    let repeat = match xml::attr(item, "repetitionValue") {
                        Some(_) => Some(Box::new(self.expr_attr(
                            item,
                            "repetitionValue",
                            "repetition count",
                        )?)),
                        None => None,
                    };
                    let Some(inner) = xml::elements(item).next() else {
                        self.err(
                            "array initial value element without a value",
                            self.tag(item),
                        );
                        return None;
                    };
                    let value = self.value(inner)?;
                    elems.push(ArrayInitElement {
                        repeat,
                        value: Box::new(value),
                        span: xml::span(item),
                    });
                }
                Some(Expression {
                    kind: ExpressionKind::ArrayInitializer(elems),
                    span: xml::span(v),
                })
            }
            "structValue" => {
                self.errors.push(
                    PlcOpenError::new(
                        "struct initial values (<structValue>) are not supported yet",
                        xml::span(v),
                    )
                    .with_help(
                        "plcc's AST has no struct aggregate initializer; give the STRUCT type \
                         field defaults instead",
                    ),
                );
                None
            }
            other => {
                self.err(format!("unsupported initial value <{other}>"), xml::span(v));
                None
            }
        }
    }

    // ── Variables ──

    fn variable(&mut self, v: XNode) -> Option<VarDecl> {
        let name = self.ident_attr(v, "name")?;
        let Some(t) = xml::child(v, "type") else {
            self.err(
                format!("variable `{}` has no <type>", name.name),
                self.tag(v),
            );
            return None;
        };
        let type_spec = self.type_of(t)?;
        let at_address = match xml::attr(v, "address") {
            Some(a) => {
                let repr = a.trim().to_string();
                let span = xml::attr_span(self.src, v, "address");
                if !repr.starts_with('%') {
                    self.err(
                        format!("address `{repr}` is not a direct variable (%I, %Q, %M)"),
                        span,
                    );
                    return None;
                }
                Some(DirectVariable { repr, span })
            }
            None => None,
        };
        let initializer = xml::child(v, "initialValue").and_then(|iv| self.initial_value(iv));
        Some(VarDecl {
            init_args: Vec::new(),
            name,
            type_spec,
            at_address,
            edge: None,
            initializer,
            span: xml::span(v),
        })
    }

    /// A `<localVars>`-style section → a VAR block.
    pub fn var_block(&mut self, section: XNode, kind: VarBlockKind) -> VarBlock {
        let declarations = xml::children(section, "variable")
            .filter_map(|v| self.variable(v))
            .collect();
        VarBlock {
            list_name: None,
            kind,
            is_constant: xml::attr_bool(section, "constant"),
            is_retain: xml::attr_bool(section, "retain") || xml::attr_bool(section, "persistent"),
            is_non_retain: xml::attr_bool(section, "nonretain"),
            declarations,
            span: xml::span(section),
        }
    }

    fn interface(&mut self, iface: XNode) -> (Vec<VarBlock>, Option<TypeSpec>) {
        let mut blocks = Vec::new();
        let mut ret = None;
        for sec in xml::elements(iface) {
            let kind = match xml::name(sec) {
                "returnType" => {
                    ret = self.type_of(sec);
                    continue;
                }
                "inputVars" => VarBlockKind::VarInput,
                "outputVars" => VarBlockKind::VarOutput,
                "inOutVars" => VarBlockKind::VarInOut,
                "localVars" => VarBlockKind::Var,
                "tempVars" => VarBlockKind::VarTemp,
                "externalVars" => VarBlockKind::VarExternal,
                "globalVars" => VarBlockKind::VarGlobal,
                "accessVars" => VarBlockKind::VarAccess,
                "addData" | "documentation" => continue,
                other => {
                    self.err(
                        format!("unsupported interface section <{other}>"),
                        self.tag(sec),
                    );
                    continue;
                }
            };
            blocks.push(self.var_block(sec, kind));
        }
        (blocks, ret)
    }

    // ── POUs ──

    fn pou(&mut self, pou: XNode) -> Option<Declaration> {
        let name = self.ident_attr(pou, "name")?;
        let kind = match xml::attr(pou, "pouType").map(str::trim) {
            Some("program") => PouKind::Program,
            Some("functionBlock") => PouKind::FunctionBlock,
            Some("function") => PouKind::Function,
            other => {
                self.err(
                    format!(
                        "unknown pouType {:?} (expected program, functionBlock or function)",
                        other.unwrap_or("")
                    ),
                    xml::attr_span(self.src, pou, "pouType"),
                );
                return None;
            }
        };
        let (mut var_blocks, return_type) = match xml::child(pou, "interface") {
            Some(i) => self.interface(i),
            None => (Vec::new(), None),
        };
        if kind == PouKind::Function && return_type.is_none() {
            self.err(
                format!("function `{}` has no <returnType>", name.name),
                self.tag(pou),
            );
        }
        for extra in ["actions", "transitions"] {
            if let Some(a) = xml::child(pou, extra)
                && xml::elements(a).next().is_some()
            {
                self.errors.push(
                    PlcOpenError::new(
                        format!("POU <{extra}> are not supported yet"),
                        self.tag(a),
                    )
                    .with_help("actions and transitions belong to SFC, which plcc does not support yet"),
                );
            }
        }
        let body = match xml::child(pou, "body") {
            Some(b) => self.body(b, kind, &name.name, &mut var_blocks),
            None => {
                self.err(format!("POU `{}` has no <body>", name.name), self.tag(pou));
                Vec::new()
            }
        };
        let span = xml::span(pou);
        Some(match kind {
            PouKind::Program => Declaration::Program(ProgramDecl {
                name,
                var_blocks,
                body,
                span,
            }),
            PouKind::FunctionBlock => Declaration::FunctionBlock(FunctionBlockDecl {
                name,
                extends: None,
                implements: Vec::new(),
                var_blocks,
                methods: Vec::new(),
                properties: Vec::new(),
                actions: Vec::new(),
                body,
                span,
            }),
            PouKind::Function => Declaration::Function(FunctionDecl {
                name,
                return_type,
                var_blocks,
                body,
                span,
            }),
        })
    }

    fn body(
        &mut self,
        body: XNode,
        kind: PouKind,
        pou_name: &str,
        var_blocks: &mut Vec<VarBlock>,
    ) -> Vec<Statement> {
        let Some(lang) =
            xml::elements(body).find(|e| !matches!(xml::name(*e), "documentation" | "addData"))
        else {
            self.err(
                format!("POU `{pou_name}` has an empty <body>"),
                self.tag(body),
            );
            return Vec::new();
        };
        match xml::name(lang) {
            "ST" => {
                let f: Fragment = xml::content_fragment(self.src, lang);
                embed::parse_statements(&f, &mut self.errors)
            }
            "LD" | "FBD" => graph::lower_body(self, lang, kind, var_blocks),
            "SFC" => {
                self.errors.push(
                    PlcOpenError::new(
                        format!("POU `{pou_name}`: SFC bodies are not supported yet"),
                        self.tag(lang),
                    )
                    .with_help("plcc supports ST, LD and FBD bodies in PLCopen XML"),
                );
                Vec::new()
            }
            "IL" => {
                self.errors.push(
                    PlcOpenError::new(
                        format!("POU `{pou_name}`: IL bodies are not supported"),
                        self.tag(lang),
                    )
                    .with_help("IL was deprecated by IEC 61131-3:2013; translate it to ST"),
                );
                Vec::new()
            }
            other => {
                self.err(format!("unknown body language <{other}>"), self.tag(lang));
                Vec::new()
            }
        }
    }

    // ── Configurations ──

    fn configuration(&mut self, cfg: XNode) -> Option<ConfigurationDecl> {
        let name = self.ident_attr(cfg, "name")?;
        let global_vars = xml::children(cfg, "globalVars")
            .map(|g| self.var_block(g, VarBlockKind::VarGlobal))
            .collect();
        let mut resources = Vec::new();
        for res in xml::children(cfg, "resource") {
            if let Some(r) = self.resource(res) {
                resources.push(r);
            }
        }
        Some(ConfigurationDecl {
            name,
            global_vars,
            resources,
            span: xml::span(cfg),
        })
    }

    fn resource(&mut self, res: XNode) -> Option<ResourceDecl> {
        let name = self.ident_attr(res, "name")?;
        let global_vars = xml::children(res, "globalVars")
            .map(|g| self.var_block(g, VarBlockKind::VarGlobal))
            .collect();
        let mut tasks = Vec::new();
        let mut program_configs = Vec::new();
        for task in xml::children(res, "task") {
            let Some(tname) = self.ident_attr(task, "name") else {
                continue;
            };
            let mut properties = Vec::new();
            if let Some(iv) = xml::attr(task, "interval") {
                let span = xml::attr_span(self.src, task, "interval");
                match iso_duration(iv) {
                    Some(lit) => properties.push((
                        Ident::new("INTERVAL", span),
                        Expression {
                            kind: ExpressionKind::TimeLiteral(lit),
                            span,
                        },
                    )),
                    None => {
                        if let Some(e) = self.expr_attr(task, "interval", "task interval") {
                            properties.push((Ident::new("INTERVAL", span), e));
                        }
                    }
                }
            }
            if xml::attr(task, "priority").is_some() {
                let span = xml::attr_span(self.src, task, "priority");
                if let Some(e) = self.expr_attr(task, "priority", "task priority") {
                    properties.push((Ident::new("PRIORITY", span), e));
                }
            }
            if xml::attr(task, "single").is_some_and(|s| !s.trim().is_empty()) {
                let span = xml::attr_span(self.src, task, "single");
                if let Some(e) = self.expr_attr(task, "single", "task SINGLE input") {
                    properties.push((Ident::new("SINGLE", span), e));
                }
            }
            for pi in xml::children(task, "pouInstance") {
                if let Some(pc) = self.pou_instance(pi, Some(tname.clone())) {
                    program_configs.push(pc);
                }
            }
            tasks.push(TaskDecl {
                name: tname,
                properties,
                span: xml::span(task),
            });
        }
        for pi in xml::children(res, "pouInstance") {
            if let Some(pc) = self.pou_instance(pi, None) {
                program_configs.push(pc);
            }
        }
        Some(ResourceDecl {
            name,
            on: None,
            global_vars,
            tasks,
            program_configs,
            span: xml::span(res),
        })
    }

    fn pou_instance(&mut self, pi: XNode, task: Option<Ident>) -> Option<ProgramConfig> {
        let name = self.ident_attr(pi, "name")?;
        let program_type = self.ident_attr(pi, "typeName")?;
        Some(ProgramConfig {
            name,
            task,
            program_type,
            connections: Vec::new(),
            span: xml::span(pi),
        })
    }
}

/// An xsd:duration task interval (`PT0.02S`, as some tools write it) → `T#20ms`.
/// `None` for anything else (IEC `T#20ms` is parsed as ST).
fn iso_duration(s: &str) -> Option<String> {
    let rest = s.trim().strip_prefix("PT")?;
    let mut ns: f64 = 0.0;
    let mut num = String::new();
    for c in rest.chars() {
        match c {
            '0'..='9' | '.' => num.push(c),
            'H' | 'M' | 'S' => {
                let v: f64 = num.parse().ok()?;
                num.clear();
                ns += v * match c {
                    'H' => 3.6e12,
                    'M' => 6e10,
                    _ => 1e9,
                };
            }
            _ => return None,
        }
    }
    if !num.is_empty() {
        return None;
    }
    let ns = ns.round() as i64;
    Some(if ns % 1_000_000 == 0 {
        format!("T#{}ms", ns / 1_000_000)
    } else {
        format!("T#{}us", ns / 1_000)
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn iso_durations() {
        assert_eq!(super::iso_duration("PT0.02S").as_deref(), Some("T#20ms"));
        assert_eq!(super::iso_duration("PT1M").as_deref(), Some("T#60000ms"));
        assert_eq!(super::iso_duration("T#20ms"), None);
    }
}
