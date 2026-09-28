// SPDX-License-Identifier: MPL-2.0

//! Inheritance: `FUNCTION_BLOCK D EXTENDS B` and `CLASS D EXTENDS B`.
//!
//! EXTENDS was parsed and then ignored: a derived FB had none of its base's
//! variables or methods, so any use of them was "unknown identifier".
//!
//! The model:
//!
//! * **Layout.** A derived instance is its base's fields followed by its own, so a
//!   pointer to a derived instance is also a valid pointer to its base part.
//! * **Methods.** A derived POU has its own methods plus every base method it does
//!   not override. An inherited method is compiled *again* as a method of the
//!   derived POU, so a call to another method from inside it — `THIS^.Area()`, or
//!   plain `Area()` — reaches the derived override. That is the late binding
//!   CODESYS methods have, for every call through a variable of a known FB type
//!   (INTERFACE references are not supported).
//! * **Body.** Calling a derived FB runs the derived body only; `SUPER^()` runs the
//!   base body, `SUPER^.M()` the base implementation of `M` (CODESYS semantics).
//! * `THIS^` is the current instance, `SUPER^` the same instance seen as the base.

use super::*;

struct Pou {
    extends: Option<Ident>,
    var_blocks: Vec<VarBlock>,
    methods: Vec<MethodDecl>,
}

/// What [`flatten_inheritance`] learned about the class hierarchy.
#[derive(Default)]
pub(super) struct Hierarchy {
    /// Uppercase derived POU → its direct base's name.
    pub(super) bases: HashMap<String, String>,
    /// (uppercase POU, uppercase method) → uppercase POU that declares the method,
    /// for methods a POU inherits. `SUPER^` inside such a method is the declaring
    /// POU's base, not the inheriting one's.
    pub(super) origins: HashMap<(String, String), String>,
}

/// The unit with every EXTENDS flattened into the derived POU's own variables
/// and methods (or `None` when nothing extends anything), and the hierarchy.
pub(super) fn flatten_inheritance(
    unit: &CompilationUnit,
) -> Result<(Option<CompilationUnit>, Hierarchy), CodegenError> {
    let mut pous: HashMap<String, Pou> = HashMap::new();
    for decl in &unit.declarations {
        let (name, pou) = match decl {
            Declaration::FunctionBlock(fb) => (
                &fb.name.name,
                Pou {
                    extends: fb.extends.clone(),
                    var_blocks: fb.var_blocks.clone(),
                    methods: fb.methods.clone(),
                },
            ),
            Declaration::Class(c) => (
                &c.name.name,
                Pou {
                    extends: c.extends.clone(),
                    var_blocks: c.var_blocks.clone(),
                    methods: c.methods.clone(),
                },
            ),
            _ => continue,
        };
        pous.insert(name.to_uppercase(), pou);
    }
    let bases: HashMap<String, String> = pous
        .iter()
        .filter_map(|(n, p)| p.extends.as_ref().map(|b| (n.clone(), b.name.clone())))
        .collect();
    if bases.is_empty() {
        return Ok((None, Hierarchy::default()));
    }

    // Flatten each POU once, bases first.
    let mut flat: HashMap<String, (Vec<VarBlock>, Vec<MethodDecl>)> = HashMap::new();
    let mut origins: HashMap<(String, String), String> = HashMap::new();
    fn resolve(
        name: &str,
        pous: &HashMap<String, Pou>,
        flat: &mut HashMap<String, (Vec<VarBlock>, Vec<MethodDecl>)>,
        origins: &mut HashMap<(String, String), String>,
        chain: &mut Vec<String>,
    ) -> Result<(), CodegenError> {
        if flat.contains_key(name) {
            return Ok(());
        }
        if chain.iter().any(|c| c == name) {
            return Err(CodegenError::UnsupportedType(format!(
                "`{}` extends itself (through {})",
                name,
                chain.join(" -> ")
            )));
        }
        let pou = &pous[name];
        let Some(base) = &pou.extends else {
            flat.insert(name.to_string(), (pou.var_blocks.clone(), pou.methods.clone()));
            return Ok(());
        };
        let base_key = base.name.to_uppercase();
        if !pous.contains_key(&base_key) {
            return Err(CodegenError::UndefinedVariable(format!(
                "`{name}` EXTENDS `{}`, which is not a FUNCTION_BLOCK or CLASS in this unit",
                base.name
            )));
        }
        chain.push(name.to_string());
        resolve(&base_key, pous, flat, origins, chain)?;
        chain.pop();
        let (base_vars, base_methods) = flat[&base_key].clone();
        let own_names: Vec<String> = pou
            .var_blocks
            .iter()
            .flat_map(|b| b.declarations.iter().map(|d| d.name.name.to_uppercase()))
            .collect();
        for b in &base_vars {
            for d in &b.declarations {
                if own_names.contains(&d.name.name.to_uppercase()) {
                    return Err(CodegenError::UnsupportedType(format!(
                        "`{name}` declares `{}`, which it already inherits from `{}`",
                        d.name.name, base.name
                    )));
                }
            }
        }
        let mut vars = base_vars;
        vars.extend(pou.var_blocks.iter().cloned());
        let mut methods = pou.methods.clone();
        for m in base_methods {
            if !methods
                .iter()
                .any(|own| own.name.name.eq_ignore_ascii_case(&m.name.name))
            {
                let key = m.name.name.to_uppercase();
                let origin = origins
                    .get(&(base_key.clone(), key.clone()))
                    .cloned()
                    .unwrap_or_else(|| base_key.clone());
                origins.insert((name.to_string(), key), origin);
                methods.push(m);
            }
        }
        flat.insert(name.to_string(), (vars, methods));
        Ok(())
    }
    let names: Vec<String> = pous.keys().cloned().collect();
    for n in &names {
        resolve(n, &pous, &mut flat, &mut origins, &mut Vec::new())?;
    }

    let mut out = unit.clone();
    for decl in &mut out.declarations {
        match decl {
            Declaration::FunctionBlock(fb) if fb.extends.is_some() => {
                let (v, m) = flat[&fb.name.name.to_uppercase()].clone();
                fb.var_blocks = v;
                fb.methods = m;
            }
            Declaration::Class(c) if c.extends.is_some() => {
                let (v, m) = flat[&c.name.name.to_uppercase()].clone();
                c.var_blocks = v;
                c.methods = m;
            }
            _ => {}
        }
    }
    Ok((Some(out), Hierarchy { bases, origins }))
}

impl<'ctx> Compiler<'ctx> {
    /// Bind `THIS` (and `SUPER`, when the POU extends another) to the instance at
    /// `state_ptr`, for the body or a method of FB/CLASS `pou`.
    pub(super) fn bind_this_super(
        &mut self,
        pou: &str,
        method: Option<&str>,
        state_ptr: PointerValue<'ctx>,
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let slot = self.entry_alloca(function, ptr_ty.into(), "this")?;
        self.builder
            .build_store(slot, state_ptr)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        self.variables.insert(
            "THIS".into(),
            (slot, IecType::Pointer(Box::new(IecType::FbInstance(pou.to_string())))),
        );
        // In an inherited method, SUPER is the base of the POU that declared it.
        let declaring = method
            .and_then(|m| {
                self.hierarchy
                    .origins
                    .get(&(pou.to_uppercase(), m.to_uppercase()))
                    .cloned()
            })
            .unwrap_or_else(|| pou.to_uppercase());
        if let Some(base) = self.hierarchy.bases.get(&declaring).cloned() {
            self.variables.insert(
                "SUPER".into(),
                (slot, IecType::Pointer(Box::new(IecType::FbInstance(base)))),
            );
        }
        self.current_pou = Some(pou.to_uppercase());
        Ok(())
    }

    /// `M(args)` written inside an FB/CLASS body or method, where `M` is a method
    /// of that POU and not a FUNCTION: the call `THIS^.M(args)`.
    pub(super) fn implicit_method_call(&self, callee: &Expression) -> Option<Expression> {
        let ExpressionKind::Identifier(id) = &callee.kind else {
            return None;
        };
        if self.fn_signatures.contains_key(&id.name.to_lowercase())
            || self.variables.contains_key(&id.name.to_uppercase())
        {
            return None;
        }
        let pou = self.current_pou.as_ref()?;
        let layout = self.compiled_fbs.get(pou)?;
        if !layout.methods.contains_key(&id.name.to_uppercase()) {
            return None;
        }
        let span = callee.span;
        let this = Expression {
            kind: ExpressionKind::Dereference(Box::new(Expression {
                kind: ExpressionKind::Identifier(Ident::new("THIS".to_string(), span)),
                span,
            })),
            span,
        };
        Some(Expression {
            kind: ExpressionKind::MemberAccess {
                object: Box::new(this),
                member: id.clone(),
            },
            span,
        })
    }
}
