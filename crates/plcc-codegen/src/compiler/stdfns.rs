// SPDX-License-Identifier: MPL-2.0

//! Standard functions that are spelled-out forms of operators or of other
//! builtins (IEC 61131-3 Tables 29–31, 36): `ADD`, `MUL`, `SUB`, `DIV`, `MOD`,
//! `AND`, `OR`, `XOR`, `NOT`, `GT`, `GE`, `EQ`, `LE`, `LT`, `NE`, `MOVE`, and the
//! extensible forms of `MIN`/`MAX`. Each is rewritten to the expression it
//! stands for, so it gets exactly the operator's typing and lowering. `MUX` is
//! lowered here too.
//!
//! These were all "unknown function".

use super::*;

impl<'ctx> Compiler<'ctx> {
    /// The expression a call to one of these functions stands for, or `None`.
    pub(super) fn desugar_std_call(&self, expr: &Expression) -> Option<Expression> {
        let ExpressionKind::FunctionCall { callee, args } = &expr.kind else {
            return None;
        };
        let ExpressionKind::Identifier(id) = &callee.kind else {
            return None;
        };
        if self.fn_signatures.contains_key(&id.name.to_lowercase())
            || args.iter().any(|a| a.name.is_some() || a.is_output)
        {
            return None;
        }
        let span = expr.span;
        let vals: Vec<Expression> = args.iter().map(|a| a.value.clone()).collect();
        let bin = |op: BinaryOp, l: Expression, r: Expression| Expression {
            kind: ExpressionKind::BinaryOp {
                op,
                left: Box::new(l),
                right: Box::new(r),
            },
            span,
        };
        let fold = |op: BinaryOp, vals: Vec<Expression>| -> Option<Expression> {
            let mut it = vals.into_iter();
            let first = it.next()?;
            Some(it.fold(first, |acc, v| bin(op, acc, v)))
        };
        let upper = id.name.to_uppercase();
        let arith = match upper.as_str() {
            "ADD" => Some((BinaryOp::Add, true)),
            "MUL" => Some((BinaryOp::Mul, true)),
            "AND" => Some((BinaryOp::And, true)),
            "OR" => Some((BinaryOp::Or, true)),
            "XOR" => Some((BinaryOp::Xor, true)),
            "SUB" => Some((BinaryOp::Sub, false)),
            "DIV" => Some((BinaryOp::Div, false)),
            "MOD" => Some((BinaryOp::Mod, false)),
            _ => None,
        };
        if let Some((op, extensible)) = arith {
            if vals.len() < 2 || (!extensible && vals.len() != 2) {
                return None;
            }
            return fold(op, vals);
        }
        // GT(a, b, c) is a > b AND b > c (IEC Table 31: a monotonic sequence).
        let cmp = match upper.as_str() {
            "GT" => Some(BinaryOp::Greater),
            "GE" => Some(BinaryOp::GreaterEqual),
            "EQ" => Some(BinaryOp::Equal),
            "LE" => Some(BinaryOp::LessEqual),
            "LT" => Some(BinaryOp::Less),
            "NE" => Some(BinaryOp::NotEqual),
            _ => None,
        };
        if let Some(op) = cmp {
            if vals.len() < 2 || (op == BinaryOp::NotEqual && vals.len() != 2) {
                return None;
            }
            let pairs: Vec<Expression> = vals
                .windows(2)
                .map(|w| bin(op, w[0].clone(), w[1].clone()))
                .collect();
            return fold(BinaryOp::And, pairs);
        }
        match upper.as_str() {
            "NOT" if vals.len() == 1 => Some(Expression {
                kind: ExpressionKind::UnaryOp {
                    op: UnaryOp::Not,
                    operand: Box::new(vals[0].clone()),
                },
                span,
            }),
            "MOVE" if vals.len() == 1 => Some(Expression {
                kind: ExpressionKind::Parenthesized(Box::new(vals[0].clone())),
                span,
            }),
            // MAX(a, b, c) = MAX(MAX(a, b), c).
            "MIN" | "MAX" if vals.len() > 2 => {
                let mut it = vals.into_iter();
                let first = it.next()?;
                Some(it.fold(first, |acc, v| Expression {
                    kind: ExpressionKind::FunctionCall {
                        callee: callee.clone(),
                        args: vec![
                            CallArg {
                                name: None,
                                value: acc,
                                is_output: false,
                                negated: false,
                                span,
                            },
                            CallArg {
                                name: None,
                                value: v,
                                is_output: false,
                                negated: false,
                                span,
                            },
                        ],
                    },
                    span,
                }))
            }
            _ => None,
        }
    }

    /// Whether `expr` is a `MUX` call this module lowers.
    pub(super) fn is_mux_call(&self, expr: &Expression) -> bool {
        let ExpressionKind::FunctionCall { callee, args } = &expr.kind else {
            return false;
        };
        matches!(&callee.kind, ExpressionKind::Identifier(id)
            if id.name.eq_ignore_ascii_case("MUX")
                && !self.fn_signatures.contains_key("mux")
                && args.len() >= 2
                && args.iter().all(|a| a.name.is_none() && !a.is_output))
    }

    /// Result type of `MUX(K, IN0, ...)`: the common type of the inputs.
    pub(super) fn mux_result_type(&self, expr: &Expression) -> Option<IecType> {
        let ExpressionKind::FunctionCall { args, .. } = &expr.kind else {
            return None;
        };
        args[1..]
            .iter()
            .map(|a| self.rvalue_iec_type(&a.value))
            .reduce(Self::arith_result_type)
            .flatten()
    }

    /// `MUX(K, IN0, IN1, ...)`: IN<K>. K is evaluated once. An out-of-range K
    /// selects IN0 (IEC leaves it an error; CODESYS's result is unspecified).
    pub(super) fn compile_mux(
        &mut self,
        expr: &Expression,
        function: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>, CodegenError> {
        let err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
        let ExpressionKind::FunctionCall { args, .. } = &expr.kind else {
            return Ok(None);
        };
        let k_expr = &args[0].value;
        let k = self
            .compile_expression(k_expr, function)?
            .ok_or_else(|| self.no_value_error("argument K of `MUX`", k_expr))?;
        let k = self.int_operand(k, "K of MUX")?;
        let k_ty = self.rvalue_iec_type(k_expr);
        let k = self.widen_to(k, Self::signedness_of(k_ty.as_ref()), self.context.i64_type())?;
        let result_ty = self.mux_result_type(expr);
        let mut values = Vec::new();
        for (i, a) in args[1..].iter().enumerate() {
            let v = self
                .compile_expression(&a.value, function)?
                .ok_or_else(|| self.no_value_error(format!("input {i} of `MUX`"), &a.value))?;
            let v = match &result_ty {
                Some(t) => {
                    let src = self.rvalue_iec_type(&a.value);
                    self.coerce_value(v, src.as_ref(), t)?
                }
                None => v,
            };
            values.push(v);
        }
        // All inputs must share one LLVM type for the select chain.
        let first_ty = values[0].get_type();
        if values.iter().any(|v| v.get_type() != first_ty) {
            return Err(CodegenError::UnsupportedType(
                "MUX: the inputs do not have a common type".into(),
            ));
        }
        let mut acc = values[0];
        for (i, v) in values.iter().enumerate().skip(1) {
            let hit = self
                .builder
                .build_int_compare(
                    IntPredicate::EQ,
                    k,
                    self.context.i64_type().const_int(i as u64, false),
                    "mux_k",
                )
                .map_err(err)?;
            acc = self.builder.build_select(hit, *v, acc, "mux").map_err(err)?;
        }
        Ok(Some(acc))
    }
}
