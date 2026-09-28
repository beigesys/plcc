// SPDX-License-Identifier: MPL-2.0

//! STRING values anywhere in an expression.
//!
//! A STRING is a NUL-terminated byte buffer. Every string-valued expression — a
//! variable, a literal, a nested `CONCAT`/`LEFT`/`RIGHT`/`MID`/`INSERT`/`DELETE`/
//! `REPLACE` call, a FUNCTION returning STRING — can be turned into a pointer to
//! such a buffer ([`Compiler::compile_string_ptr`]). The string builtins always
//! build their result in a fresh temporary, so `s := CONCAT('a', s)` never reads
//! a buffer it is writing; the assignment then copies it into the destination,
//! truncating to the destination's length.
//!
//! This replaces a special case that only understood a string builtin as the
//! whole right-hand side of an assignment to a plain STRING variable, with only
//! variables as its arguments: a literal argument, a nested call, a comparison of
//! strings, `LEN('abc')` (silently 0) all failed.

use super::*;

/// Capacity, in bytes including the terminator, of a `STRING` with no length.
pub(super) const DEFAULT_STRING_BYTES: u32 = 256;

impl<'ctx> Compiler<'ctx> {
    /// The string builtins that produce a STRING.
    pub(super) fn is_string_builtin(name: &str) -> bool {
        matches!(
            name.to_uppercase().as_str(),
            "CONCAT" | "LEFT" | "RIGHT" | "MID" | "INSERT" | "DELETE" | "REPLACE"
        )
    }

    /// Bytes (with terminator) of a STRING type.
    pub(super) fn string_bytes(ty: &IecType) -> Option<u32> {
        match ty.base() {
            IecType::StringType { max_len } => {
                Some(max_len.unwrap_or(DEFAULT_STRING_BYTES as usize - 1) as u32 + 1)
            }
            _ => None,
        }
    }

    /// The name of a string builtin `expr` calls, unless a user FUNCTION of that
    /// name shadows it.
    fn string_builtin_call<'e>(&self, expr: &'e Expression) -> Option<(String, &'e [CallArg])> {
        let ExpressionKind::FunctionCall { callee, args } = &expr.kind else {
            return None;
        };
        let ExpressionKind::Identifier(id) = &callee.kind else {
            return None;
        };
        (Self::is_string_builtin(&id.name) && !self.fn_signatures.contains_key(&id.name.to_lowercase()))
            .then(|| (id.name.to_uppercase(), args.as_slice()))
    }

    /// Static STRING type of a literal or string-builtin call (variables and user
    /// FUNCTIONs are typed by the ordinary paths).
    pub(super) fn string_expr_type(&self, expr: &Expression) -> Option<IecType> {
        match &expr.kind {
            ExpressionKind::StringLiteral(s) => Some(IecType::StringType {
                max_len: Some(decode_iec_string(s, false).len()),
            }),
            ExpressionKind::Parenthesized(inner) => self.string_expr_type(inner),
            _ => {
                let (name, args) = self.string_builtin_call(expr)?;
                let bytes = self.string_result_bytes(&name, args);
                Some(IecType::StringType {
                    max_len: Some(bytes as usize - 1),
                })
            }
        }
    }

    /// Whether `expr` is a STRING value.
    pub(super) fn is_string_expr(&self, expr: &Expression) -> bool {
        self.string_expr_type(expr).is_some()
            || self
                .rvalue_iec_type(expr)
                .is_some_and(|t| Self::string_bytes(&t).is_some())
    }

    fn arg_string_bytes(&self, e: &Expression) -> u32 {
        self.string_expr_type(e)
            .or_else(|| self.rvalue_iec_type(e))
            .and_then(|t| Self::string_bytes(&t))
            .unwrap_or(DEFAULT_STRING_BYTES)
    }

    /// Capacity of a builtin's result: large enough for any result its inputs can
    /// produce, so nothing is cut short before the final assignment truncates.
    fn string_result_bytes(&self, name: &str, args: &[CallArg]) -> u32 {
        let cap = |i: usize| args.get(i).map_or(1, |a| self.arg_string_bytes(&a.value));
        let bytes = match name {
            "CONCAT" => args.iter().map(|a| self.arg_string_bytes(&a.value) - 1).sum::<u32>() + 1,
            "INSERT" | "REPLACE" => cap(0) + cap(1) - 1,
            _ => cap(0),
        };
        bytes.clamp(1, 1 << 16)
    }

    /// A private constant holding a STRING literal's bytes.
    fn string_literal_global(&self, raw: &str) -> PointerValue<'ctx> {
        let bytes: Vec<u8> = decode_iec_string(raw, false)
            .into_iter()
            .map(|c| c as u8)
            .collect();
        let init = self.context.const_string(&bytes, true);
        let g = self.module.add_global(init.get_type(), None, "strlit");
        g.set_initializer(&init);
        g.set_constant(true);
        g.set_linkage(inkwell::module::Linkage::Private);
        g.as_pointer_value()
    }

    /// A pointer to the bytes of the STRING `expr` denotes, and the capacity of
    /// that buffer. `None` when `expr` is not a STRING.
    pub(super) fn compile_string_ptr(
        &mut self,
        expr: &Expression,
        function: FunctionValue<'ctx>,
    ) -> Result<Option<(PointerValue<'ctx>, u32)>, CodegenError> {
        match &expr.kind {
            ExpressionKind::StringLiteral(s) => {
                let n = decode_iec_string(s, false).len() as u32;
                return Ok(Some((self.string_literal_global(s), n + 1)));
            }
            ExpressionKind::Parenthesized(inner) => return self.compile_string_ptr(inner, function),
            _ => {}
        }
        if let Some((name, args)) = self.string_builtin_call(expr) {
            let bytes = self.string_result_bytes(&name, args);
            let buf_ty = self.context.i8_type().array_type(bytes);
            let buf = self.entry_alloca(function, buf_ty.into(), "strtmp")?;
            self.emit_string_builtin(&name, args, buf, bytes, function)?;
            return Ok(Some((buf, bytes)));
        }
        if let Some(bytes) = self.lvalue_iec_type(expr).as_ref().and_then(Self::string_bytes) {
            if let Some(ptr) = self.compile_lvalue_with_fn(expr, function)? {
                return Ok(Some((ptr, bytes)));
            }
        }
        // A FUNCTION or METHOD returning STRING: spill its value.
        let is_call = matches!(expr.kind, ExpressionKind::FunctionCall { .. });
        if is_call && self.rvalue_iec_type(expr).as_ref().and_then(Self::string_bytes).is_some() {
            if let Some(BasicValueEnum::ArrayValue(av)) = self.compile_expression(expr, function)? {
                let tmp = self.entry_alloca(function, av.get_type().into(), "strret")?;
                self.builder
                    .build_store(tmp, av)
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                return Ok(Some((tmp, av.get_type().len())));
            }
        }
        Ok(None)
    }

    /// Like [`Self::compile_string_ptr`], but a non-STRING is a diagnostic naming
    /// `what`.
    pub(super) fn string_operand(
        &mut self,
        expr: &Expression,
        what: &str,
        function: FunctionValue<'ctx>,
    ) -> Result<(PointerValue<'ctx>, u32), CodegenError> {
        self.compile_string_ptr(expr, function)?.ok_or_else(|| {
            CodegenError::UnsupportedType(format!(
                "{what}: `{}` is not a STRING",
                Self::describe_lvalue(expr)
            ))
        })
    }

    /// The STRING value of `expr` as an `[n x i8]` array value (for the paths that
    /// store or pass values: FB inputs, FUNCTION arguments, struct fields).
    pub(super) fn compile_string_value(
        &mut self,
        expr: &Expression,
        function: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>, CodegenError> {
        let Some((ptr, bytes)) = self.compile_string_ptr(expr, function)? else {
            return Ok(None);
        };
        let ty = self.context.i8_type().array_type(bytes);
        Ok(Some(
            self.builder
                .build_load(ty, ptr, "strval")
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
        ))
    }

    /// `dest := src` for STRINGs: at most `dest_bytes - 1` characters, always
    /// terminated.
    pub(super) fn emit_string_copy(
        &mut self,
        dest: PointerValue<'ctx>,
        dest_bytes: u32,
        src: PointerValue<'ctx>,
    ) -> Result<(), CodegenError> {
        let f = self.get_or_create_strlcpy_fn()?;
        self.builder
            .build_call(
                f,
                &[
                    dest.into(),
                    src.into(),
                    self.context.i32_type().const_int(dest_bytes as u64, false).into(),
                ],
                "",
            )
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        Ok(())
    }

    /// `target := <string expression>` for a STRING target. Returns false when
    /// either side is not a STRING.
    pub(super) fn try_compile_string_store(
        &mut self,
        target: &Expression,
        value: &Expression,
        function: FunctionValue<'ctx>,
    ) -> Result<bool, CodegenError> {
        let Some(dest_bytes) = self.lvalue_iec_type(target).as_ref().and_then(Self::string_bytes)
        else {
            return Ok(false);
        };
        // A plain variable-to-variable copy keeps the ordinary value path.
        if self.string_expr_type(value).is_none()
            && !matches!(value.kind, ExpressionKind::FunctionCall { .. })
        {
            return Ok(false);
        }
        let Some((src, _)) = self.compile_string_ptr(value, function)? else {
            return Ok(false);
        };
        let dest = self.compile_lvalue_with_fn(target, function)?.ok_or_else(|| {
            CodegenError::UnsupportedType(format!(
                "`{}` is not an assignable location",
                Self::describe_lvalue(target)
            ))
        })?;
        self.emit_string_copy(dest, dest_bytes, src)?;
        Ok(true)
    }

    /// Integer argument of a string builtin, as i32.
    fn string_count_arg(
        &mut self,
        name: &str,
        args: &[CallArg],
        i: usize,
        function: FunctionValue<'ctx>,
    ) -> Result<inkwell::values::IntValue<'ctx>, CodegenError> {
        let a = &args[i].value;
        let v = self
            .compile_expression(a, function)?
            .ok_or_else(|| self.no_value_error(format!("argument {} of `{name}`", i + 1), a))?;
        let v = match v {
            BasicValueEnum::FloatValue(f) => self.float_to_int(f, &IecType::Dint, true)?.into(),
            v => v,
        };
        let iv = self.int_operand(v, name)?;
        let signed = !Self::widens_unsigned_opt(self.rvalue_iec_type(a).as_ref());
        self.resize_int(iv, self.context.i32_type(), signed)
    }

    /// Evaluate the string builtin `name(args)` into `buf` (capacity `bytes`).
    fn emit_string_builtin(
        &mut self,
        name: &str,
        args: &[CallArg],
        buf: PointerValue<'ctx>,
        bytes: u32,
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        let err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
        let arity = |n: usize| -> Result<(), CodegenError> {
            if args.len() == n {
                Ok(())
            } else {
                Err(CodegenError::ArgumentBinding {
                    callee: name.to_string(),
                    problem: format!("takes {n} arguments, got {}", args.len()),
                })
            }
        };
        let i32t = self.context.i32_type();
        let cap = i32t.const_int(bytes as u64, false);
        match name {
            "CONCAT" => {
                if args.len() < 2 {
                    return Err(CodegenError::ArgumentBinding {
                        callee: name.to_string(),
                        problem: format!("takes 2 or more arguments, got {}", args.len()),
                    });
                }
                // Append each argument in turn: plcc_strlcat(buf, src, cap).
                let first = self.string_operand(&args[0].value, "CONCAT", function)?.0;
                self.emit_string_copy(buf, bytes, first)?;
                let cat = self.get_or_create_strlcat_fn()?;
                for a in &args[1..] {
                    let src = self.string_operand(&a.value, "CONCAT", function)?.0;
                    self.builder
                        .build_call(cat, &[buf.into(), src.into(), cap.into()], "")
                        .map_err(err)?;
                }
            }
            "LEFT" | "RIGHT" => {
                arity(2)?;
                let src = self.string_operand(&args[0].value, name, function)?.0;
                let n = self.string_count_arg(name, args, 1, function)?;
                let f = if name == "LEFT" {
                    self.get_or_create_left_fn()
                } else {
                    self.get_or_create_right_fn()
                };
                self.builder
                    .build_call(f, &[buf.into(), src.into(), n.into(), cap.into()], "")
                    .map_err(err)?;
            }
            "MID" => {
                arity(3)?;
                let src = self.string_operand(&args[0].value, name, function)?.0;
                let l = self.string_count_arg(name, args, 1, function)?;
                let p = self.string_count_arg(name, args, 2, function)?;
                let f = self.get_or_create_mid_fn();
                self.builder
                    .build_call(f, &[buf.into(), src.into(), l.into(), p.into(), cap.into()], "")
                    .map_err(err)?;
            }
            // REPLACE(IN1, IN2, L, P); INSERT(IN1, IN2, P) = REPLACE(IN1, IN2, 0, P+1);
            // DELETE(IN, L, P) = REPLACE(IN, '', L, P).
            "REPLACE" | "INSERT" | "DELETE" => {
                let (in1, in2, l, p) = match name {
                    "REPLACE" => {
                        arity(4)?;
                        let in1 = self.string_operand(&args[0].value, name, function)?.0;
                        let in2 = self.string_operand(&args[1].value, name, function)?.0;
                        let l = self.string_count_arg(name, args, 2, function)?;
                        let p = self.string_count_arg(name, args, 3, function)?;
                        (in1, in2, l, p)
                    }
                    "INSERT" => {
                        arity(3)?;
                        let in1 = self.string_operand(&args[0].value, name, function)?.0;
                        let in2 = self.string_operand(&args[1].value, name, function)?.0;
                        let p = self.string_count_arg(name, args, 2, function)?;
                        let p = self
                            .builder
                            .build_int_add(p, i32t.const_int(1, false), "p1")
                            .map_err(err)?;
                        (in1, in2, i32t.const_zero(), p)
                    }
                    _ => {
                        arity(3)?;
                        let in1 = self.string_operand(&args[0].value, name, function)?.0;
                        let l = self.string_count_arg(name, args, 1, function)?;
                        let p = self.string_count_arg(name, args, 2, function)?;
                        (in1, self.string_literal_global(""), l, p)
                    }
                };
                let f = self.get_or_create_replace_fn()?;
                self.builder
                    .build_call(
                        f,
                        &[buf.into(), in1.into(), in2.into(), l.into(), p.into(), cap.into()],
                        "",
                    )
                    .map_err(err)?;
            }
            _ => {
                return Err(CodegenError::LlvmError(format!(
                    "internal: {name} is not a string builtin"
                )));
            }
        }
        Ok(())
    }

    /// `a <op> b` for two STRINGs: byte-wise (unsigned) lexicographic comparison.
    pub(super) fn compile_string_compare(
        &mut self,
        op: BinaryOp,
        left: &Expression,
        right: &Expression,
        function: FunctionValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
        let (a, _) = self.string_operand(left, "a STRING comparison", function)?;
        let (b, _) = self.string_operand(right, "a STRING comparison", function)?;
        let f = self.get_or_create_strcmp_fn()?;
        let r = match self
            .builder
            .build_call(f, &[a.into(), b.into()], "strcmp")
            .map_err(err)?
            .try_as_basic_value()
        {
            inkwell::values::ValueKind::Basic(v) => v.into_int_value(),
            inkwell::values::ValueKind::Instruction(_) => {
                return Err(CodegenError::LlvmError("plcc_strcmp returned no value".into()));
            }
        };
        let pred = match op {
            BinaryOp::Equal => IntPredicate::EQ,
            BinaryOp::NotEqual => IntPredicate::NE,
            BinaryOp::Less => IntPredicate::SLT,
            BinaryOp::LessEqual => IntPredicate::SLE,
            BinaryOp::Greater => IntPredicate::SGT,
            BinaryOp::GreaterEqual => IntPredicate::SGE,
            _ => {
                return Err(CodegenError::UnsupportedType(format!(
                    "operator {op:?} is not defined on STRING"
                )));
            }
        };
        Ok(self
            .builder
            .build_int_compare(pred, r, r.get_type().const_zero(), "strcmp_res")
            .map_err(err)?
            .into())
    }

    /// Declare an internal helper, or return it if it exists.
    fn string_helper(
        &self,
        name: &str,
        ty: inkwell::types::FunctionType<'ctx>,
    ) -> (FunctionValue<'ctx>, bool) {
        if let Some(f) = self.module.get_function(name) {
            return (f, false);
        }
        let f = self
            .module
            .add_function(name, ty, Some(inkwell::module::Linkage::Internal));
        (f, true)
    }

    /// `plcc_strlcpy(dest, src, cap)`: copy at most cap-1 bytes, terminate.
    fn get_or_create_strlcpy_fn(&self) -> Result<FunctionValue<'ctx>, CodegenError> {
        let ptr = self.context.ptr_type(AddressSpace::default());
        let i32t = self.context.i32_type();
        let ty = self
            .context
            .void_type()
            .fn_type(&[ptr.into(), ptr.into(), i32t.into()], false);
        let (f, new) = self.string_helper("plcc_strlcpy", ty);
        if new {
            self.build_strlcat_body(f, false)?;
        }
        Ok(f)
    }

    /// `plcc_strlcat(dest, src, cap)`: append src to dest within cap bytes.
    fn get_or_create_strlcat_fn(&self) -> Result<FunctionValue<'ctx>, CodegenError> {
        let ptr = self.context.ptr_type(AddressSpace::default());
        let i32t = self.context.i32_type();
        let ty = self
            .context
            .void_type()
            .fn_type(&[ptr.into(), ptr.into(), i32t.into()], false);
        let (f, new) = self.string_helper("plcc_strlcat", ty);
        if new {
            self.build_strlcat_body(f, true)?;
        }
        Ok(f)
    }

    fn build_strlcat_body(&self, f: FunctionValue<'ctx>, append: bool) -> Result<(), CodegenError> {
        let err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
        let saved = self.builder.get_insert_block();
        let i8t = self.context.i8_type();
        let i32t = self.context.i32_type();
        let i64t = self.context.i64_type();
        let param = |n: u32| {
            f.get_nth_param(n)
                .ok_or_else(|| CodegenError::LlvmError("string helper: missing param".into()))
        };
        let dest = param(0)?.into_pointer_value();
        let src = param(1)?.into_pointer_value();
        let cap = param(2)?.into_int_value();
        let entry = self.context.append_basic_block(f, "entry");
        self.builder.position_at_end(entry);
        let k_ptr = self.builder.build_alloca(i32t, "k").map_err(err)?;
        let i_ptr = self.builder.build_alloca(i32t, "i").map_err(err)?;
        let start = if append {
            let strlen = self.get_or_create_strlen_fn();
            let n = match self
                .builder
                .build_call(strlen, &[dest.into()], "n")
                .map_err(err)?
                .try_as_basic_value()
            {
                inkwell::values::ValueKind::Basic(v) => v.into_int_value(),
                _ => return Err(CodegenError::LlvmError("plcc_strlen returned no value".into())),
            };
            self.builder.build_int_z_extend(n, i32t, "n32").map_err(err)?
        } else {
            i32t.const_zero()
        };
        self.builder.build_store(k_ptr, start).map_err(err)?;
        let limit = self
            .builder
            .build_int_sub(cap, i32t.const_int(1, false), "limit")
            .map_err(err)?;
        // `src == dest` (`s := s`) copies each byte onto itself: harmless.
        self.emit_bounded_copy(
            f,
            dest,
            k_ptr,
            i_ptr,
            src,
            i32t.const_zero(),
            i32t.const_int(i32::MAX as u64, false),
            limit,
        )?;
        let k = self.builder.build_load(i32t, k_ptr, "k").map_err(err)?.into_int_value();
        let k64 = self.builder.build_int_s_extend(k, i64t, "k64").map_err(err)?;
        let nul = unsafe { self.builder.build_in_bounds_gep(i8t, dest, &[k64], "nul") }.map_err(err)?;
        self.builder.build_store(nul, i8t.const_zero()).map_err(err)?;
        self.builder.build_return(None).map_err(err)?;
        if let Some(bb) = saved {
            self.builder.position_at_end(bb);
        }
        Ok(())
    }

    /// `plcc_strcmp(a, b) -> i32`: <0, 0, >0 by unsigned byte comparison.
    fn get_or_create_strcmp_fn(&self) -> Result<FunctionValue<'ctx>, CodegenError> {
        let err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
        let ptr = self.context.ptr_type(AddressSpace::default());
        let i8t = self.context.i8_type();
        let i32t = self.context.i32_type();
        let i64t = self.context.i64_type();
        let ty = i32t.fn_type(&[ptr.into(), ptr.into()], false);
        let (f, new) = self.string_helper("plcc_strcmp", ty);
        if !new {
            return Ok(f);
        }
        let saved = self.builder.get_insert_block();
        let param = |n: u32| {
            f.get_nth_param(n)
                .ok_or_else(|| CodegenError::LlvmError("plcc_strcmp: missing param".into()))
        };
        let a = param(0)?.into_pointer_value();
        let b = param(1)?.into_pointer_value();
        let entry = self.context.append_basic_block(f, "entry");
        let head = self.context.append_basic_block(f, "head");
        let next = self.context.append_basic_block(f, "next");
        let done = self.context.append_basic_block(f, "done");
        self.builder.position_at_end(entry);
        let i_ptr = self.builder.build_alloca(i64t, "i").map_err(err)?;
        self.builder.build_store(i_ptr, i64t.const_zero()).map_err(err)?;
        self.builder.build_unconditional_branch(head).map_err(err)?;

        self.builder.position_at_end(head);
        let i = self.builder.build_load(i64t, i_ptr, "i").map_err(err)?.into_int_value();
        let pa = unsafe { self.builder.build_in_bounds_gep(i8t, a, &[i], "pa") }.map_err(err)?;
        let pb = unsafe { self.builder.build_in_bounds_gep(i8t, b, &[i], "pb") }.map_err(err)?;
        let ca = self.builder.build_load(i8t, pa, "ca").map_err(err)?.into_int_value();
        let cb = self.builder.build_load(i8t, pb, "cb").map_err(err)?.into_int_value();
        let ca = self.builder.build_int_z_extend(ca, i32t, "ca32").map_err(err)?;
        let cb = self.builder.build_int_z_extend(cb, i32t, "cb32").map_err(err)?;
        let diff = self.builder.build_int_sub(ca, cb, "diff").map_err(err)?;
        let ne = self
            .builder
            .build_int_compare(IntPredicate::NE, diff, i32t.const_zero(), "ne")
            .map_err(err)?;
        let end = self
            .builder
            .build_int_compare(IntPredicate::EQ, ca, i32t.const_zero(), "end")
            .map_err(err)?;
        let stop = self.builder.build_or(ne, end, "stop").map_err(err)?;
        self.builder.build_conditional_branch(stop, done, next).map_err(err)?;

        self.builder.position_at_end(next);
        let i1 = self
            .builder
            .build_int_add(i, i64t.const_int(1, false), "i1")
            .map_err(err)?;
        self.builder.build_store(i_ptr, i1).map_err(err)?;
        self.builder.build_unconditional_branch(head).map_err(err)?;

        self.builder.position_at_end(done);
        self.builder.build_return(Some(&diff)).map_err(err)?;
        if let Some(bb) = saved {
            self.builder.position_at_end(bb);
        }
        Ok(f)
    }
}
