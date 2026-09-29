// SPDX-License-Identifier: MPL-2.0

//! CODESYS `FB_init`: a method an FB may declare, which runs once when an
//! instance is initialized, with the arguments written at the declaration:
//!
//! ```text
//! METHOD FB_init : BOOL
//! VAR_INPUT
//!     bInitRetains : BOOL;   // cold start (retain data is re-initialized)
//!     bInCopyCode  : BOOL;   // online change (never in plcc)
//!     nSize        : INT;    // the instance's own parameters follow
//! END_VAR
//!
//! buf : FB_Buffer(nSize := 16);   // or FB_Buffer(16)
//! ```
//!
//! plcc calls it after the instance's own `_init` (its declared initial values,
//! and recursively its members, including their FB_init), passing
//! `bInitRetains := TRUE` (plcc starts cold) and `bInCopyCode := FALSE`. The
//! arguments are evaluated in the initializing POU, so `THIS^` there is the
//! enclosing instance, as in CODESYS.

use super::*;
use plcc_st::span::Span;

/// Per FB (uppercase): the FB_init inputs, in order, with their declared
/// initial values.
pub(super) type Defaults = HashMap<String, Vec<(String, Option<Expression>, bool)>>;

pub(super) fn defaults(unit: &CompilationUnit) -> Defaults {
    let mut out = Defaults::new();
    for d in &unit.declarations {
        let (name, methods) = match d {
            Declaration::FunctionBlock(fb) => (&fb.name.name, &fb.methods),
            Declaration::Class(c) => (&c.name.name, &c.methods),
            _ => continue,
        };
        if let Some(m) = methods
            .iter()
            .find(|m| m.name.name.eq_ignore_ascii_case("FB_init"))
        {
            let inputs = m
                .var_blocks
                .iter()
                .filter(|b| b.kind == VarBlockKind::VarInput)
                .flat_map(|b| b.declarations.iter())
                .map(|v| {
                    let is_ref = matches!(v.type_spec.kind, TypeSpecKind::Reference(_));
                    (v.name.name.clone(), v.initializer.clone(), is_ref)
                })
                .collect();
            out.insert(name.to_uppercase(), inputs);
        }
    }
    out
}

/// A zero value of an elementary type, for an FB_init input the declaration
/// leaves out and that has no initial value of its own.
fn zero_of(ty: &IecType, span: Span) -> Option<Expression> {
    let kind = match ty.base() {
        IecType::Bool => ExpressionKind::BoolLiteral(false),
        t if t.is_any_real() => ExpressionKind::RealLiteral(0.0),
        t if t.is_any_int() || t.is_any_bit() => ExpressionKind::IntegerLiteral(0),
        IecType::StringType { .. } => ExpressionKind::StringLiteral(String::new()),
        IecType::Time => ExpressionKind::TimeLiteral("T#0ms".into()),
        IecType::Ltime => ExpressionKind::TimeLiteral("LTIME#0ns".into()),
        // An address, or an unbound interface reference.
        IecType::Pointer(_) => ExpressionKind::IntegerLiteral(0),
        _ => return None,
    };
    Some(Expression { kind, span })
}

impl<'ctx> Compiler<'ctx> {
    /// The FB_init method of FB type `fb`, when it has one.
    fn fb_init_method(&self, fb: &str) -> Option<MethodInfo> {
        self.compiled_fbs
            .get(&fb.to_uppercase())?
            .methods
            .get("FB_INIT")
            .cloned()
    }

    /// After `<fb>_init` of the member `name` (a variable in scope in the init
    /// function being emitted), call its FB_init with `init_args`.
    pub(super) fn emit_fb_init_call(
        &mut self,
        pou: &str,
        name: &str,
        fb: &str,
        init_args: &[CallArg],
        span: Span,
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        let Some(method) = self.fb_init_method(fb) else {
            if init_args.is_empty() {
                return Ok(());
            }
            return Err(CodegenError::Located {
                message: format!(
                    "`{name}` passes FB_init arguments, but `{fb}` has no FB_init method"
                ),
                span,
            });
        };
        let bool_lit = |v: bool| Expression {
            kind: ExpressionKind::BoolLiteral(v),
            span,
        };
        let implicit = |i: usize, v: bool, named: bool| CallArg {
            name: named
                .then(|| {
                    method
                        .params
                        .get(i)
                        .map(|p| Ident::new(p.name.clone(), span))
                })
                .flatten(),
            value: bool_lit(v),
            is_output: false,
            negated: false,
            span,
        };
        // Named unless the declaration passes positional arguments: a named call
        // may leave the instance's own inputs out (they start at their defaults).
        let named = init_args.is_empty() || init_args.iter().any(|a| a.name.is_some());
        let mut args = Vec::new();
        if method.params.len() >= 2 {
            args.push(implicit(0, true, named));
            args.push(implicit(1, false, named));
        }
        args.extend(init_args.iter().cloned());
        // Inputs the declaration leaves out take their declared initial value
        // (or zero), as in CODESYS.
        let declared = self
            .fb_init_defaults
            .get(&fb.to_uppercase())
            .cloned()
            .unwrap_or_default();
        let given = args.len();
        for (i, p) in method.params.iter().enumerate().skip(2) {
            if p.is_output || p.is_in_out {
                continue;
            }
            let supplied = if named {
                args.iter().any(|a| {
                    a.name
                        .as_ref()
                        .is_some_and(|n| n.name.eq_ignore_ascii_case(&p.name))
                })
            } else {
                i < given
            };
            if supplied {
                continue;
            }
            let init = declared
                .iter()
                .find(|(n, _, _)| n.eq_ignore_ascii_case(&p.name))
                .and_then(|(_, e, _)| e.clone())
                .or_else(|| zero_of(&p.ty, span));
            if let Some(value) = init {
                args.push(CallArg {
                    name: named.then(|| Ident::new(p.name.clone(), span)),
                    value,
                    is_output: false,
                    negated: false,
                    span,
                });
            }
        }

        // A REFERENCE TO input is passed by address (`__REF_OF`, as `REF=` does).
        let ref_params: Vec<String> = declared
            .iter()
            .filter(|(_, _, is_ref)| *is_ref)
            .map(|(n, _, _)| n.to_uppercase())
            .collect();
        if !ref_params.is_empty() {
            for (i, a) in args.iter_mut().enumerate() {
                let param = match &a.name {
                    Some(n) => n.name.to_uppercase(),
                    None => method
                        .params
                        .get(i)
                        .map(|p| p.name.to_uppercase())
                        .unwrap_or_default(),
                };
                if ref_params.contains(&param) {
                    let value = a.value.clone();
                    let vspan = value.span;
                    a.value = Expression {
                        kind: ExpressionKind::FunctionCall {
                            callee: Box::new(Expression {
                                kind: ExpressionKind::Identifier(Ident::new(
                                    "__REF_OF".to_string(),
                                    vspan,
                                )),
                                span: vspan,
                            }),
                            args: vec![CallArg {
                                name: None,
                                value,
                                is_output: false,
                                negated: false,
                                span: vspan,
                            }],
                        },
                        span: vspan,
                    };
                }
            }
        }

        // `THIS^` in the arguments is the instance being initialized around
        // this member.
        let saved_pou = self.current_pou.clone();
        let saved_this = self.variables.get("THIS").cloned();
        if saved_this.is_none()
            && self.compiled_fbs.contains_key(&pou.to_uppercase())
            && let Some(state) = self.current_state_ptr
        {
            self.bind_this_super(pou, None, state, function)?;
        }
        let stmt = Statement {
            kind: StatementKind::FunctionCall {
                callee: Expression {
                    kind: ExpressionKind::MemberAccess {
                        object: Box::new(Expression {
                            kind: ExpressionKind::Identifier(Ident::new(name.to_string(), span)),
                            span,
                        }),
                        member: Ident::new("FB_init".to_string(), span),
                    },
                    span,
                },
                args,
            },
            span,
        };
        let result = self.compile_statement(&stmt, function);
        self.current_pou = saved_pou;
        if saved_this.is_none() {
            self.variables.remove("THIS");
            self.variables.remove("SUPER");
        }
        result
    }
}
