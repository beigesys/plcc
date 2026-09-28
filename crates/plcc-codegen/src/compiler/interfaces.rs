// SPDX-License-Identifier: MPL-2.0

//! INTERFACE references (IEC 61131-3 3rd ed. §6.6.4, CODESYS).
//!
//! A variable of an INTERFACE type holds a reference to an instance of any FB or
//! CLASS that implements the interface: here a pair `{instance pointer, method
//! table}`. The method table of (POU, interface) is a constant array of the
//! POU's method functions in the interface's method order (inherited
//! interfaces first). `itf := inst` stores the pair; `itf.M(..)` calls through
//! the table (late binding); `itf = 0` / `itf <> 0` test whether it is bound.
//!
//! Interface variables used to be a single pointer that nothing could assign or
//! call through: `itf := inst` and `itf.M()` were compile errors.

use super::*;

/// Name prefix of the STRUCT type that represents an INTERFACE reference.
const ITF_PREFIX: &str = "__ITF_";

/// One interface's methods, in table order.
#[derive(Clone, Default)]
pub(super) struct InterfaceTable {
    /// Uppercase interface name → (uppercase method name, parameters, result).
    methods: HashMap<String, Vec<(String, Vec<Param>, IecType)>>,
    /// Uppercase POU → uppercase interfaces it implements (also through its bases).
    implements: HashMap<String, Vec<String>>,
}

impl<'ctx> Compiler<'ctx> {
    /// The type of a variable declared with INTERFACE type `name`.
    pub(super) fn interface_type(name: &str) -> IecType {
        IecType::Struct {
            name: format!("{ITF_PREFIX}{}", name.to_uppercase()),
            fields: vec![
                ("__OBJ".into(), IecType::Pointer(Box::new(IecType::Void))),
                ("__VT".into(), IecType::Pointer(Box::new(IecType::Void))),
            ],
        }
    }

    /// The interface an INTERFACE-reference type refers to.
    pub(super) fn interface_of_type(ty: &IecType) -> Option<String> {
        match ty.base() {
            IecType::Struct { name, .. } => name.strip_prefix(ITF_PREFIX).map(str::to_string),
            _ => None,
        }
    }

    pub(super) fn interface_of_expr(&self, expr: &Expression) -> Option<String> {
        self.lvalue_iec_type(expr)
            .as_ref()
            .and_then(Self::interface_of_type)
    }

    /// Resolve every interface's method list and every POU's IMPLEMENTS set.
    pub(super) fn layout_interfaces(&mut self, unit: &CompilationUnit) {
        let mut decls: HashMap<String, &InterfaceDecl> = HashMap::new();
        for d in &unit.declarations {
            if let Declaration::Interface(i) = d {
                decls.insert(i.name.name.to_uppercase(), i);
            }
        }
        fn collect<'a>(
            name: &str,
            decls: &HashMap<String, &'a InterfaceDecl>,
            seen: &mut Vec<String>,
            out: &mut Vec<&'a MethodDecl>,
        ) {
            if seen.iter().any(|s| s == name) {
                return;
            }
            seen.push(name.to_string());
            let Some(i) = decls.get(name) else {
                return;
            };
            for base in &i.extends {
                collect(&base.name.to_uppercase(), decls, seen, out);
            }
            for m in &i.methods {
                if !out.iter().any(|o| o.name.name.eq_ignore_ascii_case(&m.name.name)) {
                    out.push(m);
                }
            }
        }
        let names: Vec<String> = decls.keys().cloned().collect();
        for n in names {
            let mut ms = Vec::new();
            collect(&n, &decls, &mut Vec::new(), &mut ms);
            let table: Vec<(String, Vec<Param>, IecType)> = ms
                .iter()
                .map(|m| {
                    let params = self.resolve_params(&m.var_blocks);
                    let ret = m
                        .return_type
                        .as_ref()
                        .map(|t| self.resolve_type_spec(t))
                        .unwrap_or(IecType::Void);
                    (m.name.name.to_uppercase(), params, ret)
                })
                .collect();
            self.interfaces.methods.insert(n, table);
        }
        // IMPLEMENTS, including what a base implements and interfaces those extend.
        let mut direct: HashMap<String, Vec<String>> = HashMap::new();
        for d in &unit.declarations {
            let (name, imps) = match d {
                Declaration::FunctionBlock(fb) => (&fb.name.name, &fb.implements),
                Declaration::Class(c) => (&c.name.name, &c.implements),
                _ => continue,
            };
            direct.insert(
                name.to_uppercase(),
                imps.iter().map(|i| i.name.to_uppercase()).collect(),
            );
        }
        let pous: Vec<String> = direct.keys().cloned().collect();
        for p in pous {
            let mut all: Vec<String> = Vec::new();
            let mut cur = Some(p.clone());
            let mut guard = 0;
            while let Some(c) = cur.take() {
                guard += 1;
                if guard > 64 {
                    break;
                }
                for i in direct.get(&c).cloned().unwrap_or_default() {
                    let mut stack = vec![i];
                    while let Some(i) = stack.pop() {
                        if all.contains(&i) {
                            continue;
                        }
                        if let Some(d) = decls.get(&i) {
                            stack.extend(d.extends.iter().map(|e| e.name.to_uppercase()));
                        }
                        all.push(i);
                    }
                }
                cur = self.hierarchy.bases.get(&c).map(|b| b.to_uppercase());
            }
            self.interfaces.implements.insert(p, all);
        }
    }

    /// The method table of POU `pou` for interface `iface`.
    fn interface_vtable(&mut self, pou: &str, iface: &str) -> Result<PointerValue<'ctx>, CodegenError> {
        let sym = format!("__vt_{}_{}", pou.to_lowercase(), iface.to_lowercase());
        if let Some(g) = self.module.get_global(&sym) {
            return Ok(g.as_pointer_value());
        }
        let layout = self.compiled_fbs.get(&pou.to_uppercase()).cloned().ok_or_else(|| {
            CodegenError::UndefinedVariable(format!("`{pou}` is not a FUNCTION_BLOCK or CLASS"))
        })?;
        let methods = self.interfaces.methods.get(iface).cloned().unwrap_or_default();
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let mut entries = Vec::new();
        for (m, _, _) in &methods {
            let info = layout.methods.get(m).ok_or_else(|| {
                CodegenError::UndefinedVariable(format!(
                    "`{pou}` does not implement method `{m}` of INTERFACE `{iface}`"
                ))
            })?;
            let f = self.module.get_function(&info.fn_name).ok_or_else(|| {
                CodegenError::LlvmError(format!("method function `{}` not found", info.fn_name))
            })?;
            entries.push(f.as_global_value().as_pointer_value());
        }
        let init = ptr_ty.const_array(&entries);
        let g = self.module.add_global(init.get_type(), None, &sym);
        g.set_initializer(&init);
        g.set_constant(true);
        g.set_linkage(inkwell::module::Linkage::Private);
        Ok(g.as_pointer_value())
    }

    /// `{instance, table}` for binding FB/CLASS instance `expr` to an `iface`
    /// reference, or `None` when `expr` is not such an instance.
    pub(super) fn interface_value(
        &mut self,
        expr: &Expression,
        iface: &str,
        function: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>, CodegenError> {
        let Some(IecType::FbInstance(pou)) = self.lvalue_iec_type(expr) else {
            return Ok(None);
        };
        let implements = self
            .interfaces
            .implements
            .get(&pou.to_uppercase())
            .is_some_and(|l| l.iter().any(|i| i == iface));
        if !implements {
            return Err(CodegenError::UnsupportedType(format!(
                "`{}` is a `{pou}`, which does not implement INTERFACE `{iface}`",
                Self::describe_lvalue(expr)
            )));
        }
        let obj = self.compile_lvalue_with_fn(expr, function)?.ok_or_else(|| {
            CodegenError::UnsupportedType(format!(
                "`{}` has no address to reference",
                Self::describe_lvalue(expr)
            ))
        })?;
        let vt = self.interface_vtable(&pou, iface)?;
        let st = self
            .iec_to_llvm_type(&Self::interface_type(iface))
            .into_struct_type();
        let err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
        let v = self
            .builder
            .build_insert_value(st.get_undef(), obj, 0, "itf_obj")
            .map_err(err)?;
        let v = self.builder.build_insert_value(v, vt, 1, "itf_vt").map_err(err)?;
        Ok(Some(v.into_struct_value().into()))
    }

    /// `target := inst` for an INTERFACE-typed target. Returns false when the
    /// target is not an interface reference or the value is not an instance.
    pub(super) fn try_compile_interface_store(
        &mut self,
        target: &Expression,
        value: &Expression,
        function: FunctionValue<'ctx>,
    ) -> Result<bool, CodegenError> {
        let Some(iface) = self.interface_of_expr(target) else {
            return Ok(false);
        };
        let v = if matches!(value.kind, ExpressionKind::IntegerLiteral(0)) {
            // `itf := 0`: unbind.
            Some(
                self.iec_to_llvm_type(&Self::interface_type(&iface))
                    .into_struct_type()
                    .const_zero()
                    .into(),
            )
        } else {
            self.interface_value(value, &iface, function)?
        };
        let Some(v) = v else {
            return Ok(false);
        };
        let ptr = self.compile_lvalue_with_fn(target, function)?.ok_or_else(|| {
            CodegenError::UnsupportedType(format!(
                "`{}` is not an assignable location",
                Self::describe_lvalue(target)
            ))
        })?;
        self.builder
            .build_store(ptr, v)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        Ok(true)
    }

    /// The instance pointer of an interface reference.
    fn interface_object(
        &mut self,
        itf: &Expression,
        function: FunctionValue<'ctx>,
    ) -> Result<(PointerValue<'ctx>, PointerValue<'ctx>), CodegenError> {
        let err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
        let iface = self.interface_of_expr(itf).unwrap_or_default();
        let st = self
            .iec_to_llvm_type(&Self::interface_type(&iface))
            .into_struct_type();
        let ptr = self.compile_lvalue_with_fn(itf, function)?.ok_or_else(|| {
            CodegenError::UnsupportedType(format!(
                "`{}` has no address",
                Self::describe_lvalue(itf)
            ))
        })?;
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let objp = self.builder.build_struct_gep(st, ptr, 0, "itf_objp").map_err(err)?;
        let vtp = self.builder.build_struct_gep(st, ptr, 1, "itf_vtp").map_err(err)?;
        let obj = self.builder.build_load(ptr_ty, objp, "itf_obj").map_err(err)?.into_pointer_value();
        let vt = self.builder.build_load(ptr_ty, vtp, "itf_vt").map_err(err)?.into_pointer_value();
        Ok((obj, vt))
    }

    /// `itf = 0`, `itf <> 0`, `itf = other`: compare the referenced instances.
    pub(super) fn compile_interface_compare(
        &mut self,
        op: BinaryOp,
        left: &Expression,
        right: &Expression,
        function: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>, CodegenError> {
        let err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
        if !matches!(op, BinaryOp::Equal | BinaryOp::NotEqual) {
            return Ok(None);
        }
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let side = |this: &mut Self, e: &Expression| -> Result<Option<PointerValue<'ctx>>, CodegenError> {
            if this.interface_of_expr(e).is_some() {
                return Ok(Some(this.interface_object(e, function)?.0));
            }
            if matches!(e.kind, ExpressionKind::IntegerLiteral(0)) {
                return Ok(Some(ptr_ty.const_null()));
            }
            Ok(None)
        };
        let (Some(l), Some(r)) = (side(self, left)?, side(self, right)?) else {
            return Ok(None);
        };
        let li = self.builder.build_ptr_to_int(l, self.context.i64_type(), "l").map_err(err)?;
        let ri = self.builder.build_ptr_to_int(r, self.context.i64_type(), "r").map_err(err)?;
        let pred = if op == BinaryOp::Equal {
            IntPredicate::EQ
        } else {
            IntPredicate::NE
        };
        Ok(Some(
            self.builder
                .build_int_compare(pred, li, ri, "itf_cmp")
                .map_err(err)?
                .into(),
        ))
    }

    /// Result type of `itf.M(..)`.
    pub(super) fn interface_method_type(&self, object: &Expression, method: &str) -> Option<IecType> {
        let iface = self.interface_of_expr(object)?;
        self.interfaces
            .methods
            .get(&iface)?
            .iter()
            .find(|(m, _, _)| m.eq_ignore_ascii_case(method))
            .map(|(_, _, r)| r.clone())
            .filter(|t| *t != IecType::Void)
    }

    /// `itf.M(args)`: call entry M of the referenced instance's method table.
    pub(super) fn compile_interface_call(
        &mut self,
        object: &Expression,
        method: &str,
        args: &[CallArg],
        function: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>, CodegenError> {
        let err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
        let iface = self.interface_of_expr(object).unwrap_or_default();
        let label = format!("{}.{method}", Self::describe_lvalue(object));
        let methods = self.interfaces.methods.get(&iface).cloned().unwrap_or_default();
        let Some(index) = methods.iter().position(|(m, _, _)| m.eq_ignore_ascii_case(method)) else {
            return Err(CodegenError::UndefinedVariable(format!(
                "INTERFACE `{iface}` has no method `{method}`"
            )));
        };
        let (_, params, ret) = methods[index].clone();
        let (obj, vt) = self.interface_object(object, function)?;
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let slot = unsafe {
            self.builder.build_in_bounds_gep(
                ptr_ty,
                vt,
                &[self.context.i64_type().const_int(index as u64, false)],
                "vt_slot",
            )
        }
        .map_err(err)?;
        let fn_ptr = self.builder.build_load(ptr_ty, slot, "vt_fn").map_err(err)?.into_pointer_value();

        let mut param_types: Vec<BasicMetadataTypeEnum<'ctx>> = vec![ptr_ty.into()];
        param_types.extend(self.param_llvm_types(&params));
        let fn_type = if ret == IecType::Void {
            self.context.void_type().fn_type(&param_types, false)
        } else {
            self.iec_to_llvm_type(&ret).fn_type(&param_types, false)
        };

        let ordered = Self::bind_args(&label, &params, args)?;
        let mut call_args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = vec![obj.into()];
        let mut outputs = Vec::new();
        for (i, bound) in ordered.iter().enumerate() {
            let p = &params[i];
            if p.is_output {
                let tmp = self.entry_alloca(function, self.iec_to_llvm_type(&p.ty), &p.name)?;
                if let Some(a) = bound {
                    outputs.push((*a, tmp, p.ty.clone()));
                }
                call_args.push(tmp.into());
                continue;
            }
            let Some(bound) = bound else {
                return Err(CodegenError::ArgumentBinding {
                    callee: label.clone(),
                    problem: format!("no value for argument {}", i + 1),
                });
            };
            if p.is_in_out {
                let r = self.compile_argument_reference(&bound.value, &label, &p.name, function)?;
                call_args.push(r.into());
                continue;
            }
            let v = self.compile_call_arg(
                &bound.value,
                Some(&p.ty),
                &format!("argument {} of `{label}`", i + 1),
                function,
            )?;
            call_args.push(v.into());
        }
        let call = self
            .builder
            .build_indirect_call(fn_type, fn_ptr, &call_args, "itf_call")
            .map_err(err)?;
        for (a, tmp, ty) in outputs {
            self.copy_output(a, tmp, &ty, function)?;
        }
        Ok(match call.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(v) => Some(v),
            inkwell::values::ValueKind::Instruction(_) => None,
        })
    }
}
