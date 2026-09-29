// SPDX-License-Identifier: MPL-2.0

//! CODESYS `AND_THEN` / `OR_ELSE`: BOOL operators whose right operand is only
//! evaluated when the left one does not already decide the result, so
//! `p <> 0 AND_THEN p^.x > 0` never dereferences a null pointer.

use super::*;

impl<'ctx> Compiler<'ctx> {
    pub(super) fn compile_short_circuit(
        &mut self,
        op: BinaryOp,
        left: &Expression,
        right: &Expression,
        function: FunctionValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
        let l = self
            .compile_expression(left, function)?
            .ok_or_else(|| self.no_value_error("operand", left))?;
        let l = self.to_i1(self.int_operand(l, "an AND_THEN/OR_ELSE operand")?)?;
        let left_end = self
            .builder
            .get_insert_block()
            .ok_or_else(|| CodegenError::LlvmError("no insert block".into()))?;
        let rhs_bb = self.context.append_basic_block(function, "sc_rhs");
        let join_bb = self.context.append_basic_block(function, "sc_join");
        // AND_THEN evaluates the right side when the left is TRUE, OR_ELSE when
        // it is FALSE; otherwise the left value is the result.
        if op == BinaryOp::AndThen {
            self.builder
                .build_conditional_branch(l, rhs_bb, join_bb)
                .map_err(err)?;
        } else {
            self.builder
                .build_conditional_branch(l, join_bb, rhs_bb)
                .map_err(err)?;
        }
        self.builder.position_at_end(rhs_bb);
        let r = self
            .compile_expression(right, function)?
            .ok_or_else(|| self.no_value_error("operand", right))?;
        let r = self.to_i1(self.int_operand(r, "an AND_THEN/OR_ELSE operand")?)?;
        let rhs_end = self
            .builder
            .get_insert_block()
            .ok_or_else(|| CodegenError::LlvmError("no insert block".into()))?;
        self.builder
            .build_unconditional_branch(join_bb)
            .map_err(err)?;
        self.builder.position_at_end(join_bb);
        let phi = self
            .builder
            .build_phi(self.context.bool_type(), "sc")
            .map_err(err)?;
        phi.add_incoming(&[(&l, left_end), (&r, rhs_end)]);
        Ok(phi.as_basic_value())
    }
}
