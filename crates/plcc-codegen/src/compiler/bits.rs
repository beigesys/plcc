// SPDX-License-Identifier: MPL-2.0

//! CODESYS bit access on an integer or bit-string variable: `x.3` reads bit 3 of
//! `x` as a BOOL, and `x.3 := TRUE;` sets it. Bit 0 is the least significant. The
//! index must be a literal below the width of the type (`w.15` on a WORD, `lw.63`
//! on an LWORD). The object may be any addressable expression for a write
//! (`s.w.3 := ..`, `arr[i].7 := ..`) and any value for a read.
//!
//! CODESYS help, "Bit access in variables": `<variable name>.<bit number>`; IEC
//! 61131-3 3rd ed. has the same form as a partial access (`x.%X3`, Table 16).

use super::*;

impl<'ctx> Compiler<'ctx> {
    /// `(object, bit)` when `expr` is a bit access on an integer / bit string.
    pub(super) fn bit_access<'e>(&self, expr: &'e Expression) -> Option<(&'e Expression, u32)> {
        let ExpressionKind::MemberAccess { object, member } = &expr.kind else {
            return None;
        };
        let digits = member
            .name
            .strip_prefix("%X")
            .or_else(|| member.name.strip_prefix("%x"))
            .unwrap_or(&member.name);
        if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let bit: u32 = digits.parse().ok()?;
        let ty = self
            .lvalue_iec_type(object)
            .or_else(|| self.rvalue_iec_type(object))?;
        let base = ty.base();
        (base.is_any_int() || (base.is_any_bit() && *base != IecType::Bool)).then_some((object, bit))
    }

    fn check_bit_index(
        &self,
        expr: &Expression,
        width: u32,
        bit: u32,
    ) -> Result<(), CodegenError> {
        if bit >= width {
            return Err(CodegenError::UnsupportedType(format!(
                "`{}`: bit {bit} does not exist in a {width}-bit value",
                Self::describe_lvalue(expr)
            )));
        }
        Ok(())
    }

    /// The value of a bit access, as a BOOL (i8 0/1).
    pub(super) fn compile_bit_read(
        &mut self,
        expr: &Expression,
        object: &Expression,
        bit: u32,
        function: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>, CodegenError> {
        let err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
        let Some(v) = self.compile_expression(object, function)? else {
            return Ok(None);
        };
        let iv = self.int_operand(v, "a bit access")?;
        self.check_bit_index(expr, iv.get_type().get_bit_width(), bit)?;
        let shifted = self
            .builder
            .build_right_shift(iv, iv.get_type().const_int(bit as u64, false), false, "bit")
            .map_err(err)?;
        let one = self
            .builder
            .build_int_truncate_or_bit_cast(shifted, self.context.bool_type(), "bit1")
            .map_err(err)?;
        Ok(Some(
            self.builder
                .build_int_z_extend(one, self.context.i8_type(), "bitval")
                .map_err(err)?
                .into(),
        ))
    }

    /// `x.3 := value;` — a read-modify-write of `x`. Returns false when `target` is
    /// not a bit access.
    pub(super) fn try_compile_bit_assignment(
        &mut self,
        target: &Expression,
        value: &Expression,
        function: FunctionValue<'ctx>,
    ) -> Result<bool, CodegenError> {
        let err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
        let Some((object, bit)) = self.bit_access(target) else {
            return Ok(false);
        };
        let Some(obj_ty) = self.lvalue_iec_type(object) else {
            return Err(CodegenError::UnsupportedType(format!(
                "`{}` is not an assignable location",
                Self::describe_lvalue(target)
            )));
        };
        let Some(val) = self.compile_expression(value, function)? else {
            return Err(self.no_value_error(
                format!(
                    "right-hand side of the assignment to `{}`",
                    Self::describe_lvalue(target)
                ),
                value,
            ));
        };
        let set = {
            let iv = self.int_operand(val, "a bit assignment")?;
            self.builder
                .build_int_compare(IntPredicate::NE, iv, iv.get_type().const_zero(), "bitset")
                .map_err(err)?
        };
        let ptr = self
            .compile_lvalue_with_fn(object, function)?
            .ok_or_else(|| {
                CodegenError::UnsupportedType(format!(
                    "`{}` is not an assignable location",
                    Self::describe_lvalue(object)
                ))
            })?;
        let it = self.iec_to_llvm_type(&obj_ty).into_int_type();
        self.check_bit_index(target, it.get_bit_width(), bit)?;
        let old = self.builder.build_load(it, ptr, "bits").map_err(err)?.into_int_value();
        let mask = it.const_int(1u64 << bit, false);
        let cleared = self
            .builder
            .build_and(old, mask.const_not(), "bitclr")
            .map_err(err)?;
        let setbit = self
            .builder
            .build_int_z_extend(set, it, "bitext")
            .map_err(err)?;
        let setbit = self
            .builder
            .build_left_shift(setbit, it.const_int(bit as u64, false), "bitpos")
            .map_err(err)?;
        let new = self.builder.build_or(cleared, setbit, "bitnew").map_err(err)?;
        self.builder.build_store(ptr, new).map_err(err)?;
        Ok(true)
    }
}
