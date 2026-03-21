// SPDX-License-Identifier: MPL-2.0

use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::intrinsics::Intrinsic;
use inkwell::module::Module;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine, TargetTriple,
};
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum, StructType};
use inkwell::values::{BasicValue, BasicValueEnum, FunctionValue, GlobalValue, PointerValue};
use inkwell::{AddressSpace, FloatPredicate, IntPredicate, OptimizationLevel};
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;
use plcc_hir::types::{IecType, TypeRegistry};
use plcc_hir::check::TypeChecker;
use plcc_st::ast::*;

/// Parse a TIME literal string (e.g., "T#100ms", "T#1s500ms", "T#1h30m") into nanoseconds.
fn parse_time_literal_ns(s: &str) -> i64 {
    let s = s.trim();
    // Strip T# or t# prefix
    let s = if s.len() > 2 && (s.starts_with("T#") || s.starts_with("t#")) {
        &s[2..]
    } else {
        s
    };
    let mut ns: i64 = 0;
    let mut num_buf = String::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() || c == '.' || c == '_' {
            if c != '_' {
                num_buf.push(c);
            }
            chars.next();
        } else {
            let val: f64 = num_buf.parse().unwrap_or(0.0);
            num_buf.clear();
            // Read unit suffix
            let mut unit = String::new();
            while let Some(&u) = chars.peek() {
                if u.is_ascii_alphabetic() {
                    unit.push(u);
                    chars.next();
                } else {
                    break;
                }
            }
            let multiplier: f64 = match unit.to_lowercase().as_str() {
                "d" => 86_400_000_000_000.0,
                "h" => 3_600_000_000_000.0,
                "m" => 60_000_000_000.0,
                "s" => 1_000_000_000.0,
                "ms" => 1_000_000.0,
                "us" => 1_000.0,
                "ns" => 1.0,
                _ => 0.0,
            };
            ns += (val * multiplier) as i64;
        }
    }
    // Handle trailing number with no unit (assume ms for bare numbers)
    if !num_buf.is_empty() {
        let val: f64 = num_buf.parse().unwrap_or(0.0);
        ns += (val * 1_000_000.0) as i64; // default ms
    }
    ns
}

#[derive(Debug, Error)]
pub enum CodegenError {
    #[error("unsupported type: {0}")]
    UnsupportedType(String),
    #[error("undefined variable: {0}")]
    UndefinedVariable(String),
    #[error("LLVM error: {0}")]
    LlvmError(String),
    #[error("target error: {0}")]
    TargetError(String),
}

/// Layout information for a compiled function block.
#[derive(Clone, Debug)]
struct FbLayout<'ctx> {
    struct_type: StructType<'ctx>,
    scan_fn_name: String,
    /// Ordered field names and their IEC types (inputs, outputs, locals — all in declaration order).
    fields: Vec<(String, IecType)>,
}

/// Runtime info for an FB instance embedded in a parent POU's state struct.
#[derive(Clone, Debug)]
struct FbInstanceInfo<'ctx> {
    /// Index of this FB instance's sub-struct within the parent struct.
    field_index: u32,
    fb_type_name: String,
    scan_fn_name: String,
    fields: Vec<(String, IecType)>,
    struct_type: StructType<'ctx>,
}

pub struct Compiler<'ctx> {
    context: &'ctx Context,
    module: Module<'ctx>,
    builder: Builder<'ctx>,
    variables: HashMap<String, (PointerValue<'ctx>, IecType)>,
    type_registry: TypeRegistry,
    type_checker: TypeChecker,
    /// Target block for EXIT statements inside loops.
    loop_exit_bb: Option<inkwell::basic_block::BasicBlock<'ctx>>,
    /// Global variables: (global LLVM value, struct type, field names+types).
    global_var: Option<(GlobalValue<'ctx>, StructType<'ctx>, Vec<(String, IecType)>)>,
    /// Compiled FB layouts, keyed by uppercase FB type name.
    compiled_fbs: HashMap<String, FbLayout<'ctx>>,
    /// FB instances in the current POU being compiled, keyed by uppercase instance name.
    fb_instances: HashMap<String, FbInstanceInfo<'ctx>>,
    /// The parent struct type for the current POU being compiled (needed for GEP on FB instances).
    current_struct_type: Option<StructType<'ctx>>,
    /// The state pointer for the current POU being compiled.
    current_state_ptr: Option<PointerValue<'ctx>>,
}

impl<'ctx> Compiler<'ctx> {
    pub fn new(context: &'ctx Context, module_name: &str) -> Self {
        let module = context.create_module(module_name);
        let builder = context.create_builder();
        Self {
            context,
            module,
            builder,
            variables: HashMap::new(),
            type_registry: TypeRegistry::new(),
            type_checker: TypeChecker::new(),
            loop_exit_bb: None,
            global_var: None,
            compiled_fbs: HashMap::new(),
            fb_instances: HashMap::new(),
            current_struct_type: None,
            current_state_ptr: None,
        }
    }

    /// Register LLVM intrinsic declarations for standard math functions.
    fn register_standard_functions(&self) {
        let f32_ty: BasicTypeEnum = self.context.f32_type().into();
        let f64_ty: BasicTypeEnum = self.context.f64_type().into();

        let intrinsics_f32_f64 = [
            "llvm.fabs",
            "llvm.sqrt",
            "llvm.sin",
            "llvm.cos",
            "llvm.exp",
            "llvm.log",
            "llvm.pow",
            "llvm.floor",
            "llvm.ceil",
            "llvm.trunc",
        ];

        for name in &intrinsics_f32_f64 {
            if let Some(intr) = Intrinsic::find(name) {
                intr.get_declaration(&self.module, &[f32_ty]);
                intr.get_declaration(&self.module, &[f64_ty]);
            }
        }

        let i16_ty: BasicTypeEnum = self.context.i16_type().into();
        let i32_ty: BasicTypeEnum = self.context.i32_type().into();
        for name in &["llvm.fshl", "llvm.fshr"] {
            if let Some(intr) = Intrinsic::find(name) {
                intr.get_declaration(&self.module, &[i16_ty]);
                intr.get_declaration(&self.module, &[i32_ty]);
            }
        }
    }

    /// Try to compile a call to a standard library function.
    /// Returns `Ok(Some(val))` if handled, `Ok(None)` if not a known stdlib function.
    fn compile_stdlib_call(
        &mut self,
        name: &str,
        args: &[CallArg],
        function: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>, CodegenError> {
        let uname = name.to_uppercase();

        let mut arg_vals: Vec<BasicValueEnum<'ctx>> = Vec::new();
        for arg in args {
            if let Some(val) = self.compile_expression(&arg.value, function)? {
                arg_vals.push(val);
            } else {
                return Err(CodegenError::LlvmError(format!(
                    "failed to compile argument for {uname}"
                )));
            }
        }

        match uname.as_str() {
            "SQRT" | "SIN" | "COS" | "EXP" | "LN" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(format!(
                        "{uname} expects 1 argument, got {}",
                        arg_vals.len()
                    )));
                }
                let arg = self.ensure_float(arg_vals[0])?;
                let intrinsic_name = match uname.as_str() {
                    "SQRT" => "llvm.sqrt",
                    "SIN" => "llvm.sin",
                    "COS" => "llvm.cos",
                    "EXP" => "llvm.exp",
                    "LN" => "llvm.log",
                    _ => unreachable!(),
                };
                let fty = arg.get_type();
                let intr = Intrinsic::find(intrinsic_name).ok_or_else(|| {
                    CodegenError::LlvmError(format!("intrinsic {intrinsic_name} not found"))
                })?;
                let fn_val = intr
                    .get_declaration(&self.module, &[fty.into()])
                    .ok_or_else(|| {
                        CodegenError::LlvmError(format!(
                            "failed to get declaration for {intrinsic_name}"
                        ))
                    })?;
                let result = self
                    .builder
                    .build_call(fn_val, &[arg.into()], &uname.to_lowercase())
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                    .try_as_basic_value();
                let result = match result {
                    inkwell::values::ValueKind::Basic(v) => v,
                    _ => return Err(CodegenError::LlvmError("expected return value from intrinsic".into())),
                };
                Ok(Some(result))
            }

            "EXPT" => {
                if arg_vals.len() != 2 {
                    return Err(CodegenError::LlvmError(format!(
                        "EXPT expects 2 arguments, got {}",
                        arg_vals.len()
                    )));
                }
                let base = self.ensure_float(arg_vals[0])?;
                let exp_val = self.ensure_float(arg_vals[1])?;
                let (base, exp_val) = self.match_float_widths(base, exp_val)?;
                let fty = base.get_type();
                let intr = Intrinsic::find("llvm.pow").ok_or_else(|| {
                    CodegenError::LlvmError("intrinsic llvm.pow not found".into())
                })?;
                let fn_val = intr
                    .get_declaration(&self.module, &[fty.into()])
                    .ok_or_else(|| {
                        CodegenError::LlvmError("failed to get llvm.pow declaration".into())
                    })?;
                let result = self
                    .builder
                    .build_call(fn_val, &[base.into(), exp_val.into()], "expt")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                    .try_as_basic_value();
                let result = match result {
                    inkwell::values::ValueKind::Basic(v) => v,
                    _ => return Err(CodegenError::LlvmError("expected return value from intrinsic".into())),
                };
                Ok(Some(result))
            }

            "ABS" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(format!(
                        "ABS expects 1 argument, got {}",
                        arg_vals.len()
                    )));
                }
                let arg = arg_vals[0];
                if arg.is_float_value() {
                    let fv = arg.into_float_value();
                    let fty = fv.get_type();
                    let intr = Intrinsic::find("llvm.fabs").unwrap();
                    let fn_val = intr.get_declaration(&self.module, &[fty.into()]).unwrap();
                    let call_result = self
                        .builder
                        .build_call(fn_val, &[fv.into()], "fabs")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .try_as_basic_value();
                    let result = match call_result {
                        inkwell::values::ValueKind::Basic(v) => v,
                        _ => return Err(CodegenError::LlvmError("expected return value from fabs intrinsic".into())),
                    };
                    Ok(Some(result))
                } else {
                    let iv = arg.into_int_value();
                    let zero = iv.get_type().const_zero();
                    let is_neg = self
                        .builder
                        .build_int_compare(IntPredicate::SLT, iv, zero, "is_neg")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    let neg_val = self
                        .builder
                        .build_int_neg(iv, "neg")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    let result = self
                        .builder
                        .build_select(is_neg, neg_val, iv, "abs")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    Ok(Some(result.into()))
                }
            }

            "MIN" | "MAX" => {
                if arg_vals.len() != 2 {
                    return Err(CodegenError::LlvmError(format!(
                        "{uname} expects 2 arguments, got {}",
                        arg_vals.len()
                    )));
                }
                let a = arg_vals[0];
                let b = arg_vals[1];
                let is_max = uname == "MAX";

                if a.is_float_value() || b.is_float_value() {
                    let fa = self.ensure_float(a)?;
                    let fb = self.ensure_float(b)?;
                    let (fa, fb) = self.match_float_widths(fa, fb)?;
                    let pred = if is_max {
                        FloatPredicate::OGT
                    } else {
                        FloatPredicate::OLT
                    };
                    let cmp = self
                        .builder
                        .build_float_compare(pred, fa, fb, "cmp")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    let result = self
                        .builder
                        .build_select(cmp, fa, fb, &uname.to_lowercase())
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    Ok(Some(result.into()))
                } else {
                    let ia = a.into_int_value();
                    let ib = b.into_int_value();
                    let (ia, ib) = self.match_int_widths(ia, ib)?;
                    let pred = if is_max {
                        IntPredicate::SGT
                    } else {
                        IntPredicate::SLT
                    };
                    let cmp = self
                        .builder
                        .build_int_compare(pred, ia, ib, "cmp")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    let result = self
                        .builder
                        .build_select(cmp, ia, ib, &uname.to_lowercase())
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    Ok(Some(result.into()))
                }
            }

            "LIMIT" => {
                if arg_vals.len() != 3 {
                    return Err(CodegenError::LlvmError(format!(
                        "LIMIT expects 3 arguments, got {}",
                        arg_vals.len()
                    )));
                }
                let mn = arg_vals[0];
                let val = arg_vals[1];
                let mx = arg_vals[2];

                if val.is_float_value() || mn.is_float_value() || mx.is_float_value() {
                    let fmn = self.ensure_float(mn)?;
                    let fval = self.ensure_float(val)?;
                    let fmx = self.ensure_float(mx)?;
                    let cmp_hi = self
                        .builder
                        .build_float_compare(FloatPredicate::OLT, fval, fmx, "cmp_hi")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    let clamped_hi = self
                        .builder
                        .build_select(cmp_hi, fval, fmx, "clamp_hi")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into_float_value();
                    let cmp_lo = self
                        .builder
                        .build_float_compare(FloatPredicate::OGT, clamped_hi, fmn, "cmp_lo")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    let result = self
                        .builder
                        .build_select(cmp_lo, clamped_hi, fmn, "limit")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    Ok(Some(result.into()))
                } else {
                    let imn = mn.into_int_value();
                    let ival = val.into_int_value();
                    let imx = mx.into_int_value();
                    let (ival, imx) = self.match_int_widths(ival, imx)?;
                    let (ival, imn) = self.match_int_widths(ival, imn)?;
                    let (imx, imn) = self.match_int_widths(imx, imn)?;
                    let cmp_hi = self
                        .builder
                        .build_int_compare(IntPredicate::SLT, ival, imx, "cmp_hi")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    let clamped_hi = self
                        .builder
                        .build_select(cmp_hi, ival, imx, "clamp_hi")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into_int_value();
                    let cmp_lo = self
                        .builder
                        .build_int_compare(IntPredicate::SGT, clamped_hi, imn, "cmp_lo")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    let result = self
                        .builder
                        .build_select(cmp_lo, clamped_hi, imn, "limit")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    Ok(Some(result.into()))
                }
            }

            "SEL" => {
                if arg_vals.len() != 3 {
                    return Err(CodegenError::LlvmError(format!(
                        "SEL expects 3 arguments, got {}",
                        arg_vals.len()
                    )));
                }
                let g = arg_vals[0].into_int_value();
                let in0 = arg_vals[1];
                let in1 = arg_vals[2];
                let cond = self.to_i1(g)?;
                let result = self
                    .builder
                    .build_select(cond, in1, in0, "sel")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(Some(result.into()))
            }

            "SHL" => {
                if arg_vals.len() != 2 {
                    return Err(CodegenError::LlvmError(format!(
                        "SHL expects 2 arguments, got {}",
                        arg_vals.len()
                    )));
                }
                let val = arg_vals[0].into_int_value();
                let n = arg_vals[1].into_int_value();
                let (val, n) = self.match_int_widths(val, n)?;
                let result = self
                    .builder
                    .build_left_shift(val, n, "shl")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(Some(result.into()))
            }
            "SHR" => {
                if arg_vals.len() != 2 {
                    return Err(CodegenError::LlvmError(format!(
                        "SHR expects 2 arguments, got {}",
                        arg_vals.len()
                    )));
                }
                let val = arg_vals[0].into_int_value();
                let n = arg_vals[1].into_int_value();
                let (val, n) = self.match_int_widths(val, n)?;
                let result = self
                    .builder
                    .build_right_shift(val, n, false, "shr")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(Some(result.into()))
            }

            "ROL" => {
                if arg_vals.len() != 2 {
                    return Err(CodegenError::LlvmError(format!(
                        "ROL expects 2 arguments, got {}",
                        arg_vals.len()
                    )));
                }
                let val = arg_vals[0].into_int_value();
                let n = arg_vals[1].into_int_value();
                let (val, n) = self.match_int_widths(val, n)?;
                let ity: BasicTypeEnum = val.get_type().into();
                let intr = Intrinsic::find("llvm.fshl").ok_or_else(|| {
                    CodegenError::LlvmError("intrinsic llvm.fshl not found".into())
                })?;
                let fn_val = intr.get_declaration(&self.module, &[ity]).ok_or_else(|| {
                    CodegenError::LlvmError("failed to get llvm.fshl declaration".into())
                })?;
                let result = self
                    .builder
                    .build_call(fn_val, &[val.into(), val.into(), n.into()], "rol")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                    .try_as_basic_value();
                let result = match result {
                    inkwell::values::ValueKind::Basic(v) => v,
                    _ => return Err(CodegenError::LlvmError("expected return value from intrinsic".into())),
                };
                Ok(Some(result))
            }
            "ROR" => {
                if arg_vals.len() != 2 {
                    return Err(CodegenError::LlvmError(format!(
                        "ROR expects 2 arguments, got {}",
                        arg_vals.len()
                    )));
                }
                let val = arg_vals[0].into_int_value();
                let n = arg_vals[1].into_int_value();
                let (val, n) = self.match_int_widths(val, n)?;
                let ity: BasicTypeEnum = val.get_type().into();
                let intr = Intrinsic::find("llvm.fshr").ok_or_else(|| {
                    CodegenError::LlvmError("intrinsic llvm.fshr not found".into())
                })?;
                let fn_val = intr.get_declaration(&self.module, &[ity]).ok_or_else(|| {
                    CodegenError::LlvmError("failed to get llvm.fshr declaration".into())
                })?;
                let result = self
                    .builder
                    .build_call(fn_val, &[val.into(), val.into(), n.into()], "ror")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                    .try_as_basic_value();
                let result = match result {
                    inkwell::values::ValueKind::Basic(v) => v,
                    _ => return Err(CodegenError::LlvmError("expected return value from intrinsic".into())),
                };
                Ok(Some(result))
            }

            "INT_TO_REAL" | "DINT_TO_REAL" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(format!(
                        "{uname} expects 1 argument"
                    )));
                }
                let iv = arg_vals[0].into_int_value();
                let result = self
                    .builder
                    .build_signed_int_to_float(iv, self.context.f32_type(), "to_real")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(Some(result.into()))
            }
            "INT_TO_LREAL" | "DINT_TO_LREAL" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(format!(
                        "{uname} expects 1 argument"
                    )));
                }
                let iv = arg_vals[0].into_int_value();
                let result = self
                    .builder
                    .build_signed_int_to_float(iv, self.context.f64_type(), "to_lreal")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(Some(result.into()))
            }
            "REAL_TO_INT" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(
                        "REAL_TO_INT expects 1 argument".into(),
                    ));
                }
                let fv = arg_vals[0].into_float_value();
                let result = self
                    .builder
                    .build_float_to_signed_int(fv, self.context.i16_type(), "real_to_int")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(Some(result.into()))
            }
            "REAL_TO_DINT" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(
                        "REAL_TO_DINT expects 1 argument".into(),
                    ));
                }
                let fv = arg_vals[0].into_float_value();
                let result = self
                    .builder
                    .build_float_to_signed_int(fv, self.context.i32_type(), "real_to_dint")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(Some(result.into()))
            }
            "INT_TO_DINT" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(
                        "INT_TO_DINT expects 1 argument".into(),
                    ));
                }
                let iv = arg_vals[0].into_int_value();
                let result = self
                    .builder
                    .build_int_s_extend(iv, self.context.i32_type(), "int_to_dint")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(Some(result.into()))
            }
            "DINT_TO_INT" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(
                        "DINT_TO_INT expects 1 argument".into(),
                    ));
                }
                let iv = arg_vals[0].into_int_value();
                let result = self
                    .builder
                    .build_int_truncate(iv, self.context.i16_type(), "dint_to_int")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(Some(result.into()))
            }
            "BOOL_TO_INT" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(
                        "BOOL_TO_INT expects 1 argument".into(),
                    ));
                }
                let iv = arg_vals[0].into_int_value();
                let result = self
                    .builder
                    .build_int_z_extend(iv, self.context.i16_type(), "bool_to_int")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(Some(result.into()))
            }
            "TRUNC" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(
                        "TRUNC expects 1 argument".into(),
                    ));
                }
                let fv = self.ensure_float(arg_vals[0])?;
                let fty = fv.get_type();
                let intr = Intrinsic::find("llvm.trunc").ok_or_else(|| {
                    CodegenError::LlvmError("intrinsic llvm.trunc not found".into())
                })?;
                let fn_val =
                    intr.get_declaration(&self.module, &[fty.into()])
                        .ok_or_else(|| {
                            CodegenError::LlvmError(
                                "failed to get llvm.trunc declaration".into(),
                            )
                        })?;
                let result = self
                    .builder
                    .build_call(fn_val, &[fv.into()], "trunc")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                    .try_as_basic_value();
                let result = match result {
                    inkwell::values::ValueKind::Basic(v) => v,
                    _ => return Err(CodegenError::LlvmError("expected return value from intrinsic".into())),
                };
                Ok(Some(result))
            }

            "LEN" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(
                        "LEN expects 1 argument".into(),
                    ));
                }
                // LEN needs the pointer to the string array, not the loaded value.
                // Re-evaluate the argument as an lvalue to get the pointer.
                if let Some(str_ptr) = self.compile_lvalue_with_fn(&args[0].value, function)? {
                    let strlen_fn = self.get_or_create_strlen_fn();
                    let result = self
                        .builder
                        .build_call(strlen_fn, &[str_ptr.into()], "len_result")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .try_as_basic_value();
                    match result {
                        inkwell::values::ValueKind::Basic(v) => Ok(Some(v)),
                        _ => Err(CodegenError::LlvmError("LEN: expected return value".into())),
                    }
                } else {
                    Ok(Some(self.context.i16_type().const_zero().into()))
                }
            }

            _ => Ok(None),
        }
    }

    /// Get or create a `plcc_strlen` helper function that counts non-null bytes.
    /// Signature: i16 plcc_strlen(ptr) -- scans bytes until null, returns count as i16.
    fn get_or_create_strlen_fn(&self) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function("plcc_strlen") {
            return f;
        }

        let i16_ty = self.context.i16_type();
        let i8_ty = self.context.i8_type();
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let fn_type = i16_ty.fn_type(&[ptr_ty.into()], false);
        let function = self.module.add_function("plcc_strlen", fn_type, None);

        let saved_block = self.builder.get_insert_block();

        let entry = self.context.append_basic_block(function, "entry");
        let loop_bb = self.context.append_basic_block(function, "loop");
        let inc_bb = self.context.append_basic_block(function, "inc");
        let done_bb = self.context.append_basic_block(function, "done");

        self.builder.position_at_end(entry);
        let counter = self.builder.build_alloca(i16_ty, "counter").unwrap();
        self.builder.build_store(counter, i16_ty.const_zero()).unwrap();
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        self.builder.position_at_end(loop_bb);
        let str_ptr = function.get_nth_param(0).unwrap().into_pointer_value();
        let idx = self.builder.build_load(i16_ty, counter, "idx").unwrap().into_int_value();
        let idx_i64 = self.builder.build_int_s_extend(idx, self.context.i64_type(), "idx64").unwrap();
        let char_ptr = unsafe {
            self.builder.build_in_bounds_gep(i8_ty, str_ptr, &[idx_i64], "char_ptr").unwrap()
        };
        let ch = self.builder.build_load(i8_ty, char_ptr, "ch").unwrap().into_int_value();
        let is_null = self.builder.build_int_compare(IntPredicate::EQ, ch, i8_ty.const_zero(), "is_null").unwrap();
        self.builder.build_conditional_branch(is_null, done_bb, inc_bb).unwrap();

        self.builder.position_at_end(inc_bb);
        let cur = self.builder.build_load(i16_ty, counter, "cur").unwrap().into_int_value();
        let next = self.builder.build_int_add(cur, i16_ty.const_int(1, false), "next").unwrap();
        self.builder.build_store(counter, next).unwrap();
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        self.builder.position_at_end(done_bb);
        let result = self.builder.build_load(i16_ty, counter, "result").unwrap();
        self.builder.build_return(Some(&result)).unwrap();

        if let Some(bb) = saved_block {
            self.builder.position_at_end(bb);
        }

        function
    }

    /// Convert a value to float if it's an integer (int -> f32).
    fn ensure_float(
        &self,
        val: BasicValueEnum<'ctx>,
    ) -> Result<inkwell::values::FloatValue<'ctx>, CodegenError> {
        if val.is_float_value() {
            Ok(val.into_float_value())
        } else {
            self.builder
                .build_signed_int_to_float(
                    val.into_int_value(),
                    self.context.f32_type(),
                    "itof",
                )
                .map_err(|e| CodegenError::LlvmError(e.to_string()))
        }
    }

    /// Match two float values to the same (wider) type.
    fn match_float_widths(
        &self,
        a: inkwell::values::FloatValue<'ctx>,
        b: inkwell::values::FloatValue<'ctx>,
    ) -> Result<
        (
            inkwell::values::FloatValue<'ctx>,
            inkwell::values::FloatValue<'ctx>,
        ),
        CodegenError,
    > {
        let aty = a.get_type();
        let bty = b.get_type();
        if aty == bty {
            Ok((a, b))
        } else if aty == self.context.f64_type() {
            let b_ext = self
                .builder
                .build_float_ext(b, self.context.f64_type(), "fext")
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            Ok((a, b_ext))
        } else {
            let a_ext = self
                .builder
                .build_float_ext(a, self.context.f64_type(), "fext")
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            Ok((a_ext, b))
        }
    }

    pub fn compile(&mut self, unit: &CompilationUnit) -> Result<(), CodegenError> {
        self.register_standard_functions();

        // Register types and POUs
        for decl in &unit.declarations {
            if let Declaration::FunctionBlock(fb) = decl {
                let fb_type = IecType::FbInstance(fb.name.name.clone());
                self.type_registry.register(
                    fb.name.name.to_uppercase(),
                    fb_type.clone(),
                );
                // Also register in the type checker so resolve_type_spec finds it.
                // Register both original case and uppercase since TypeRegistry lookups
                // are case-sensitive and the parser may preserve original casing.
                self.type_checker.types.register(
                    fb.name.name.to_uppercase(),
                    fb_type.clone(),
                );
                self.type_checker.types.register(
                    fb.name.name.clone(),
                    fb_type,
                );
            }
        }

        // Scan for VAR_GLOBAL declarations and create a global struct
        let mut global_fields = Vec::new();
        let mut global_names = Vec::new();
        for decl in &unit.declarations {
            if let Declaration::GlobalVarDecl(block) = decl {
                for var in &block.declarations {
                    let ty = self.resolve_type_spec(&var.type_spec);
                    global_fields.push(self.iec_to_llvm_type(&ty));
                    global_names.push((var.name.name.clone(), ty, var.initializer.clone()));
                }
            }
        }
        if !global_fields.is_empty() {
            let global_struct = self.context.struct_type(&global_fields, false);
            let global_val = self.module.add_global(global_struct, None, "plcc_globals");
            // Build initializer with constant values
            let mut const_vals: Vec<inkwell::values::BasicValueEnum<'ctx>> = Vec::new();
            for (i, (_name, ty, init)) in global_names.iter().enumerate() {
                if let Some(init_expr) = init {
                    // Try to evaluate constant initializer
                    if let Some(val) = self.eval_const_initializer(init_expr, ty) {
                        const_vals.push(val);
                    } else {
                        const_vals.push(global_fields[i].const_zero());
                    }
                } else {
                    const_vals.push(global_fields[i].const_zero());
                }
            }
            let init = global_struct.const_named_struct(&const_vals);
            global_val.set_initializer(&init);

            let names_types: Vec<(String, IecType)> = global_names
                .into_iter()
                .map(|(name, ty, _)| (name, ty))
                .collect();
            self.global_var = Some((global_val, global_struct, names_types));
        }

        // Compile FBs and functions first so they're available when programs reference them
        for decl in &unit.declarations {
            match decl {
                Declaration::Function(f) => self.compile_function(f)?,
                Declaration::FunctionBlock(fb) => self.compile_function_block(fb)?,
                _ => {}
            }
        }
        // Then compile programs (which may instantiate FBs)
        for decl in &unit.declarations {
            match decl {
                Declaration::Program(p) => self.compile_program(p)?,
                _ => {}
            }
        }
        Ok(())
    }

    /// Evaluate a constant expression for use as a global initializer.
    fn eval_const_initializer(
        &self,
        expr: &Expression,
        _ty: &IecType,
    ) -> Option<BasicValueEnum<'ctx>> {
        match &expr.kind {
            ExpressionKind::IntegerLiteral(v) => {
                // Use i16 (INT) by default, matching compile_expression
                Some(self.context.i16_type().const_int(*v as u64, true).into())
            }
            ExpressionKind::RealLiteral(v) => {
                Some(self.context.f32_type().const_float(*v).into())
            }
            ExpressionKind::BoolLiteral(v) => {
                Some(self.context.i8_type().const_int(*v as u64, false).into())
            }
            _ => None,
        }
    }

    /// Add global variable GEPs to the variables map.
    fn add_globals_to_variables(&mut self) -> Result<(), CodegenError> {
        if let Some((global_val, global_struct, ref names)) = self.global_var.clone() {
            let global_ptr = global_val.as_pointer_value();
            for (i, (name, iec_ty)) in names.iter().enumerate() {
                let ptr = self
                    .builder
                    .build_struct_gep(global_struct, global_ptr, i as u32, name)
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                self.variables
                    .insert(name.to_uppercase(), (ptr, iec_ty.clone()));
            }
        }
        Ok(())
    }

    /// Convert any integer value to i1 for use in conditional branches.
    fn to_i1(&self, val: inkwell::values::IntValue<'ctx>) -> Result<inkwell::values::IntValue<'ctx>, CodegenError> {
        let bit_width = val.get_type().get_bit_width();
        if bit_width == 1 {
            Ok(val)
        } else {
            // Truncate or compare != 0
            let zero = val.get_type().const_zero();
            self.builder
                .build_int_compare(IntPredicate::NE, val, zero, "tobool")
                .map_err(|e| CodegenError::LlvmError(e.to_string()))
        }
    }

    /// Ensure two integer values have the same bit width by extending the smaller one.
    fn match_int_widths(
        &self,
        a: inkwell::values::IntValue<'ctx>,
        b: inkwell::values::IntValue<'ctx>,
    ) -> Result<(inkwell::values::IntValue<'ctx>, inkwell::values::IntValue<'ctx>), CodegenError> {
        let aw = a.get_type().get_bit_width();
        let bw = b.get_type().get_bit_width();
        if aw == bw {
            Ok((a, b))
        } else if aw < bw {
            let ext = self.builder
                .build_int_s_extend(a, b.get_type(), "sext")
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            Ok((ext, b))
        } else {
            let ext = self.builder
                .build_int_s_extend(b, a.get_type(), "sext")
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            Ok((a, ext))
        }
    }

    fn iec_to_llvm_type(&self, ty: &IecType) -> BasicTypeEnum<'ctx> {
        match ty {
            IecType::Bool => self.context.i8_type().into(), // i8 for memory-safe layout
            IecType::Sint | IecType::Byte => self.context.i8_type().into(),
            IecType::Int | IecType::Word | IecType::Usint => self.context.i16_type().into(),
            IecType::Dint | IecType::Dword | IecType::Uint | IecType::Udint => {
                self.context.i32_type().into()
            }
            IecType::Lint | IecType::Lword | IecType::Ulint => self.context.i64_type().into(),
            IecType::Real => self.context.f32_type().into(),
            IecType::Lreal => self.context.f64_type().into(),
            IecType::Array {
                ranges,
                element_type,
            } => {
                let elem_ty = self.iec_to_llvm_type(element_type);
                let total_size: u32 = ranges
                    .iter()
                    .map(|(lo, hi)| (hi - lo + 1) as u32)
                    .product();
                elem_ty.array_type(total_size).into()
            }
            // TIME/LTIME stored as i64 (nanoseconds)
            IecType::Time | IecType::Ltime => self.context.i64_type().into(),
            // DATE types stored as i64 (Unix timestamp in nanoseconds)
            IecType::Date | IecType::Tod | IecType::Dt | IecType::Ldate | IecType::Ltod | IecType::Ldt => {
                self.context.i64_type().into()
            }
            // STRING stored as fixed-size byte array (default 256 bytes)
            IecType::StringType { max_len } => {
                let len = max_len.unwrap_or(256) + 1; // +1 for null terminator
                self.context.i8_type().array_type(len as u32).into()
            }
            IecType::WstringType { max_len } => {
                let len = max_len.unwrap_or(256) + 1;
                self.context.i16_type().array_type(len as u32).into()
            }
            IecType::Char => self.context.i8_type().into(),
            IecType::Wchar => self.context.i16_type().into(),
            // ENUM backed by base type (default i32)
            IecType::Enum { base_type, .. } => self.iec_to_llvm_type(base_type),
            // Subrange uses base type
            IecType::Subrange { base_type, .. } => self.iec_to_llvm_type(base_type),
            // Pointer as opaque ptr
            IecType::Pointer(_) => self.context.ptr_type(AddressSpace::default()).into(),
            // Struct with known fields
            IecType::Struct { fields, .. } => {
                let field_types: Vec<BasicTypeEnum<'ctx>> = fields
                    .iter()
                    .map(|(_, ft)| self.iec_to_llvm_type(ft))
                    .collect();
                self.context.struct_type(&field_types, false).into()
            }
            // FB instance — look up the compiled FB's struct type
            IecType::FbInstance(name) => {
                if let Some(layout) = self.compiled_fbs.get(&name.to_uppercase()) {
                    layout.struct_type.into()
                } else {
                    // Fallback if FB hasn't been compiled yet
                    self.context.i32_type().into()
                }
            }
            // Fallback for others
            _ => self.context.i32_type().into(),
        }
    }

    fn resolve_type_spec(&mut self, spec: &TypeSpec) -> IecType {
        self.type_checker.resolve_type_spec(spec)
    }

    fn compile_program(&mut self, prog: &ProgramDecl) -> Result<(), CodegenError> {
        // Create a scan() function for this program
        let fn_name = format!("{}_scan", prog.name.name.to_lowercase());

        // Build the struct type for program state
        let mut field_types: Vec<BasicTypeEnum<'ctx>> = Vec::new();
        let mut field_names: Vec<String> = Vec::new();
        let mut field_iec_types: Vec<IecType> = Vec::new();

        for block in &prog.var_blocks {
            for decl in &block.declarations {
                let iec_ty = self.resolve_type_spec(&decl.type_spec);
                let llvm_ty = self.iec_to_llvm_type(&iec_ty);
                field_types.push(llvm_ty);
                field_names.push(decl.name.name.clone());
                field_iec_types.push(iec_ty);
            }
        }

        let struct_type = self.context.struct_type(&field_types, false);
        let state_ptr_type = self.context.ptr_type(AddressSpace::default());

        // scan(state: *mut ProgramState) -> void
        let fn_type = self.context.void_type().fn_type(&[state_ptr_type.into()], false);
        let function = self.module.add_function(&fn_name, fn_type, None);

        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);

        let state_ptr = function.get_nth_param(0).unwrap().into_pointer_value();

        // Set up variables as GEP into the state struct, and detect FB instances
        self.variables.clear();
        self.fb_instances.clear();
        self.current_struct_type = Some(struct_type);
        self.current_state_ptr = Some(state_ptr);

        for (i, (name, iec_ty)) in field_names
            .iter()
            .zip(field_iec_types.iter())
            .enumerate()
        {
            let ptr = self
                .builder
                .build_struct_gep(struct_type, state_ptr, i as u32, name)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            self.variables
                .insert(name.to_uppercase(), (ptr, iec_ty.clone()));

            // If this is an FB instance, register it
            if let IecType::FbInstance(fb_type_name) = iec_ty {
                if let Some(layout) = self.compiled_fbs.get(&fb_type_name.to_uppercase()).cloned() {
                    self.fb_instances.insert(
                        name.to_uppercase(),
                        FbInstanceInfo {
                            field_index: i as u32,
                            fb_type_name: fb_type_name.clone(),
                            scan_fn_name: layout.scan_fn_name.clone(),
                            fields: layout.fields.clone(),
                            struct_type: layout.struct_type,
                        },
                    );
                }
            }
        }

        // Add global variables
        self.add_globals_to_variables()?;

        // Compile body
        for stmt in &prog.body {
            self.compile_statement(stmt, function)?;
        }

        self.builder.build_return(None)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        // Generate _init() function that applies variable initializers
        let init_fn_name = format!("{}_init", prog.name.name.to_lowercase());
        let init_fn = self.module.add_function(&init_fn_name, fn_type, None);
        let init_entry = self.context.append_basic_block(init_fn, "entry");
        self.builder.position_at_end(init_entry);

        let init_state_ptr = init_fn.get_nth_param(0).unwrap().into_pointer_value();

        // Re-create GEPs for init function
        let mut field_idx = 0u32;
        for block in &prog.var_blocks {
            for decl in &block.declarations {
                let iec_ty = self.resolve_type_spec(&decl.type_spec);
                // Skip FB instance fields in init — they are zeroed which is fine
                // (FB internal vars with initializers would need their own _init, but
                // zero-init is correct default for IEC FBs)
                let ptr = self
                    .builder
                    .build_struct_gep(struct_type, init_state_ptr, field_idx, &decl.name.name)
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                self.variables
                    .insert(decl.name.name.to_uppercase(), (ptr, iec_ty.clone()));

                if !matches!(iec_ty, IecType::FbInstance(_)) {
                    if let Some(init_expr) = &decl.initializer {
                        if let Some(val) = self.compile_expression(init_expr, init_fn)? {
                            self.builder
                                .build_store(ptr, val)
                                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                        }
                    }
                }
                field_idx += 1;
            }
        }

        self.builder.build_return(None)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        Ok(())
    }

    fn compile_function(&mut self, func: &FunctionDecl) -> Result<(), CodegenError> {
        let ret_iec_ty = func
            .return_type
            .as_ref()
            .map(|t| self.resolve_type_spec(t))
            .unwrap_or(IecType::Void);

        // Collect input params
        let mut param_types: Vec<BasicMetadataTypeEnum<'ctx>> = Vec::new();
        let mut param_names: Vec<String> = Vec::new();
        let mut param_iec_types: Vec<IecType> = Vec::new();

        for block in &func.var_blocks {
            if block.kind == VarBlockKind::VarInput {
                for decl in &block.declarations {
                    let iec_ty = self.resolve_type_spec(&decl.type_spec);
                    let llvm_ty = self.iec_to_llvm_type(&iec_ty);
                    param_types.push(llvm_ty.into());
                    param_names.push(decl.name.name.clone());
                    param_iec_types.push(iec_ty);
                }
            }
        }

        let fn_type = if ret_iec_ty == IecType::Void {
            self.context.void_type().fn_type(&param_types, false)
        } else {
            let ret_llvm = self.iec_to_llvm_type(&ret_iec_ty);
            ret_llvm.fn_type(&param_types, false)
        };

        let function = self
            .module
            .add_function(&func.name.name.to_lowercase(), fn_type, None);
        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);

        self.variables.clear();

        // Allocate input params
        for (i, (name, iec_ty)) in param_names
            .iter()
            .zip(param_iec_types.iter())
            .enumerate()
        {
            let llvm_ty = self.iec_to_llvm_type(iec_ty);
            let alloca = self
                .builder
                .build_alloca(llvm_ty, name)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            self.builder
                .build_store(alloca, function.get_nth_param(i as u32).unwrap())
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            self.variables
                .insert(name.to_uppercase(), (alloca, iec_ty.clone()));
        }

        // Allocate local vars
        for block in &func.var_blocks {
            if block.kind != VarBlockKind::VarInput {
                for decl in &block.declarations {
                    let iec_ty = self.resolve_type_spec(&decl.type_spec);
                    let llvm_ty = self.iec_to_llvm_type(&iec_ty);
                    let alloca = self
                        .builder
                        .build_alloca(llvm_ty, &decl.name.name)
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

                    // Initialize if there's an initializer
                    if let Some(init) = &decl.initializer {
                        if let Some(val) = self.compile_expression(init, function)? {
                            self.builder
                                .build_store(alloca, val)
                                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                        }
                    }

                    self.variables
                        .insert(decl.name.name.to_uppercase(), (alloca, iec_ty));
                }
            }
        }

        // Return value variable (function name = return)
        if ret_iec_ty != IecType::Void {
            let ret_llvm = self.iec_to_llvm_type(&ret_iec_ty);
            let ret_alloca = self
                .builder
                .build_alloca(ret_llvm, &func.name.name)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            self.variables.insert(
                func.name.name.to_uppercase(),
                (ret_alloca, ret_iec_ty.clone()),
            );
        }

        // Add global variables
        self.add_globals_to_variables()?;

        // Compile body
        for stmt in &func.body {
            self.compile_statement(stmt, function)?;
        }

        // Return
        if ret_iec_ty != IecType::Void {
            let ret_llvm_ty = self.iec_to_llvm_type(&ret_iec_ty);
            if let Some((ptr, _)) = self.variables.get(&func.name.name.to_uppercase()) {
                let val = self
                    .builder
                    .build_load(ret_llvm_ty, *ptr, "retval")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                self.builder
                    .build_return(Some(&val))
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            } else {
                self.builder
                    .build_return(None)
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            }
        } else {
            self.builder
                .build_return(None)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        }

        Ok(())
    }

    fn compile_function_block(&mut self, fb: &FunctionBlockDecl) -> Result<(), CodegenError> {
        // Similar to program — creates a scan function with state struct pointer
        let fn_name = format!("{}_scan", fb.name.name.to_lowercase());

        let mut field_types: Vec<BasicTypeEnum<'ctx>> = Vec::new();
        let mut field_names: Vec<String> = Vec::new();
        let mut field_iec_types: Vec<IecType> = Vec::new();

        for block in &fb.var_blocks {
            for decl in &block.declarations {
                let iec_ty = self.resolve_type_spec(&decl.type_spec);
                let llvm_ty = self.iec_to_llvm_type(&iec_ty);
                field_types.push(llvm_ty);
                field_names.push(decl.name.name.clone());
                field_iec_types.push(iec_ty);
            }
        }

        let struct_type = self.context.struct_type(&field_types, false);
        let state_ptr_type = self.context.ptr_type(AddressSpace::default());
        let fn_type = self.context.void_type().fn_type(&[state_ptr_type.into()], false);
        let function = self.module.add_function(&fn_name, fn_type, None);

        // Record this FB's layout for use by parent POUs that instantiate it
        let fb_fields: Vec<(String, IecType)> = field_names
            .iter()
            .zip(field_iec_types.iter())
            .map(|(n, t)| (n.clone(), t.clone()))
            .collect();
        self.compiled_fbs.insert(
            fb.name.name.to_uppercase(),
            FbLayout {
                struct_type,
                scan_fn_name: fn_name.clone(),
                fields: fb_fields,
            },
        );

        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);

        let state_ptr = function.get_nth_param(0).unwrap().into_pointer_value();

        self.variables.clear();
        self.fb_instances.clear();
        for (i, (name, iec_ty)) in field_names
            .iter()
            .zip(field_iec_types.iter())
            .enumerate()
        {
            let ptr = self
                .builder
                .build_struct_gep(struct_type, state_ptr, i as u32, name)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            self.variables
                .insert(name.to_uppercase(), (ptr, iec_ty.clone()));
        }

        // Add global variables
        self.add_globals_to_variables()?;

        for stmt in &fb.body {
            self.compile_statement(stmt, function)?;
        }

        self.builder
            .build_return(None)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        Ok(())
    }

    fn compile_statement(
        &mut self,
        stmt: &Statement,
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        match &stmt.kind {
            StatementKind::Assignment { target, value } => {
                if let Some(ptr) = self.compile_lvalue_with_fn(target, function)? {
                    if let Some(val) = self.compile_expression(value, function)? {
                        self.builder
                            .build_store(ptr, val)
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    }
                }
            }
            StatementKind::If {
                condition,
                then_body,
                elsif_branches,
                else_body,
            } => {
                self.compile_if(condition, then_body, elsif_branches, else_body, function)?;
            }
            StatementKind::For {
                variable,
                from,
                to,
                by,
                body,
            } => {
                self.compile_for(variable, from, to, by, body, function)?;
            }
            StatementKind::While { condition, body } => {
                self.compile_while(condition, body, function)?;
            }
            StatementKind::Case {
                selector,
                branches,
                else_body,
            } => {
                self.compile_case(selector, branches, else_body, function)?;
            }
            StatementKind::FunctionCall { callee, args } => {
                // Check if this is an FB instance call
                if let ExpressionKind::Identifier(ident) = &callee.kind {
                    if self.fb_instances.contains_key(&ident.name.to_uppercase()) {
                        self.compile_fb_call(&ident.name, args, function)?;
                    } else {
                        // Regular function call
                        let call_expr = Expression {
                            kind: ExpressionKind::FunctionCall {
                                callee: Box::new(callee.clone()),
                                args: args.clone(),
                            },
                            span: stmt.span,
                        };
                        self.compile_expression(&call_expr, function)?;
                    }
                } else {
                    let call_expr = Expression {
                        kind: ExpressionKind::FunctionCall {
                            callee: Box::new(callee.clone()),
                            args: args.clone(),
                        },
                        span: stmt.span,
                    };
                    self.compile_expression(&call_expr, function)?;
                }
            }
            StatementKind::Repeat { body, until } => {
                self.compile_repeat(body, until, function)?;
            }
            StatementKind::Exit => {
                if let Some(exit_bb) = self.loop_exit_bb {
                    self.builder
                        .build_unconditional_branch(exit_bb)
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    // Create an unreachable block so subsequent statements have somewhere to go
                    let after_exit = self.context.append_basic_block(function, "after_exit");
                    self.builder.position_at_end(after_exit);
                }
            }
            StatementKind::Return { .. }
            | StatementKind::Continue
            | StatementKind::Empty => {
                // TODO: implement these
            }
        }
        Ok(())
    }

    /// Compile an FB instance call: write inputs, call scan, leave outputs in place.
    fn compile_fb_call(
        &mut self,
        instance_name: &str,
        args: &[CallArg],
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        let info = self
            .fb_instances
            .get(&instance_name.to_uppercase())
            .ok_or_else(|| {
                CodegenError::UndefinedVariable(format!("FB instance '{instance_name}' not found"))
            })?
            .clone();

        // Get pointer to the embedded FB struct within the parent state
        let parent_struct_type = self.current_struct_type.ok_or_else(|| {
            CodegenError::LlvmError("no parent struct type for FB instance".into())
        })?;
        let parent_state_ptr = self.current_state_ptr.ok_or_else(|| {
            CodegenError::LlvmError("no parent state pointer for FB instance".into())
        })?;
        let fb_ptr = self
            .builder
            .build_struct_gep(parent_struct_type, parent_state_ptr, info.field_index, &format!("{}_ptr", instance_name))
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        // Write named arguments (inputs) to the FB struct fields
        for arg in args {
            if let Some(arg_name) = &arg.name {
                // Find the field index in the FB's struct
                let field_idx = info
                    .fields
                    .iter()
                    .position(|(name, _)| name.eq_ignore_ascii_case(&arg_name.name))
                    .ok_or_else(|| {
                        CodegenError::UndefinedVariable(format!(
                            "FB field '{}' not found in '{}'",
                            arg_name.name, info.fb_type_name
                        ))
                    })?;

                // Compile the argument value
                if let Some(val) = self.compile_expression(&arg.value, function)? {
                    let field_ptr = self
                        .builder
                        .build_struct_gep(info.struct_type, fb_ptr, field_idx as u32, &arg_name.name)
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    self.builder
                        .build_store(field_ptr, val)
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                }
            }
        }

        // Call the FB's scan function
        let scan_fn = self
            .module
            .get_function(&info.scan_fn_name)
            .ok_or_else(|| {
                CodegenError::LlvmError(format!(
                    "FB scan function '{}' not found",
                    info.scan_fn_name
                ))
            })?;
        self.builder
            .build_call(scan_fn, &[fb_ptr.into()], &format!("{}_call", instance_name))
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        Ok(())
    }

    fn compile_if(
        &mut self,
        condition: &Expression,
        then_body: &[Statement],
        elsif_branches: &[ElsifBranch],
        else_body: &Option<Vec<Statement>>,
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        let cond_val = self
            .compile_expression(condition, function)?
            .ok_or_else(|| CodegenError::LlvmError("failed to compile condition".into()))?;

        let then_bb = self.context.append_basic_block(function, "then");
        let merge_bb = self.context.append_basic_block(function, "merge");

        if elsif_branches.is_empty() && else_body.is_none() {
            self.builder
                .build_conditional_branch(self.to_i1(cond_val.into_int_value())?, then_bb, merge_bb)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

            self.builder.position_at_end(then_bb);
            for stmt in then_body {
                self.compile_statement(stmt, function)?;
            }
            self.builder
                .build_unconditional_branch(merge_bb)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        } else {
            let else_bb = self.context.append_basic_block(function, "else");
            self.builder
                .build_conditional_branch(self.to_i1(cond_val.into_int_value())?, then_bb, else_bb)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

            self.builder.position_at_end(then_bb);
            for stmt in then_body {
                self.compile_statement(stmt, function)?;
            }
            self.builder
                .build_unconditional_branch(merge_bb)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

            self.builder.position_at_end(else_bb);
            // Handle elsif chains
            if !elsif_branches.is_empty() {
                for (i, branch) in elsif_branches.iter().enumerate() {
                    let elsif_cond = self
                        .compile_expression(&branch.condition, function)?
                        .ok_or_else(|| {
                            CodegenError::LlvmError("failed to compile elsif condition".into())
                        })?;

                    let elsif_then = self.context.append_basic_block(function, "elsif_then");
                    let elsif_else = if i + 1 < elsif_branches.len() || else_body.is_some() {
                        self.context.append_basic_block(function, "elsif_else")
                    } else {
                        merge_bb
                    };

                    self.builder
                        .build_conditional_branch(
                            self.to_i1(elsif_cond.into_int_value())?,
                            elsif_then,
                            elsif_else,
                        )
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

                    self.builder.position_at_end(elsif_then);
                    for stmt in &branch.body {
                        self.compile_statement(stmt, function)?;
                    }
                    self.builder
                        .build_unconditional_branch(merge_bb)
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

                    self.builder.position_at_end(elsif_else);
                }
            }

            if let Some(body) = else_body {
                for stmt in body {
                    self.compile_statement(stmt, function)?;
                }
            }
            self.builder
                .build_unconditional_branch(merge_bb)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        }

        self.builder.position_at_end(merge_bb);
        Ok(())
    }

    fn compile_for(
        &mut self,
        variable: &Ident,
        from: &Expression,
        to: &Expression,
        by: &Option<Expression>,
        body: &[Statement],
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        let (var_ptr, var_ty) = self
            .variables
            .get(&variable.name.to_uppercase())
            .ok_or_else(|| CodegenError::UndefinedVariable(variable.name.clone()))?
            .clone();

        let from_val = self
            .compile_expression(from, function)?
            .ok_or_else(|| CodegenError::LlvmError("failed to compile from".into()))?;
        let to_val = self
            .compile_expression(to, function)?
            .ok_or_else(|| CodegenError::LlvmError("failed to compile to".into()))?;

        // Store initial value
        self.builder
            .build_store(var_ptr, from_val)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        let loop_bb = self.context.append_basic_block(function, "for_loop");
        let body_bb = self.context.append_basic_block(function, "for_body");
        let end_bb = self.context.append_basic_block(function, "for_end");

        // Save and set loop_exit_bb for EXIT support
        let prev_exit_bb = self.loop_exit_bb;
        self.loop_exit_bb = Some(end_bb);

        self.builder
            .build_unconditional_branch(loop_bb)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        // Loop condition
        self.builder.position_at_end(loop_bb);
        let llvm_ty = self.iec_to_llvm_type(&var_ty);
        let cur_val = self
            .builder
            .build_load(llvm_ty, var_ptr, "cur")
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        let (cur_i, to_i) = self.match_int_widths(cur_val.into_int_value(), to_val.into_int_value())?;
        // Determine loop direction: if BY is negative, compare with SGE instead of SLE
        let step_is_negative = if let Some(by_expr) = by {
            if let ExpressionKind::UnaryOp {
                op: UnaryOp::Neg, ..
            } = &by_expr.kind
            {
                true
            } else if let ExpressionKind::IntegerLiteral(v) = &by_expr.kind {
                *v < 0
            } else {
                false
            }
        } else {
            false
        };
        let predicate = if step_is_negative {
            IntPredicate::SGE
        } else {
            IntPredicate::SLE
        };
        let cond = self
            .builder
            .build_int_compare(
                predicate,
                cur_i,
                to_i,
                "for_cond",
            )
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        self.builder
            .build_conditional_branch(cond, body_bb, end_bb)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        // Body
        self.builder.position_at_end(body_bb);
        for stmt in body {
            self.compile_statement(stmt, function)?;
        }

        // Increment
        let cur_val2 = self
            .builder
            .build_load(llvm_ty, var_ptr, "cur2")
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        let step = if let Some(by_expr) = by {
            self.compile_expression(by_expr, function)?
                .ok_or_else(|| CodegenError::LlvmError("failed to compile step".into()))?
        } else {
            // Default step = 1 with same type as loop variable
            cur_val2.into_int_value().get_type().const_int(1, false).into()
        };
        let (cur_i, step_i) = self.match_int_widths(cur_val2.into_int_value(), step.into_int_value())?;
        let next_val = self
            .builder
            .build_int_add(cur_i, step_i, "next")
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        self.builder
            .build_store(var_ptr, next_val)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        self.builder
            .build_unconditional_branch(loop_bb)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        self.builder.position_at_end(end_bb);
        self.loop_exit_bb = prev_exit_bb;
        Ok(())
    }

    fn compile_while(
        &mut self,
        condition: &Expression,
        body: &[Statement],
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        let cond_bb = self.context.append_basic_block(function, "while_cond");
        let body_bb = self.context.append_basic_block(function, "while_body");
        let end_bb = self.context.append_basic_block(function, "while_end");

        // Save and set loop_exit_bb for EXIT support
        let prev_exit_bb = self.loop_exit_bb;
        self.loop_exit_bb = Some(end_bb);

        self.builder
            .build_unconditional_branch(cond_bb)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        self.builder.position_at_end(cond_bb);
        let cond_val = self
            .compile_expression(condition, function)?
            .ok_or_else(|| CodegenError::LlvmError("failed to compile condition".into()))?;
        self.builder
            .build_conditional_branch(cond_val.into_int_value(), body_bb, end_bb)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        self.builder.position_at_end(body_bb);
        for stmt in body {
            self.compile_statement(stmt, function)?;
        }
        self.builder
            .build_unconditional_branch(cond_bb)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        self.builder.position_at_end(end_bb);
        self.loop_exit_bb = prev_exit_bb;
        Ok(())
    }

    fn compile_repeat(
        &mut self,
        body: &[Statement],
        until: &Expression,
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        let body_bb = self.context.append_basic_block(function, "repeat_body");
        let cond_bb = self.context.append_basic_block(function, "repeat_cond");
        let end_bb = self.context.append_basic_block(function, "repeat_end");

        // Save and set loop_exit_bb for EXIT support
        let prev_exit_bb = self.loop_exit_bb;
        self.loop_exit_bb = Some(end_bb);

        // Jump into body (do-while: body executes at least once)
        self.builder
            .build_unconditional_branch(body_bb)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        // Body
        self.builder.position_at_end(body_bb);
        for stmt in body {
            self.compile_statement(stmt, function)?;
        }
        self.builder
            .build_unconditional_branch(cond_bb)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        // Condition check: if UNTIL condition is true, exit; otherwise loop back
        self.builder.position_at_end(cond_bb);
        let cond_val = self
            .compile_expression(until, function)?
            .ok_or_else(|| CodegenError::LlvmError("failed to compile UNTIL condition".into()))?;
        let cond_bool = self.to_i1(cond_val.into_int_value())?;
        // UNTIL means: exit when true, loop when false
        self.builder
            .build_conditional_branch(cond_bool, end_bb, body_bb)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        self.builder.position_at_end(end_bb);
        self.loop_exit_bb = prev_exit_bb;
        Ok(())
    }

    fn compile_case(
        &mut self,
        selector: &Expression,
        branches: &[CaseBranch],
        else_body: &Option<Vec<Statement>>,
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        let sel_val = self
            .compile_expression(selector, function)?
            .ok_or_else(|| CodegenError::LlvmError("failed to compile selector".into()))?;

        let end_bb = self.context.append_basic_block(function, "case_end");
        let else_bb = if else_body.is_some() {
            self.context.append_basic_block(function, "case_else")
        } else {
            end_bb
        };

        // Build switch — case label constants must match the selector's integer type
        let sel_int_ty = sel_val.into_int_value().get_type();
        let mut cases: Vec<(inkwell::values::IntValue<'ctx>, inkwell::basic_block::BasicBlock<'ctx>)> = Vec::new();
        let mut branch_blocks = Vec::new();

        for (i, branch) in branches.iter().enumerate() {
            let bb = self
                .context
                .append_basic_block(function, &format!("case_{i}"));
            branch_blocks.push(bb);
            for label in &branch.labels {
                match label {
                    CaseLabel::Value(expr) => {
                        if let ExpressionKind::IntegerLiteral(v) = &expr.kind {
                            cases.push((
                                sel_int_ty.const_int(*v as u64, true),
                                bb,
                            ));
                        }
                    }
                    CaseLabel::Range(lo, hi) => {
                        if let (
                            ExpressionKind::IntegerLiteral(lo_v),
                            ExpressionKind::IntegerLiteral(hi_v),
                        ) = (&lo.kind, &hi.kind)
                        {
                            for v in *lo_v..=*hi_v {
                                cases.push((
                                    sel_int_ty.const_int(v as u64, true),
                                    bb,
                                ));
                            }
                        }
                    }
                }
            }
        }

        let switch = self
            .builder
            .build_switch(sel_val.into_int_value(), else_bb, &cases)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        // Compile branch bodies
        for (i, branch) in branches.iter().enumerate() {
            self.builder.position_at_end(branch_blocks[i]);
            for stmt in &branch.body {
                self.compile_statement(stmt, function)?;
            }
            self.builder
                .build_unconditional_branch(end_bb)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        }

        // Else body
        if let Some(body) = else_body {
            self.builder.position_at_end(else_bb);
            for stmt in body {
                self.compile_statement(stmt, function)?;
            }
            self.builder
                .build_unconditional_branch(end_bb)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        }

        self.builder.position_at_end(end_bb);
        Ok(())
    }

    fn compile_lvalue_with_fn(
        &mut self,
        expr: &Expression,
        function: FunctionValue<'ctx>,
    ) -> Result<Option<PointerValue<'ctx>>, CodegenError> {
        self.compile_lvalue_inner(expr, Some(function))
    }

    fn compile_lvalue_inner(
        &mut self,
        expr: &Expression,
        function: Option<FunctionValue<'ctx>>,
    ) -> Result<Option<PointerValue<'ctx>>, CodegenError> {
        match &expr.kind {
            ExpressionKind::Identifier(ident) => {
                Ok(self
                    .variables
                    .get(&ident.name.to_uppercase())
                    .map(|(ptr, _)| *ptr))
            }
            ExpressionKind::ArrayIndex { array, indices } => {
                // Get the array variable's pointer and IEC type
                if let ExpressionKind::Identifier(ident) = &array.kind {
                    if let Some((arr_ptr, iec_ty)) = self.variables.get(&ident.name.to_uppercase()).cloned() {
                        if let IecType::Array { ref ranges, .. } = iec_ty {
                            let ranges = ranges.clone();
                            let function = function.ok_or_else(|| {
                                CodegenError::LlvmError("array index in lvalue requires function context".into())
                            })?;
                            let arr_llvm_ty = self.iec_to_llvm_type(&iec_ty);

                            if indices.len() == 1 {
                                let idx_val = self.compile_expression(&indices[0], function)?
                                    .ok_or_else(|| CodegenError::LlvmError("failed to compile array index".into()))?;

                                let lo = ranges[0].0;
                                let idx_int = idx_val.into_int_value();
                                let adjusted = if lo != 0 {
                                    let lo_val = idx_int.get_type().const_int(lo as u64, true);
                                    self.builder
                                        .build_int_sub(idx_int, lo_val, "adj_idx")
                                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                                } else {
                                    idx_int
                                };

                                let idx_i32 = if adjusted.get_type().get_bit_width() < 32 {
                                    self.builder
                                        .build_int_s_extend(adjusted, self.context.i32_type(), "idx_ext")
                                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                                } else {
                                    adjusted
                                };

                                let zero = self.context.i32_type().const_zero();
                                let elem_ptr = unsafe {
                                    self.builder
                                        .build_in_bounds_gep(arr_llvm_ty, arr_ptr, &[zero, idx_i32], "arr_elem")
                                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                                };
                                Ok(Some(elem_ptr))
                            } else {
                                // Multi-dimensional: flatten to linear index
                                let function = function;
                                let mut linear_idx = self.context.i32_type().const_zero();

                                for (dim, idx_expr) in indices.iter().enumerate() {
                                    let idx_val = self.compile_expression(idx_expr, function)?
                                        .ok_or_else(|| CodegenError::LlvmError("failed to compile array index".into()))?;

                                    let lo = ranges[dim].0;
                                    let idx_int = idx_val.into_int_value();
                                    let idx_i32 = if idx_int.get_type().get_bit_width() < 32 {
                                        self.builder
                                            .build_int_s_extend(idx_int, self.context.i32_type(), "idx_ext")
                                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                                    } else {
                                        idx_int
                                    };

                                    let adjusted = if lo != 0 {
                                        let lo_val = self.context.i32_type().const_int(lo as u64, true);
                                        self.builder
                                            .build_int_sub(idx_i32, lo_val, "adj_idx")
                                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                                    } else {
                                        idx_i32
                                    };

                                    let mut stride = 1i64;
                                    for d in (dim + 1)..ranges.len() {
                                        stride *= ranges[d].1 - ranges[d].0 + 1;
                                    }
                                    let stride_val = self.context.i32_type().const_int(stride as u64, false);
                                    let component = self.builder
                                        .build_int_mul(adjusted, stride_val, "dim_component")
                                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                                    linear_idx = self.builder
                                        .build_int_add(linear_idx, component, "linear_idx")
                                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                                }

                                let zero = self.context.i32_type().const_zero();
                                let elem_ptr = unsafe {
                                    self.builder
                                        .build_in_bounds_gep(arr_llvm_ty, arr_ptr, &[zero, linear_idx], "arr_elem")
                                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                                };
                                Ok(Some(elem_ptr))
                            }
                        } else {
                            Err(CodegenError::UnsupportedType(format!(
                                "array indexing on non-array type: {}", iec_ty
                            )))
                        }
                    } else {
                        Ok(None)
                    }
                } else {
                    Ok(None)
                }
            }
            ExpressionKind::MemberAccess { object, member } => {
                if let ExpressionKind::Identifier(ident) = &object.kind {
                    // Check if this is an FB instance field access
                    if let Some(info) = self.fb_instances.get(&ident.name.to_uppercase()).cloned() {
                        let parent_struct_type = self.current_struct_type.ok_or_else(|| {
                            CodegenError::LlvmError("no parent struct type".into())
                        })?;
                        let parent_state_ptr = self.current_state_ptr.ok_or_else(|| {
                            CodegenError::LlvmError("no parent state pointer".into())
                        })?;
                        let fb_ptr = self
                            .builder
                            .build_struct_gep(parent_struct_type, parent_state_ptr, info.field_index, &format!("{}_fb_lv", ident.name))
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

                        let field_idx = info
                            .fields
                            .iter()
                            .position(|(name, _)| name.eq_ignore_ascii_case(&member.name))
                            .ok_or_else(|| {
                                CodegenError::UndefinedVariable(format!(
                                    "{}.{}",
                                    ident.name, member.name
                                ))
                            })?;
                        let field_ptr = self
                            .builder
                            .build_struct_gep(info.struct_type, fb_ptr, field_idx as u32, &member.name)
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                        return Ok(Some(field_ptr));
                    }

                    // Fall back to struct field access
                    if let Some((obj_ptr, iec_ty)) = self.variables.get(&ident.name.to_uppercase()).cloned() {
                        if let IecType::Struct { fields, .. } = &iec_ty {
                            let field_idx = fields
                                .iter()
                                .position(|(name, _)| name.eq_ignore_ascii_case(&member.name))
                                .ok_or_else(|| {
                                    CodegenError::UndefinedVariable(format!(
                                        "{}.{}",
                                        ident.name, member.name
                                    ))
                                })?;
                            let struct_llvm_ty = self.iec_to_llvm_type(&iec_ty);
                            let field_ptr = self
                                .builder
                                .build_struct_gep(
                                    struct_llvm_ty.into_struct_type(),
                                    obj_ptr,
                                    field_idx as u32,
                                    &member.name,
                                )
                                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                            Ok(Some(field_ptr))
                        } else {
                            Ok(None)
                        }
                    } else {
                        Ok(None)
                    }
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    fn compile_expression(
        &mut self,
        expr: &Expression,
        function: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>, CodegenError> {
        match &expr.kind {
            ExpressionKind::IntegerLiteral(v) => {
                // Default to i16 (INT) — binary ops will widen as needed
                Ok(Some(
                    self.context
                        .i16_type()
                        .const_int(*v as u64, true)
                        .into(),
                ))
            }
            ExpressionKind::RealLiteral(v) => {
                // Default to f32 (REAL) to match common IEC usage
                Ok(Some(self.context.f32_type().const_float(*v).into()))
            }
            ExpressionKind::BoolLiteral(v) => {
                Ok(Some(
                    self.context
                        .i8_type()
                        .const_int(*v as u64, false)
                        .into(),
                ))
            }
            ExpressionKind::Identifier(ident) => {
                if let Some((ptr, ty)) = self.variables.get(&ident.name.to_uppercase()).cloned() {
                    let llvm_ty = self.iec_to_llvm_type(&ty);
                    let val = self
                        .builder
                        .build_load(llvm_ty, ptr, &ident.name)
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    Ok(Some(val))
                } else {
                    Ok(None)
                }
            }
            ExpressionKind::BinaryOp { op, left, right } => {
                let lhs = self.compile_expression(left, function)?;
                let rhs = self.compile_expression(right, function)?;
                match (lhs, rhs) {
                    (Some(l), Some(r)) => {
                        let result = self.compile_binary_op(*op, l, r)?;
                        Ok(Some(result))
                    }
                    _ => Ok(None),
                }
            }
            ExpressionKind::UnaryOp { op, operand } => {
                let val = self.compile_expression(operand, function)?;
                match val {
                    Some(v) => {
                        let result = self.compile_unary_op(*op, v)?;
                        Ok(Some(result))
                    }
                    None => Ok(None),
                }
            }
            ExpressionKind::Parenthesized(inner) => self.compile_expression(inner, function),
            ExpressionKind::FunctionCall { callee, args } => {
                if let ExpressionKind::Identifier(ident) = &callee.kind {
                    // Try standard library functions first (case-insensitive)
                    if let Some(result) =
                        self.compile_stdlib_call(&ident.name, args, function)?
                    {
                        return Ok(Some(result));
                    }

                    // Fall back to user-defined functions
                    if let Some(fn_val) =
                        self.module.get_function(&ident.name.to_lowercase())
                    {
                        let mut compiled_args = Vec::new();
                        for arg in args {
                            if let Some(val) =
                                self.compile_expression(&arg.value, function)?
                            {
                                compiled_args.push(val.into());
                            }
                        }
                        let call = self
                            .builder
                            .build_call(fn_val, &compiled_args, "call")
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                        Ok(match call.try_as_basic_value() {
                            inkwell::values::ValueKind::Basic(v) => Some(v),
                            inkwell::values::ValueKind::Instruction(_) => None,
                        })
                    } else {
                        Ok(None)
                    }
                } else {
                    Ok(None)
                }
            }
            ExpressionKind::TimeLiteral(s) => {
                let ns = parse_time_literal_ns(s);
                Ok(Some(self.context.i64_type().const_int(ns as u64, true).into()))
            }
            ExpressionKind::DateLiteral(_)
            | ExpressionKind::TodLiteral(_)
            | ExpressionKind::DtLiteral(_) => {
                // Date/time-of-day literals — store as i64 placeholder
                Ok(Some(self.context.i64_type().const_int(0, false).into()))
            }
            ExpressionKind::StringLiteral(_)
            | ExpressionKind::WstringLiteral(_) => {
                // String literals — not yet supported in codegen
                Ok(None)
            }
            ExpressionKind::DirectVariable(_) => {
                // Direct variables (%I, %Q, %M) resolved at link time
                Ok(None)
            }
            ExpressionKind::ArrayIndex { array, indices } => {
                // Get the element pointer via lvalue, then load from it
                if let Some(elem_ptr) = self.compile_lvalue_with_fn(expr, function)? {
                    // Determine the element type from the array's IEC type
                    if let ExpressionKind::Identifier(ident) = &array.kind {
                        if let Some((_, iec_ty)) = self.variables.get(&ident.name.to_uppercase()).cloned() {
                            if let IecType::Array { element_type, .. } = &iec_ty {
                                let elem_llvm_ty = self.iec_to_llvm_type(element_type);
                                let val = self
                                    .builder
                                    .build_load(elem_llvm_ty, elem_ptr, "arr_load")
                                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                                return Ok(Some(val));
                            }
                        }
                    }
                    Ok(None)
                } else {
                    Ok(None)
                }
            }
            ExpressionKind::MemberAccess { object, member } => {
                if let ExpressionKind::Identifier(ident) = &object.kind {
                    // Check if this is an FB instance field access
                    if let Some(info) = self.fb_instances.get(&ident.name.to_uppercase()).cloned() {
                        let parent_struct_type = self.current_struct_type.ok_or_else(|| {
                            CodegenError::LlvmError("no parent struct type".into())
                        })?;
                        let parent_state_ptr = self.current_state_ptr.ok_or_else(|| {
                            CodegenError::LlvmError("no parent state pointer".into())
                        })?;
                        let fb_ptr = self
                            .builder
                            .build_struct_gep(parent_struct_type, parent_state_ptr, info.field_index, &format!("{}_fb", ident.name))
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

                        let field_idx = info
                            .fields
                            .iter()
                            .position(|(name, _)| name.eq_ignore_ascii_case(&member.name))
                            .ok_or_else(|| {
                                CodegenError::UndefinedVariable(format!(
                                    "{}.{}",
                                    ident.name, member.name
                                ))
                            })?;
                        let field_ty = &info.fields[field_idx].1;
                        let field_llvm_ty = self.iec_to_llvm_type(field_ty);
                        let field_ptr = self
                            .builder
                            .build_struct_gep(info.struct_type, fb_ptr, field_idx as u32, &member.name)
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                        let val = self
                            .builder
                            .build_load(field_llvm_ty, field_ptr, &format!("{}.{}", ident.name, member.name))
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                        return Ok(Some(val));
                    }
                }
                // Fall back to struct field access via lvalue
                if let Some(field_ptr) = self.compile_lvalue_with_fn(expr, function)? {
                    if let ExpressionKind::Identifier(ident) = &object.kind {
                        if let Some((_, iec_ty)) = self.variables.get(&ident.name.to_uppercase()).cloned() {
                            if let IecType::Struct { fields, .. } = &iec_ty {
                                if let Some((_, field_ty)) = fields
                                    .iter()
                                    .find(|(name, _)| name.eq_ignore_ascii_case(&member.name))
                                {
                                    let field_llvm_ty = self.iec_to_llvm_type(field_ty);
                                    let val = self
                                        .builder
                                        .build_load(field_llvm_ty, field_ptr, &member.name)
                                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                                    return Ok(Some(val));
                                }
                            }
                        }
                    }
                    Ok(None)
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    fn compile_binary_op(
        &self,
        op: BinaryOp,
        left: BasicValueEnum<'ctx>,
        right: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        // Check if we're dealing with integers or floats
        let is_float = left.is_float_value() || right.is_float_value();

        if is_float {
            // Determine the target float type (use the wider one, or f32 if both are ints)
            let target_fty = if left.is_float_value() && right.is_float_value() {
                let lw = left.into_float_value().get_type();
                let rw = right.into_float_value().get_type();
                // Compare bit widths: f32=32, f64=64
                if lw == self.context.f64_type() || rw == self.context.f64_type() {
                    self.context.f64_type()
                } else {
                    self.context.f32_type()
                }
            } else if left.is_float_value() {
                left.into_float_value().get_type()
            } else {
                right.into_float_value().get_type()
            };

            let l = if left.is_float_value() {
                let fv = left.into_float_value();
                if fv.get_type() != target_fty {
                    self.builder.build_float_ext(fv, target_fty, "fext")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                } else { fv }
            } else {
                self.builder
                    .build_signed_int_to_float(
                        left.into_int_value(),
                        target_fty,
                        "itof",
                    )
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
            };
            let r = if right.is_float_value() {
                let fv = right.into_float_value();
                if fv.get_type() != target_fty {
                    self.builder.build_float_ext(fv, target_fty, "fext")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                } else { fv }
            } else {
                self.builder
                    .build_signed_int_to_float(
                        right.into_int_value(),
                        target_fty,
                        "itof",
                    )
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
            };

            let result = match op {
                BinaryOp::Add => self
                    .builder
                    .build_float_add(l, r, "fadd")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
                BinaryOp::Sub => self
                    .builder
                    .build_float_sub(l, r, "fsub")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
                BinaryOp::Mul => self
                    .builder
                    .build_float_mul(l, r, "fmul")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
                BinaryOp::Div => self
                    .builder
                    .build_float_div(l, r, "fdiv")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
                BinaryOp::Equal => {
                    return Ok(self
                        .builder
                        .build_float_compare(FloatPredicate::OEQ, l, r, "feq")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into());
                }
                BinaryOp::NotEqual => {
                    return Ok(self
                        .builder
                        .build_float_compare(FloatPredicate::ONE, l, r, "fne")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into());
                }
                BinaryOp::Less => {
                    return Ok(self
                        .builder
                        .build_float_compare(FloatPredicate::OLT, l, r, "flt")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into());
                }
                BinaryOp::LessEqual => {
                    return Ok(self
                        .builder
                        .build_float_compare(FloatPredicate::OLE, l, r, "fle")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into());
                }
                BinaryOp::Greater => {
                    return Ok(self
                        .builder
                        .build_float_compare(FloatPredicate::OGT, l, r, "fgt")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into());
                }
                BinaryOp::GreaterEqual => {
                    return Ok(self
                        .builder
                        .build_float_compare(FloatPredicate::OGE, l, r, "fge")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into());
                }
                _ => {
                    return Err(CodegenError::UnsupportedType(format!(
                        "float op {:?}",
                        op
                    )));
                }
            };
            Ok(result.into())
        } else {
            let (l, r) = self.match_int_widths(left.into_int_value(), right.into_int_value())?;

            let result = match op {
                BinaryOp::Add => self
                    .builder
                    .build_int_add(l, r, "add")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
                BinaryOp::Sub => self
                    .builder
                    .build_int_sub(l, r, "sub")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
                BinaryOp::Mul => self
                    .builder
                    .build_int_mul(l, r, "mul")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
                BinaryOp::Div => self
                    .builder
                    .build_int_signed_div(l, r, "div")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
                BinaryOp::Mod => self
                    .builder
                    .build_int_signed_rem(l, r, "mod")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
                BinaryOp::And => self
                    .builder
                    .build_and(l, r, "and")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
                BinaryOp::Or => self
                    .builder
                    .build_or(l, r, "or")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
                BinaryOp::Xor => self
                    .builder
                    .build_xor(l, r, "xor")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
                BinaryOp::Equal => {
                    return Ok(self
                        .builder
                        .build_int_compare(IntPredicate::EQ, l, r, "eq")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into());
                }
                BinaryOp::NotEqual => {
                    return Ok(self
                        .builder
                        .build_int_compare(IntPredicate::NE, l, r, "ne")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into());
                }
                BinaryOp::Less => {
                    return Ok(self
                        .builder
                        .build_int_compare(IntPredicate::SLT, l, r, "lt")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into());
                }
                BinaryOp::LessEqual => {
                    return Ok(self
                        .builder
                        .build_int_compare(IntPredicate::SLE, l, r, "le")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into());
                }
                BinaryOp::Greater => {
                    return Ok(self
                        .builder
                        .build_int_compare(IntPredicate::SGT, l, r, "gt")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into());
                }
                BinaryOp::GreaterEqual => {
                    return Ok(self
                        .builder
                        .build_int_compare(IntPredicate::SGE, l, r, "ge")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into());
                }
                BinaryOp::Power => {
                    // Integer power — not directly supported, return left for now
                    l
                }
            };
            Ok(result.into())
        }
    }

    fn compile_unary_op(
        &self,
        op: UnaryOp,
        operand: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        match op {
            UnaryOp::Neg => {
                if operand.is_float_value() {
                    Ok(self
                        .builder
                        .build_float_neg(operand.into_float_value(), "fneg")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into())
                } else {
                    Ok(self
                        .builder
                        .build_int_neg(operand.into_int_value(), "neg")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into())
                }
            }
            UnaryOp::Not => {
                let int_val = operand.into_int_value();
                let bit_width = int_val.get_type().get_bit_width();
                if bit_width <= 8 {
                    // Boolean NOT: compare equal to zero, then extend back to i8
                    let zero = int_val.get_type().const_zero();
                    let is_zero = self.builder
                        .build_int_compare(IntPredicate::EQ, int_val, zero, "lnot")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    let result = self.builder
                        .build_int_z_extend(is_zero, int_val.get_type(), "lnot_ext")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    Ok(result.into())
                } else {
                    // Bitwise NOT for wider integer types (WORD, DWORD, etc.)
                    Ok(self.builder
                        .build_not(int_val, "not")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into())
                }
            }
        }
    }

    /// Get a reference to the LLVM module.
    pub fn module(&self) -> &Module<'ctx> {
        &self.module
    }

    /// Emit LLVM IR to a string.
    pub fn emit_ir(&self) -> String {
        self.module.print_to_string().to_string()
    }

    /// Write object file to disk.
    pub fn emit_object(&self, path: &Path, triple: &str) -> Result<(), CodegenError> {
        Target::initialize_all(&InitializationConfig::default());

        let target_triple = TargetTriple::create(triple);
        let target = Target::from_triple(&target_triple)
            .map_err(|e| CodegenError::TargetError(e.to_string()))?;
        let machine = target
            .create_target_machine(
                &target_triple,
                "generic",
                "",
                OptimizationLevel::Default,
                RelocMode::Default,
                CodeModel::Default,
            )
            .ok_or_else(|| CodegenError::TargetError("failed to create target machine".into()))?;

        machine
            .write_to_file(&self.module, FileType::Object, path)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        Ok(())
    }

    /// Write bitcode to disk.
    pub fn emit_bitcode(&self, path: &Path) -> bool {
        self.module.write_bitcode_to_path(path)
    }
}
