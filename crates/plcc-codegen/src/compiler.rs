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
use plcc_hir::check::TypeChecker;
use plcc_hir::types::{IecType, TypeRegistry};
use plcc_st::ast::*;
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;

/// Name of the generated function that initializes VAR_GLOBAL FB instances.
const GLOBALS_INIT_FN: &str = "plcc_globals_init";

/// Parse a TIME literal string (e.g., "T#100ms", "T#1s500ms", "T#1h30m") into nanoseconds.
fn parse_time_literal_ns(s: &str) -> i64 {
    let s = s.trim();
    // Strip the duration prefix. IEC 61131-3 Annex A B.1.2.3 allows T#, LT#,
    // TIME# and LTIME#; the lexer accepts all four, so all four must be
    // stripped here or the prefix letters would be misread as unit suffixes.
    // Longest first, so `LTIME#`/`TIME#` are not truncated to `LT#`/`T#`.
    let s = ["LTIME#", "TIME#", "LT#", "T#"]
        .iter()
        .find_map(|p| {
            (s.len() > p.len() && s[..p.len()].eq_ignore_ascii_case(p)).then(|| &s[p.len()..])
        })
        .unwrap_or(s);
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
            if unit.is_empty() {
                // `c` is neither a digit nor a letter (e.g. a stray `#` or `-`).
                // Consume it so the loop always makes progress — otherwise this
                // spins forever.
                chars.next();
                continue;
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
    /// A variable was declared with a type name that resolves to nothing: not an
    /// elementary type, not a user TYPE, and not a FUNCTION_BLOCK/CLASS in scope.
    ///
    /// This used to be swallowed — the declaration silently became an `i32` slot and
    /// every statement touching it was dropped from the generated code, so a program
    /// using `TON` without a `TON` definition compiled to an empty `scan()`. It is now
    /// a hard error.
    #[error(
        "unknown type `{type_name}` for variable `{var_name}` in {pou_kind} `{pou_name}` \
         (no elementary type, TYPE declaration, FUNCTION_BLOCK or CLASS with that name is \
         in scope; standard function blocks come from the bundled stdlib — check --stdlib)"
    )]
    UnknownType {
        pou_kind: &'static str,
        pou_name: String,
        var_name: String,
        type_name: String,
    },
    /// The generated module failed LLVM's own verifier.
    ///
    /// Structural IR defects (a terminator in the middle of a block, a block with no
    /// terminator, a type mismatch) are invisible at `OptimizationLevel::None` — the
    /// JIT the execution tests use will happily run past them — and only surface as a
    /// hang or a miscompile once an optimizing backend gets hold of the module. So the
    /// verifier runs at the end of every `compile()`, not just before object emission.
    #[error("generated LLVM module failed verification: {0}")]
    InvalidModule(String),
}

/// Information about a compiled method on an FB/Class.
#[derive(Clone, Debug)]
struct MethodInfo {
    /// The LLVM function name for this method (e.g. "counter_add").
    fn_name: String,
    /// Parameter names and types (excludes the implicit instance pointer).
    params: Vec<(String, IecType)>,
    /// Return type (Void if the method doesn't return a value).
    return_type: IecType,
}

/// Layout information for a compiled function block.
#[derive(Clone, Debug)]
struct FbLayout<'ctx> {
    struct_type: StructType<'ctx>,
    scan_fn_name: String,
    /// Name of the generated `<pou>_init` function that applies declared initial
    /// values to an instance (and recursively initializes nested FB instances).
    init_fn_name: String,
    /// Ordered field names and their IEC types (inputs, outputs, locals — all in declaration order).
    fields: Vec<(String, IecType)>,
    /// Compiled methods, keyed by uppercase method name.
    methods: HashMap<String, MethodInfo>,
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
    /// Compiled methods on this FB type.
    methods: HashMap<String, MethodInfo>,
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
            // The platform clock, exposed to ST. Returns LINT/TIME nanoseconds.
            // `MONOTONIC_NS` is the ST-facing spelling; `PLCC_MONOTONIC_NS` matches the
            // symbol name for anyone who prefers to be explicit about the import.
            "MONOTONIC_NS" | "PLCC_MONOTONIC_NS" => {
                if !arg_vals.is_empty() {
                    return Err(CodegenError::LlvmError(format!(
                        "{uname} takes no arguments, got {}",
                        arg_vals.len()
                    )));
                }
                let clock_fn = self.get_or_declare_monotonic_ns();
                let call = self
                    .builder
                    .build_call(clock_fn, &[], "monons")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                match call.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(v) => Ok(Some(v)),
                    inkwell::values::ValueKind::Instruction(_) => Err(CodegenError::LlvmError(
                        "plcc_monotonic_ns returned no value".into(),
                    )),
                }
            }
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
                    _ => {
                        return Err(CodegenError::LlvmError(
                            "expected return value from intrinsic".into(),
                        ));
                    }
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
                    _ => {
                        return Err(CodegenError::LlvmError(
                            "expected return value from intrinsic".into(),
                        ));
                    }
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
                        _ => {
                            return Err(CodegenError::LlvmError(
                                "expected return value from fabs intrinsic".into(),
                            ));
                        }
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
                    _ => {
                        return Err(CodegenError::LlvmError(
                            "expected return value from intrinsic".into(),
                        ));
                    }
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
                    _ => {
                        return Err(CodegenError::LlvmError(
                            "expected return value from intrinsic".into(),
                        ));
                    }
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
                    return Err(CodegenError::LlvmError("TRUNC expects 1 argument".into()));
                }
                let fv = self.ensure_float(arg_vals[0])?;
                let fty = fv.get_type();
                let intr = Intrinsic::find("llvm.trunc").ok_or_else(|| {
                    CodegenError::LlvmError("intrinsic llvm.trunc not found".into())
                })?;
                let fn_val = intr
                    .get_declaration(&self.module, &[fty.into()])
                    .ok_or_else(|| {
                        CodegenError::LlvmError("failed to get llvm.trunc declaration".into())
                    })?;
                let result = self
                    .builder
                    .build_call(fn_val, &[fv.into()], "trunc")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                    .try_as_basic_value();
                let result = match result {
                    inkwell::values::ValueKind::Basic(v) => v,
                    _ => {
                        return Err(CodegenError::LlvmError(
                            "expected return value from intrinsic".into(),
                        ));
                    }
                };
                Ok(Some(result))
            }

            // --- Trig functions (extern C library) ---
            "TAN" | "ASIN" | "ACOS" | "ATAN" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(format!(
                        "{uname} expects 1 argument, got {}",
                        arg_vals.len()
                    )));
                }
                let arg = self.ensure_float(arg_vals[0])?;
                let fty = arg.get_type();
                let is_f64 = fty == self.context.f64_type();
                let c_name = match uname.as_str() {
                    "TAN" => {
                        if is_f64 {
                            "tan"
                        } else {
                            "tanf"
                        }
                    }
                    "ASIN" => {
                        if is_f64 {
                            "asin"
                        } else {
                            "asinf"
                        }
                    }
                    "ACOS" => {
                        if is_f64 {
                            "acos"
                        } else {
                            "acosf"
                        }
                    }
                    "ATAN" => {
                        if is_f64 {
                            "atan"
                        } else {
                            "atanf"
                        }
                    }
                    _ => unreachable!(),
                };
                let fn_type = fty.fn_type(&[fty.into()], false);
                let ext_fn = self.module.get_function(c_name).unwrap_or_else(|| {
                    self.module.add_function(
                        c_name,
                        fn_type,
                        Some(inkwell::module::Linkage::External),
                    )
                });
                let result = self
                    .builder
                    .build_call(ext_fn, &[arg.into()], &uname.to_lowercase())
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                    .try_as_basic_value();
                let result = match result {
                    inkwell::values::ValueKind::Basic(v) => v,
                    _ => {
                        return Err(CodegenError::LlvmError(
                            "expected return value from extern trig fn".into(),
                        ));
                    }
                };
                Ok(Some(result))
            }

            "ATAN2" => {
                if arg_vals.len() != 2 {
                    return Err(CodegenError::LlvmError(format!(
                        "ATAN2 expects 2 arguments, got {}",
                        arg_vals.len()
                    )));
                }
                let y = self.ensure_float(arg_vals[0])?;
                let x = self.ensure_float(arg_vals[1])?;
                let (y, x) = self.match_float_widths(y, x)?;
                let fty = y.get_type();
                let is_f64 = fty == self.context.f64_type();
                let c_name = if is_f64 { "atan2" } else { "atan2f" };
                let fn_type = fty.fn_type(&[fty.into(), fty.into()], false);
                let ext_fn = self.module.get_function(c_name).unwrap_or_else(|| {
                    self.module.add_function(
                        c_name,
                        fn_type,
                        Some(inkwell::module::Linkage::External),
                    )
                });
                let result = self
                    .builder
                    .build_call(ext_fn, &[y.into(), x.into()], "atan2")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                    .try_as_basic_value();
                let result = match result {
                    inkwell::values::ValueKind::Basic(v) => v,
                    _ => {
                        return Err(CodegenError::LlvmError(
                            "expected return value from atan2".into(),
                        ));
                    }
                };
                Ok(Some(result))
            }

            "LOG" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(format!(
                        "LOG expects 1 argument, got {}",
                        arg_vals.len()
                    )));
                }
                let arg = self.ensure_float(arg_vals[0])?;
                let fty = arg.get_type();
                let intr = Intrinsic::find("llvm.log10").ok_or_else(|| {
                    CodegenError::LlvmError("intrinsic llvm.log10 not found".into())
                })?;
                let fn_val = intr
                    .get_declaration(&self.module, &[fty.into()])
                    .ok_or_else(|| {
                        CodegenError::LlvmError("failed to get llvm.log10 declaration".into())
                    })?;
                let result = self
                    .builder
                    .build_call(fn_val, &[arg.into()], "log")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                    .try_as_basic_value();
                let result = match result {
                    inkwell::values::ValueKind::Basic(v) => v,
                    _ => {
                        return Err(CodegenError::LlvmError(
                            "expected return value from log10 intrinsic".into(),
                        ));
                    }
                };
                Ok(Some(result))
            }

            // --- Rounding functions (LLVM intrinsics) ---
            "FLOOR" | "CEIL" | "ROUND" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(format!(
                        "{uname} expects 1 argument, got {}",
                        arg_vals.len()
                    )));
                }
                let arg = self.ensure_float(arg_vals[0])?;
                let fty = arg.get_type();
                let intrinsic_name = match uname.as_str() {
                    "FLOOR" => "llvm.floor",
                    "CEIL" => "llvm.ceil",
                    "ROUND" => "llvm.round",
                    _ => unreachable!(),
                };
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
                    _ => {
                        return Err(CodegenError::LlvmError(
                            "expected return value from rounding intrinsic".into(),
                        ));
                    }
                };
                Ok(Some(result))
            }

            // --- Integer widening (sign-extend) ---
            "SINT_TO_INT" | "SINT_TO_DINT" | "SINT_TO_LINT" | "INT_TO_LINT" | "DINT_TO_LINT" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(format!(
                        "{uname} expects 1 argument"
                    )));
                }
                let iv = arg_vals[0].into_int_value();
                let target = match uname.as_str() {
                    "SINT_TO_INT" => self.context.i16_type(),
                    "SINT_TO_DINT" => self.context.i32_type(),
                    "SINT_TO_LINT" | "INT_TO_LINT" | "DINT_TO_LINT" => self.context.i64_type(),
                    _ => unreachable!(),
                };
                let result = self
                    .builder
                    .build_int_s_extend(iv, target, &uname.to_lowercase())
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(Some(result.into()))
            }

            // --- Integer widening (zero-extend) ---
            "USINT_TO_UINT" | "USINT_TO_UDINT" | "USINT_TO_ULINT" | "UINT_TO_UDINT"
            | "UINT_TO_ULINT" | "UDINT_TO_ULINT" | "BYTE_TO_WORD" | "BYTE_TO_DWORD"
            | "BYTE_TO_INT" | "WORD_TO_DWORD" | "WORD_TO_DINT" | "DWORD_TO_LINT" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(format!(
                        "{uname} expects 1 argument"
                    )));
                }
                let iv = arg_vals[0].into_int_value();
                let target = match uname.as_str() {
                    "USINT_TO_UINT" | "BYTE_TO_WORD" | "BYTE_TO_INT" => self.context.i16_type(),
                    "USINT_TO_UDINT" | "UINT_TO_UDINT" | "BYTE_TO_DWORD" | "WORD_TO_DWORD"
                    | "WORD_TO_DINT" => self.context.i32_type(),
                    "USINT_TO_ULINT" | "UINT_TO_ULINT" | "UDINT_TO_ULINT" | "DWORD_TO_LINT" => {
                        self.context.i64_type()
                    }
                    _ => unreachable!(),
                };
                let result = self
                    .builder
                    .build_int_z_extend(iv, target, &uname.to_lowercase())
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(Some(result.into()))
            }

            // --- Same-size reinterpret (noop / bitcast for same-width int types) ---
            "WORD_TO_INT" | "DWORD_TO_DINT" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(format!(
                        "{uname} expects 1 argument"
                    )));
                }
                // Same bit-width, just return the value as-is
                Ok(Some(arg_vals[0]))
            }

            // --- Integer narrowing (truncate) ---
            "LINT_TO_INT" | "LINT_TO_DINT" | "UDINT_TO_UINT" | "ULINT_TO_UINT"
            | "ULINT_TO_UDINT" | "DWORD_TO_INT" | "INT_TO_BYTE" | "DINT_TO_BYTE"
            | "INT_TO_SINT" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(format!(
                        "{uname} expects 1 argument"
                    )));
                }
                let iv = arg_vals[0].into_int_value();
                let target = match uname.as_str() {
                    "INT_TO_BYTE" | "DINT_TO_BYTE" | "INT_TO_SINT" => self.context.i8_type(),
                    "LINT_TO_INT" | "UDINT_TO_UINT" | "ULINT_TO_UINT" | "DWORD_TO_INT" => {
                        self.context.i16_type()
                    }
                    "LINT_TO_DINT" | "ULINT_TO_UDINT" => self.context.i32_type(),
                    _ => unreachable!(),
                };
                let result = self
                    .builder
                    .build_int_truncate(iv, target, &uname.to_lowercase())
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(Some(result.into()))
            }

            // --- Float conversions ---
            "REAL_TO_LREAL" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(
                        "REAL_TO_LREAL expects 1 argument".into(),
                    ));
                }
                let fv = arg_vals[0].into_float_value();
                let result = self
                    .builder
                    .build_float_ext(fv, self.context.f64_type(), "real_to_lreal")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(Some(result.into()))
            }
            "LREAL_TO_REAL" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(
                        "LREAL_TO_REAL expects 1 argument".into(),
                    ));
                }
                let fv = arg_vals[0].into_float_value();
                let result = self
                    .builder
                    .build_float_trunc(fv, self.context.f32_type(), "lreal_to_real")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(Some(result.into()))
            }

            // --- Float to signed int ---
            "REAL_TO_LINT" | "LREAL_TO_INT" | "LREAL_TO_DINT" | "LREAL_TO_LINT" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(format!(
                        "{uname} expects 1 argument"
                    )));
                }
                let fv = arg_vals[0].into_float_value();
                let target = match uname.as_str() {
                    "LREAL_TO_INT" => self.context.i16_type(),
                    "LREAL_TO_DINT" => self.context.i32_type(),
                    "REAL_TO_LINT" | "LREAL_TO_LINT" => self.context.i64_type(),
                    _ => unreachable!(),
                };
                let result = self
                    .builder
                    .build_float_to_signed_int(fv, target, &uname.to_lowercase())
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(Some(result.into()))
            }

            // --- Signed int to float ---
            "LINT_TO_REAL" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(
                        "LINT_TO_REAL expects 1 argument".into(),
                    ));
                }
                let iv = arg_vals[0].into_int_value();
                let result = self
                    .builder
                    .build_signed_int_to_float(iv, self.context.f32_type(), "lint_to_real")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(Some(result.into()))
            }

            // --- Unsigned int to float ---
            "ULINT_TO_REAL" | "UINT_TO_REAL" | "UDINT_TO_REAL" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(format!(
                        "{uname} expects 1 argument"
                    )));
                }
                let iv = arg_vals[0].into_int_value();
                let result = self
                    .builder
                    .build_unsigned_int_to_float(iv, self.context.f32_type(), &uname.to_lowercase())
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(Some(result.into()))
            }

            // --- Bool conversions (zext from i8) ---
            "BOOL_TO_BYTE" | "BOOL_TO_WORD" | "BOOL_TO_DINT" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(format!(
                        "{uname} expects 1 argument"
                    )));
                }
                let iv = arg_vals[0].into_int_value();
                let target = match uname.as_str() {
                    "BOOL_TO_BYTE" => self.context.i8_type(),
                    "BOOL_TO_WORD" => self.context.i16_type(),
                    "BOOL_TO_DINT" => self.context.i32_type(),
                    _ => unreachable!(),
                };
                let result = self
                    .builder
                    .build_int_z_extend(iv, target, &uname.to_lowercase())
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(Some(result.into()))
            }

            // --- To-bool conversions (compare != 0, zext result to i8) ---
            "INT_TO_BOOL" | "DINT_TO_BOOL" | "BYTE_TO_BOOL" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(format!(
                        "{uname} expects 1 argument"
                    )));
                }
                let iv = arg_vals[0].into_int_value();
                let zero = iv.get_type().const_zero();
                let cmp = self
                    .builder
                    .build_int_compare(IntPredicate::NE, iv, zero, "to_bool_cmp")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                let result = self
                    .builder
                    .build_int_z_extend(cmp, self.context.i8_type(), &uname.to_lowercase())
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(Some(result.into()))
            }

            "LEN" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError("LEN expects 1 argument".into()));
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

            "FIND" => {
                if arg_vals.len() != 2 {
                    return Err(CodegenError::LlvmError("FIND expects 2 arguments".into()));
                }
                // FIND(s1, s2) — returns 1-based position of s2 in s1, 0 if not found.
                // Need pointers to the string arrays.
                if let (Some(s1_ptr), Some(s2_ptr)) = (
                    self.compile_lvalue_with_fn(&args[0].value, function)?,
                    self.compile_lvalue_with_fn(&args[1].value, function)?,
                ) {
                    let find_fn = self.get_or_create_find_fn();
                    let result = self
                        .builder
                        .build_call(find_fn, &[s1_ptr.into(), s2_ptr.into()], "find_result")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .try_as_basic_value();
                    match result {
                        inkwell::values::ValueKind::Basic(v) => Ok(Some(v)),
                        _ => Err(CodegenError::LlvmError(
                            "FIND: expected return value".into(),
                        )),
                    }
                } else {
                    Ok(Some(self.context.i16_type().const_zero().into()))
                }
            }

            // Date/time arithmetic — TIME values are i64 (nanoseconds)
            "ADD_TIME" => {
                if arg_vals.len() != 2 {
                    return Err(CodegenError::LlvmError(
                        "ADD_TIME expects 2 arguments".into(),
                    ));
                }
                let t1 = arg_vals[0].into_int_value();
                let t2 = arg_vals[1].into_int_value();
                let result = self
                    .builder
                    .build_int_add(t1, t2, "add_time")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(Some(result.into()))
            }
            "SUB_TIME" => {
                if arg_vals.len() != 2 {
                    return Err(CodegenError::LlvmError(
                        "SUB_TIME expects 2 arguments".into(),
                    ));
                }
                let t1 = arg_vals[0].into_int_value();
                let t2 = arg_vals[1].into_int_value();
                let result = self
                    .builder
                    .build_int_sub(t1, t2, "sub_time")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(Some(result.into()))
            }
            "MUL_TIME" => {
                if arg_vals.len() != 2 {
                    return Err(CodegenError::LlvmError(
                        "MUL_TIME expects 2 arguments".into(),
                    ));
                }
                let t1 = arg_vals[0].into_int_value();
                let factor = arg_vals[1].into_int_value();
                // Extend factor to i64 if needed
                let i64_ty = self.context.i64_type();
                let factor_i64 = if factor.get_type().get_bit_width() < 64 {
                    self.builder
                        .build_int_s_extend(factor, i64_ty, "factor_ext")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                } else {
                    factor
                };
                let result = self
                    .builder
                    .build_int_mul(t1, factor_i64, "mul_time")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(Some(result.into()))
            }
            "DIV_TIME" => {
                if arg_vals.len() != 2 {
                    return Err(CodegenError::LlvmError(
                        "DIV_TIME expects 2 arguments".into(),
                    ));
                }
                let t1 = arg_vals[0].into_int_value();
                let divisor = arg_vals[1].into_int_value();
                // Extend divisor to i64 if needed
                let i64_ty = self.context.i64_type();
                let divisor_i64 = if divisor.get_type().get_bit_width() < 64 {
                    self.builder
                        .build_int_s_extend(divisor, i64_ty, "divisor_ext")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                } else {
                    divisor
                };
                let result = self
                    .builder
                    .build_int_signed_div(t1, divisor_i64, "div_time")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(Some(result.into()))
            }

            // String functions that return STRING are handled as special cases
            // in compile_statement (Assignment), not here. CONCAT, LEFT, RIGHT, MID
            // need a destination pointer which is only available at the assignment level.
            "CONCAT" | "LEFT" | "RIGHT" | "MID" => {
                // Return None here — handled in compile_string_assignment
                Ok(None)
            }

            _ => Ok(None),
        }
    }

    /// Get or declare the extern `plcc_print(i8*)` function.
    /// This is provided by the firmware runtime (writes to debug UART).
    fn get_or_declare_plcc_print(&self) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function("plcc_print") {
            return f;
        }
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let void_ty = self.context.void_type();
        let fn_type = void_ty.fn_type(&[ptr_ty.into()], false);
        self.module.add_function(
            "plcc_print",
            fn_type,
            Some(inkwell::module::Linkage::External),
        )
    }

    /// Get or declare the extern `plcc_monotonic_ns() -> i64` function.
    ///
    /// This is the one time source the compiler needs and the language cannot
    /// provide. The platform supplies it, exactly like `plcc_print`:
    ///   * `plcc sim` / the JIT map it onto a host `std::time::Instant` clock;
    ///   * bare-metal integrators must export a `plcc_monotonic_ns` symbol
    ///     (e.g. from a cycle counter or SysTick).
    ///
    /// Signed i64, not u64: TIME and LTIME are already laid out as i64 nanoseconds
    /// by `iec_to_llvm_type`, and every arithmetic and comparison path in codegen
    /// treats those as signed. Returning u64 would mean the very first thing any
    /// timer did with the value was a signedness reinterpretation. i64 nanoseconds
    /// still spans ~292 years of uptime, which is not a real constraint.
    ///
    /// The value is nanoseconds since an arbitrary, fixed epoch. Only differences
    /// are meaningful. It must never go backwards.
    pub const MONOTONIC_NS_SYMBOL: &'static str = "plcc_monotonic_ns";

    fn get_or_declare_monotonic_ns(&self) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function(Self::MONOTONIC_NS_SYMBOL) {
            return f;
        }
        let fn_type = self.context.i64_type().fn_type(&[], false);
        self.module.add_function(
            Self::MONOTONIC_NS_SYMBOL,
            fn_type,
            Some(inkwell::module::Linkage::External),
        )
    }

    /// Compile a PRINT('literal') or PRINT(string_var) call.
    /// Emits: call void @plcc_print(ptr)
    fn compile_print_call(
        &mut self,
        args: &[CallArg],
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        if args.len() != 1 {
            return Err(CodegenError::LlvmError(format!(
                "PRINT expects 1 argument, got {}",
                args.len()
            )));
        }

        let print_fn = self.get_or_declare_plcc_print();
        let arg_expr = &args[0].value;

        // Check if the argument is a string literal
        let str_ptr = match &arg_expr.kind {
            ExpressionKind::StringLiteral(s) | ExpressionKind::WstringLiteral(s) => {
                // Create a global constant string and get a pointer to it
                let bytes = s.as_bytes();
                let i8_ty = self.context.i8_type();
                let arr_ty = i8_ty.array_type((bytes.len() + 1) as u32);
                let mut vals: Vec<inkwell::values::IntValue> = bytes
                    .iter()
                    .map(|&b| i8_ty.const_int(b as u64, false))
                    .collect();
                vals.push(i8_ty.const_zero()); // null terminator
                let const_arr = i8_ty.const_array(&vals);
                let global = self.module.add_global(arr_ty, None, "print_str");
                global.set_initializer(&const_arr);
                global.set_constant(true);
                global.as_pointer_value()
            }
            _ => {
                // Assume it's a STRING variable — get its pointer via lvalue
                self.compile_lvalue_with_fn(arg_expr, function)?
                    .ok_or_else(|| {
                        CodegenError::LlvmError(
                            "PRINT: argument must be a string literal or STRING variable".into(),
                        )
                    })?
            }
        };

        self.builder
            .build_call(print_fn, &[str_ptr.into()], "")
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        Ok(())
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
        self.builder
            .build_store(counter, i16_ty.const_zero())
            .unwrap();
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        self.builder.position_at_end(loop_bb);
        let str_ptr = function.get_nth_param(0).unwrap().into_pointer_value();
        let idx = self
            .builder
            .build_load(i16_ty, counter, "idx")
            .unwrap()
            .into_int_value();
        let idx_i64 = self
            .builder
            .build_int_s_extend(idx, self.context.i64_type(), "idx64")
            .unwrap();
        let char_ptr = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, str_ptr, &[idx_i64], "char_ptr")
                .unwrap()
        };
        let ch = self
            .builder
            .build_load(i8_ty, char_ptr, "ch")
            .unwrap()
            .into_int_value();
        let is_null = self
            .builder
            .build_int_compare(IntPredicate::EQ, ch, i8_ty.const_zero(), "is_null")
            .unwrap();
        self.builder
            .build_conditional_branch(is_null, done_bb, inc_bb)
            .unwrap();

        self.builder.position_at_end(inc_bb);
        let cur = self
            .builder
            .build_load(i16_ty, counter, "cur")
            .unwrap()
            .into_int_value();
        let next = self
            .builder
            .build_int_add(cur, i16_ty.const_int(1, false), "next")
            .unwrap();
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

    /// Get or create a `plcc_find` helper function.
    /// Signature: i16 plcc_find(ptr haystack, ptr needle) — returns 1-based position, 0 if not found.
    fn get_or_create_find_fn(&self) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function("plcc_find") {
            return f;
        }

        let i16_ty = self.context.i16_type();
        let i8_ty = self.context.i8_type();
        let i64_ty = self.context.i64_type();
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let fn_type = i16_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false);
        let function = self.module.add_function("plcc_find", fn_type, None);

        let saved_block = self.builder.get_insert_block();

        let entry = self.context.append_basic_block(function, "entry");
        let outer_loop = self.context.append_basic_block(function, "outer_loop");
        let inner_loop = self.context.append_basic_block(function, "inner_loop");
        let inner_check = self.context.append_basic_block(function, "inner_check");
        let found_bb = self.context.append_basic_block(function, "found");
        let next_bb = self.context.append_basic_block(function, "next");
        let not_found_bb = self.context.append_basic_block(function, "not_found");

        let haystack = function.get_nth_param(0).unwrap().into_pointer_value();
        let needle = function.get_nth_param(1).unwrap().into_pointer_value();

        // entry: alloca i, j
        self.builder.position_at_end(entry);
        let i_ptr = self.builder.build_alloca(i16_ty, "i").unwrap();
        let j_ptr = self.builder.build_alloca(i16_ty, "j").unwrap();
        self.builder
            .build_store(i_ptr, i16_ty.const_zero())
            .unwrap();
        self.builder.build_unconditional_branch(outer_loop).unwrap();

        // outer_loop: check haystack[i] != 0
        self.builder.position_at_end(outer_loop);
        let i_val = self
            .builder
            .build_load(i16_ty, i_ptr, "i")
            .unwrap()
            .into_int_value();
        let i_64 = self
            .builder
            .build_int_s_extend(i_val, i64_ty, "i64")
            .unwrap();
        let h_ptr = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, haystack, &[i_64], "hp")
                .unwrap()
        };
        let h_ch = self
            .builder
            .build_load(i8_ty, h_ptr, "hch")
            .unwrap()
            .into_int_value();
        let h_null = self
            .builder
            .build_int_compare(IntPredicate::EQ, h_ch, i8_ty.const_zero(), "hnull")
            .unwrap();
        self.builder
            .build_conditional_branch(h_null, not_found_bb, inner_loop)
            .unwrap();

        // inner_loop: reset j=0, start matching
        self.builder.position_at_end(inner_loop);
        self.builder
            .build_store(j_ptr, i16_ty.const_zero())
            .unwrap();
        self.builder
            .build_unconditional_branch(inner_check)
            .unwrap();

        // inner_check: compare haystack[i+j] == needle[j]
        self.builder.position_at_end(inner_check);
        let i2 = self
            .builder
            .build_load(i16_ty, i_ptr, "i2")
            .unwrap()
            .into_int_value();
        let j2 = self
            .builder
            .build_load(i16_ty, j_ptr, "j2")
            .unwrap()
            .into_int_value();
        // Check needle[j] == 0 => found
        let j2_64 = self.builder.build_int_s_extend(j2, i64_ty, "j64").unwrap();
        let n_ptr = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, needle, &[j2_64], "np")
                .unwrap()
        };
        let n_ch = self
            .builder
            .build_load(i8_ty, n_ptr, "nch")
            .unwrap()
            .into_int_value();
        let n_null = self
            .builder
            .build_int_compare(IntPredicate::EQ, n_ch, i8_ty.const_zero(), "nnull")
            .unwrap();
        self.builder
            .build_conditional_branch(n_null, found_bb, next_bb)
            .unwrap();

        // next: compare chars, if mismatch goto outer_loop i++, else j++ and inner_check
        self.builder.position_at_end(next_bb);
        let ij = self.builder.build_int_add(i2, j2, "ij").unwrap();
        let ij_64 = self.builder.build_int_s_extend(ij, i64_ty, "ij64").unwrap();
        let hp2 = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, haystack, &[ij_64], "hp2")
                .unwrap()
        };
        let hc2 = self
            .builder
            .build_load(i8_ty, hp2, "hc2")
            .unwrap()
            .into_int_value();
        let match_cmp = self
            .builder
            .build_int_compare(IntPredicate::EQ, hc2, n_ch, "mcmp")
            .unwrap();

        let j_inc_bb = self.context.append_basic_block(function, "j_inc");
        let i_inc_bb = self.context.append_basic_block(function, "i_inc");
        self.builder
            .build_conditional_branch(match_cmp, j_inc_bb, i_inc_bb)
            .unwrap();

        // j_inc
        self.builder.position_at_end(j_inc_bb);
        let j3 = self
            .builder
            .build_load(i16_ty, j_ptr, "j3")
            .unwrap()
            .into_int_value();
        let j_next = self
            .builder
            .build_int_add(j3, i16_ty.const_int(1, false), "jnext")
            .unwrap();
        self.builder.build_store(j_ptr, j_next).unwrap();
        self.builder
            .build_unconditional_branch(inner_check)
            .unwrap();

        // i_inc
        self.builder.position_at_end(i_inc_bb);
        let i3 = self
            .builder
            .build_load(i16_ty, i_ptr, "i3")
            .unwrap()
            .into_int_value();
        let i_next = self
            .builder
            .build_int_add(i3, i16_ty.const_int(1, false), "inext")
            .unwrap();
        self.builder.build_store(i_ptr, i_next).unwrap();
        self.builder.build_unconditional_branch(outer_loop).unwrap();

        // found: return i + 1 (1-based)
        self.builder.position_at_end(found_bb);
        let i_final = self
            .builder
            .build_load(i16_ty, i_ptr, "ifinal")
            .unwrap()
            .into_int_value();
        let pos = self
            .builder
            .build_int_add(i_final, i16_ty.const_int(1, false), "pos")
            .unwrap();
        self.builder.build_return(Some(&pos)).unwrap();

        // not_found: return 0
        self.builder.position_at_end(not_found_bb);
        self.builder
            .build_return(Some(&i16_ty.const_zero()))
            .unwrap();

        if let Some(bb) = saved_block {
            self.builder.position_at_end(bb);
        }

        function
    }

    /// Get or create `plcc_concat(dest: *i8, s1: *i8, s2: *i8, max_len: i32)`.
    /// Copies s1 then s2 into dest, null-terminates, respecting max_len.
    fn get_or_create_concat_fn(&self) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function("plcc_concat") {
            return f;
        }

        let void_ty = self.context.void_type();
        let i8_ty = self.context.i8_type();
        let i32_ty = self.context.i32_type();
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let fn_type = void_ty.fn_type(
            &[ptr_ty.into(), ptr_ty.into(), ptr_ty.into(), i32_ty.into()],
            false,
        );
        let function = self.module.add_function("plcc_concat", fn_type, None);

        let saved_block = self.builder.get_insert_block();

        let entry = self.context.append_basic_block(function, "entry");
        let copy1_loop = self.context.append_basic_block(function, "copy1_loop");
        let copy1_body = self.context.append_basic_block(function, "copy1_body");
        let copy2_start = self.context.append_basic_block(function, "copy2_start");
        let copy2_loop = self.context.append_basic_block(function, "copy2_loop");
        let copy2_body = self.context.append_basic_block(function, "copy2_body");
        let done = self.context.append_basic_block(function, "done");

        let dest = function.get_nth_param(0).unwrap().into_pointer_value();
        let s1 = function.get_nth_param(1).unwrap().into_pointer_value();
        let s2 = function.get_nth_param(2).unwrap().into_pointer_value();
        let max_len = function.get_nth_param(3).unwrap().into_int_value();

        // entry: alloca dest_idx, src_idx
        self.builder.position_at_end(entry);
        let dest_idx = self.builder.build_alloca(i32_ty, "dest_idx").unwrap();
        let src_idx = self.builder.build_alloca(i32_ty, "src_idx").unwrap();
        self.builder
            .build_store(dest_idx, i32_ty.const_zero())
            .unwrap();
        self.builder
            .build_store(src_idx, i32_ty.const_zero())
            .unwrap();
        let max_minus1 = self
            .builder
            .build_int_sub(max_len, i32_ty.const_int(1, false), "max_m1")
            .unwrap();
        self.builder.build_unconditional_branch(copy1_loop).unwrap();

        // copy1_loop: check s1[src_idx] != 0 && dest_idx < max_len - 1
        self.builder.position_at_end(copy1_loop);
        let si = self
            .builder
            .build_load(i32_ty, src_idx, "si")
            .unwrap()
            .into_int_value();
        let si_i64 = self
            .builder
            .build_int_s_extend(si, self.context.i64_type(), "si64")
            .unwrap();
        let ch_ptr = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, s1, &[si_i64], "ch_ptr")
                .unwrap()
        };
        let ch = self
            .builder
            .build_load(i8_ty, ch_ptr, "ch")
            .unwrap()
            .into_int_value();
        let not_null = self
            .builder
            .build_int_compare(IntPredicate::NE, ch, i8_ty.const_zero(), "not_null")
            .unwrap();
        let di = self
            .builder
            .build_load(i32_ty, dest_idx, "di")
            .unwrap()
            .into_int_value();
        let in_bounds = self
            .builder
            .build_int_compare(IntPredicate::SLT, di, max_minus1, "in_bounds")
            .unwrap();
        let cont = self.builder.build_and(not_null, in_bounds, "cont").unwrap();
        self.builder
            .build_conditional_branch(cont, copy1_body, copy2_start)
            .unwrap();

        // copy1_body: dest[dest_idx] = ch; dest_idx++; src_idx++
        self.builder.position_at_end(copy1_body);
        let di2 = self
            .builder
            .build_load(i32_ty, dest_idx, "di2")
            .unwrap()
            .into_int_value();
        let di2_i64 = self
            .builder
            .build_int_s_extend(di2, self.context.i64_type(), "di264")
            .unwrap();
        let dest_ch_ptr = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, dest, &[di2_i64], "dest_ch")
                .unwrap()
        };
        self.builder.build_store(dest_ch_ptr, ch).unwrap();
        let di_next = self
            .builder
            .build_int_add(di2, i32_ty.const_int(1, false), "di_next")
            .unwrap();
        self.builder.build_store(dest_idx, di_next).unwrap();
        let si_next = self
            .builder
            .build_int_add(si, i32_ty.const_int(1, false), "si_next")
            .unwrap();
        self.builder.build_store(src_idx, si_next).unwrap();
        self.builder.build_unconditional_branch(copy1_loop).unwrap();

        // copy2_start: reset src_idx for s2
        self.builder.position_at_end(copy2_start);
        self.builder
            .build_store(src_idx, i32_ty.const_zero())
            .unwrap();
        self.builder.build_unconditional_branch(copy2_loop).unwrap();

        // copy2_loop: check s2[src_idx] != 0 && dest_idx < max_len - 1
        self.builder.position_at_end(copy2_loop);
        let si3 = self
            .builder
            .build_load(i32_ty, src_idx, "si3")
            .unwrap()
            .into_int_value();
        let si3_i64 = self
            .builder
            .build_int_s_extend(si3, self.context.i64_type(), "si364")
            .unwrap();
        let ch2_ptr = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, s2, &[si3_i64], "ch2_ptr")
                .unwrap()
        };
        let ch2 = self
            .builder
            .build_load(i8_ty, ch2_ptr, "ch2")
            .unwrap()
            .into_int_value();
        let not_null2 = self
            .builder
            .build_int_compare(IntPredicate::NE, ch2, i8_ty.const_zero(), "not_null2")
            .unwrap();
        let di3 = self
            .builder
            .build_load(i32_ty, dest_idx, "di3")
            .unwrap()
            .into_int_value();
        let in_bounds2 = self
            .builder
            .build_int_compare(IntPredicate::SLT, di3, max_minus1, "in_bounds2")
            .unwrap();
        let cont2 = self
            .builder
            .build_and(not_null2, in_bounds2, "cont2")
            .unwrap();
        self.builder
            .build_conditional_branch(cont2, copy2_body, done)
            .unwrap();

        // copy2_body: dest[dest_idx] = ch2; dest_idx++; src_idx++
        self.builder.position_at_end(copy2_body);
        let di4 = self
            .builder
            .build_load(i32_ty, dest_idx, "di4")
            .unwrap()
            .into_int_value();
        let di4_i64 = self
            .builder
            .build_int_s_extend(di4, self.context.i64_type(), "di464")
            .unwrap();
        let dest_ch2_ptr = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, dest, &[di4_i64], "dest_ch2")
                .unwrap()
        };
        self.builder.build_store(dest_ch2_ptr, ch2).unwrap();
        let di4_next = self
            .builder
            .build_int_add(di4, i32_ty.const_int(1, false), "di4_next")
            .unwrap();
        self.builder.build_store(dest_idx, di4_next).unwrap();
        let si3_next = self
            .builder
            .build_int_add(si3, i32_ty.const_int(1, false), "si3_next")
            .unwrap();
        self.builder.build_store(src_idx, si3_next).unwrap();
        self.builder.build_unconditional_branch(copy2_loop).unwrap();

        // done: null-terminate
        self.builder.position_at_end(done);
        let final_di = self
            .builder
            .build_load(i32_ty, dest_idx, "final_di")
            .unwrap()
            .into_int_value();
        let final_di_i64 = self
            .builder
            .build_int_s_extend(final_di, self.context.i64_type(), "fdi64")
            .unwrap();
        let null_ptr = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, dest, &[final_di_i64], "null_ptr")
                .unwrap()
        };
        self.builder
            .build_store(null_ptr, i8_ty.const_zero())
            .unwrap();
        self.builder.build_return(None).unwrap();

        if let Some(bb) = saved_block {
            self.builder.position_at_end(bb);
        }
        function
    }

    /// Get or create `plcc_left(dest: *i8, src: *i8, n: i32, max_len: i32)`.
    /// Copies first min(n, strlen(src)) characters from src to dest.
    fn get_or_create_left_fn(&self) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function("plcc_left") {
            return f;
        }

        let void_ty = self.context.void_type();
        let i8_ty = self.context.i8_type();
        let i32_ty = self.context.i32_type();
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let fn_type = void_ty.fn_type(
            &[ptr_ty.into(), ptr_ty.into(), i32_ty.into(), i32_ty.into()],
            false,
        );
        let function = self.module.add_function("plcc_left", fn_type, None);

        let saved_block = self.builder.get_insert_block();

        let entry = self.context.append_basic_block(function, "entry");
        let loop_bb = self.context.append_basic_block(function, "loop_bb");
        let body_bb = self.context.append_basic_block(function, "body_bb");
        let done = self.context.append_basic_block(function, "done");

        let dest = function.get_nth_param(0).unwrap().into_pointer_value();
        let src = function.get_nth_param(1).unwrap().into_pointer_value();
        let n = function.get_nth_param(2).unwrap().into_int_value();
        let max_len = function.get_nth_param(3).unwrap().into_int_value();

        self.builder.position_at_end(entry);
        let idx = self.builder.build_alloca(i32_ty, "idx").unwrap();
        self.builder.build_store(idx, i32_ty.const_zero()).unwrap();
        let max_minus1 = self
            .builder
            .build_int_sub(max_len, i32_ty.const_int(1, false), "max_m1")
            .unwrap();
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        // loop: i < n && i < max_len-1 && src[i] != 0
        self.builder.position_at_end(loop_bb);
        let i = self
            .builder
            .build_load(i32_ty, idx, "i")
            .unwrap()
            .into_int_value();
        let lt_n = self
            .builder
            .build_int_compare(IntPredicate::SLT, i, n, "lt_n")
            .unwrap();
        let lt_max = self
            .builder
            .build_int_compare(IntPredicate::SLT, i, max_minus1, "lt_max")
            .unwrap();
        let i_i64 = self
            .builder
            .build_int_s_extend(i, self.context.i64_type(), "i64")
            .unwrap();
        let src_ch_ptr = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, src, &[i_i64], "src_ch")
                .unwrap()
        };
        let ch = self
            .builder
            .build_load(i8_ty, src_ch_ptr, "ch")
            .unwrap()
            .into_int_value();
        let not_null = self
            .builder
            .build_int_compare(IntPredicate::NE, ch, i8_ty.const_zero(), "not_null")
            .unwrap();
        let c1 = self.builder.build_and(lt_n, lt_max, "c1").unwrap();
        let cont = self.builder.build_and(c1, not_null, "cont").unwrap();
        self.builder
            .build_conditional_branch(cont, body_bb, done)
            .unwrap();

        // body: dest[i] = src[i]; i++
        self.builder.position_at_end(body_bb);
        let dest_ch_ptr = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, dest, &[i_i64], "dest_ch")
                .unwrap()
        };
        self.builder.build_store(dest_ch_ptr, ch).unwrap();
        let i_next = self
            .builder
            .build_int_add(i, i32_ty.const_int(1, false), "i_next")
            .unwrap();
        self.builder.build_store(idx, i_next).unwrap();
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        // done: null-terminate
        self.builder.position_at_end(done);
        let final_i = self
            .builder
            .build_load(i32_ty, idx, "final_i")
            .unwrap()
            .into_int_value();
        let fi_i64 = self
            .builder
            .build_int_s_extend(final_i, self.context.i64_type(), "fi64")
            .unwrap();
        let null_ptr = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, dest, &[fi_i64], "null_ptr")
                .unwrap()
        };
        self.builder
            .build_store(null_ptr, i8_ty.const_zero())
            .unwrap();
        self.builder.build_return(None).unwrap();

        if let Some(bb) = saved_block {
            self.builder.position_at_end(bb);
        }
        function
    }

    /// Get or create `plcc_right(dest: *i8, src: *i8, n: i32, max_len: i32)`.
    /// Copies last n characters of src to dest.
    fn get_or_create_right_fn(&self) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function("plcc_right") {
            return f;
        }

        let void_ty = self.context.void_type();
        let i8_ty = self.context.i8_type();
        let i32_ty = self.context.i32_type();
        let i16_ty = self.context.i16_type();
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let fn_type = void_ty.fn_type(
            &[ptr_ty.into(), ptr_ty.into(), i32_ty.into(), i32_ty.into()],
            false,
        );
        let function = self.module.add_function("plcc_right", fn_type, None);

        let saved_block = self.builder.get_insert_block();

        let entry = self.context.append_basic_block(function, "entry");
        let loop_bb = self.context.append_basic_block(function, "loop_bb");
        let body_bb = self.context.append_basic_block(function, "body_bb");
        let done = self.context.append_basic_block(function, "done");

        let dest = function.get_nth_param(0).unwrap().into_pointer_value();
        let src = function.get_nth_param(1).unwrap().into_pointer_value();
        let n = function.get_nth_param(2).unwrap().into_int_value();
        let max_len = function.get_nth_param(3).unwrap().into_int_value();

        self.builder.position_at_end(entry);
        // Get strlen(src) via call to plcc_strlen
        let strlen_fn = self.get_or_create_strlen_fn();
        let slen_result = self
            .builder
            .build_call(strlen_fn, &[src.into()], "slen")
            .unwrap()
            .try_as_basic_value();
        let slen_i16 = match slen_result {
            inkwell::values::ValueKind::Basic(v) => v.into_int_value(),
            _ => i16_ty.const_zero(),
        };
        let slen = self
            .builder
            .build_int_s_extend(slen_i16, i32_ty, "slen32")
            .unwrap();
        // start = max(0, slen - n)
        let diff = self.builder.build_int_sub(slen, n, "diff").unwrap();
        let is_neg = self
            .builder
            .build_int_compare(IntPredicate::SLT, diff, i32_ty.const_zero(), "is_neg")
            .unwrap();
        let start = self
            .builder
            .build_select(is_neg, i32_ty.const_zero(), diff, "start")
            .unwrap()
            .into_int_value();

        let dest_idx = self.builder.build_alloca(i32_ty, "dest_idx").unwrap();
        let src_idx = self.builder.build_alloca(i32_ty, "src_idx").unwrap();
        self.builder
            .build_store(dest_idx, i32_ty.const_zero())
            .unwrap();
        self.builder.build_store(src_idx, start).unwrap();
        let max_minus1 = self
            .builder
            .build_int_sub(max_len, i32_ty.const_int(1, false), "max_m1")
            .unwrap();
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        // loop: src_idx < slen && dest_idx < max_len-1
        self.builder.position_at_end(loop_bb);
        let si = self
            .builder
            .build_load(i32_ty, src_idx, "si")
            .unwrap()
            .into_int_value();
        let di = self
            .builder
            .build_load(i32_ty, dest_idx, "di")
            .unwrap()
            .into_int_value();
        let lt_slen = self
            .builder
            .build_int_compare(IntPredicate::SLT, si, slen, "lt_slen")
            .unwrap();
        let lt_max = self
            .builder
            .build_int_compare(IntPredicate::SLT, di, max_minus1, "lt_max")
            .unwrap();
        let cont = self.builder.build_and(lt_slen, lt_max, "cont").unwrap();
        self.builder
            .build_conditional_branch(cont, body_bb, done)
            .unwrap();

        // body: dest[dest_idx] = src[src_idx]; both++
        self.builder.position_at_end(body_bb);
        let si_i64 = self
            .builder
            .build_int_s_extend(si, self.context.i64_type(), "si64")
            .unwrap();
        let di_i64 = self
            .builder
            .build_int_s_extend(di, self.context.i64_type(), "di64")
            .unwrap();
        let src_ch = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, src, &[si_i64], "src_ch")
                .unwrap()
        };
        let ch = self.builder.build_load(i8_ty, src_ch, "ch").unwrap();
        let dest_ch = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, dest, &[di_i64], "dest_ch")
                .unwrap()
        };
        self.builder.build_store(dest_ch, ch).unwrap();
        let si_next = self
            .builder
            .build_int_add(si, i32_ty.const_int(1, false), "si_next")
            .unwrap();
        let di_next = self
            .builder
            .build_int_add(di, i32_ty.const_int(1, false), "di_next")
            .unwrap();
        self.builder.build_store(src_idx, si_next).unwrap();
        self.builder.build_store(dest_idx, di_next).unwrap();
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        // done: null-terminate
        self.builder.position_at_end(done);
        let final_di = self
            .builder
            .build_load(i32_ty, dest_idx, "final_di")
            .unwrap()
            .into_int_value();
        let fdi_i64 = self
            .builder
            .build_int_s_extend(final_di, self.context.i64_type(), "fdi64")
            .unwrap();
        let null_ptr = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, dest, &[fdi_i64], "null_ptr")
                .unwrap()
        };
        self.builder
            .build_store(null_ptr, i8_ty.const_zero())
            .unwrap();
        self.builder.build_return(None).unwrap();

        if let Some(bb) = saved_block {
            self.builder.position_at_end(bb);
        }
        function
    }

    /// Get or create `plcc_mid(dest: *i8, src: *i8, len: i32, pos: i32, max_len: i32)`.
    /// Copies `len` characters starting at 1-based position `pos` from src to dest.
    fn get_or_create_mid_fn(&self) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function("plcc_mid") {
            return f;
        }

        let void_ty = self.context.void_type();
        let i8_ty = self.context.i8_type();
        let i32_ty = self.context.i32_type();
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let fn_type = void_ty.fn_type(
            &[
                ptr_ty.into(),
                ptr_ty.into(),
                i32_ty.into(),
                i32_ty.into(),
                i32_ty.into(),
            ],
            false,
        );
        let function = self.module.add_function("plcc_mid", fn_type, None);

        let saved_block = self.builder.get_insert_block();

        let entry = self.context.append_basic_block(function, "entry");
        let loop_bb = self.context.append_basic_block(function, "loop_bb");
        let body_bb = self.context.append_basic_block(function, "body_bb");
        let done = self.context.append_basic_block(function, "done");

        let dest = function.get_nth_param(0).unwrap().into_pointer_value();
        let src = function.get_nth_param(1).unwrap().into_pointer_value();
        let len_param = function.get_nth_param(2).unwrap().into_int_value();
        let pos = function.get_nth_param(3).unwrap().into_int_value();
        let max_len = function.get_nth_param(4).unwrap().into_int_value();

        self.builder.position_at_end(entry);
        // offset = pos - 1 (convert 1-based to 0-based)
        let offset = self
            .builder
            .build_int_sub(pos, i32_ty.const_int(1, false), "offset")
            .unwrap();
        let dest_idx = self.builder.build_alloca(i32_ty, "dest_idx").unwrap();
        let src_idx = self.builder.build_alloca(i32_ty, "src_idx").unwrap();
        let count = self.builder.build_alloca(i32_ty, "count").unwrap();
        self.builder
            .build_store(dest_idx, i32_ty.const_zero())
            .unwrap();
        self.builder.build_store(src_idx, offset).unwrap();
        self.builder
            .build_store(count, i32_ty.const_zero())
            .unwrap();
        let max_minus1 = self
            .builder
            .build_int_sub(max_len, i32_ty.const_int(1, false), "max_m1")
            .unwrap();
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        // loop: count < len && dest_idx < max_len-1 && src[src_idx] != 0
        self.builder.position_at_end(loop_bb);
        let c = self
            .builder
            .build_load(i32_ty, count, "c")
            .unwrap()
            .into_int_value();
        let di = self
            .builder
            .build_load(i32_ty, dest_idx, "di")
            .unwrap()
            .into_int_value();
        let si = self
            .builder
            .build_load(i32_ty, src_idx, "si")
            .unwrap()
            .into_int_value();
        let lt_len = self
            .builder
            .build_int_compare(IntPredicate::SLT, c, len_param, "lt_len")
            .unwrap();
        let lt_max = self
            .builder
            .build_int_compare(IntPredicate::SLT, di, max_minus1, "lt_max")
            .unwrap();
        let si_i64 = self
            .builder
            .build_int_s_extend(si, self.context.i64_type(), "si64")
            .unwrap();
        let src_ch_ptr = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, src, &[si_i64], "src_ch")
                .unwrap()
        };
        let ch = self
            .builder
            .build_load(i8_ty, src_ch_ptr, "ch")
            .unwrap()
            .into_int_value();
        let not_null = self
            .builder
            .build_int_compare(IntPredicate::NE, ch, i8_ty.const_zero(), "not_null")
            .unwrap();
        let c1 = self.builder.build_and(lt_len, lt_max, "c1").unwrap();
        let cont = self.builder.build_and(c1, not_null, "cont").unwrap();
        self.builder
            .build_conditional_branch(cont, body_bb, done)
            .unwrap();

        // body: dest[dest_idx] = ch; all++
        self.builder.position_at_end(body_bb);
        let di_i64 = self
            .builder
            .build_int_s_extend(di, self.context.i64_type(), "di64")
            .unwrap();
        let dest_ch_ptr = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, dest, &[di_i64], "dest_ch")
                .unwrap()
        };
        self.builder.build_store(dest_ch_ptr, ch).unwrap();
        let di_next = self
            .builder
            .build_int_add(di, i32_ty.const_int(1, false), "di_next")
            .unwrap();
        let si_next = self
            .builder
            .build_int_add(si, i32_ty.const_int(1, false), "si_next")
            .unwrap();
        let c_next = self
            .builder
            .build_int_add(c, i32_ty.const_int(1, false), "c_next")
            .unwrap();
        self.builder.build_store(dest_idx, di_next).unwrap();
        self.builder.build_store(src_idx, si_next).unwrap();
        self.builder.build_store(count, c_next).unwrap();
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        // done: null-terminate
        self.builder.position_at_end(done);
        let final_di = self
            .builder
            .build_load(i32_ty, dest_idx, "final_di")
            .unwrap()
            .into_int_value();
        let fdi_i64 = self
            .builder
            .build_int_s_extend(final_di, self.context.i64_type(), "fdi64")
            .unwrap();
        let null_ptr = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, dest, &[fdi_i64], "null_ptr")
                .unwrap()
        };
        self.builder
            .build_store(null_ptr, i8_ty.const_zero())
            .unwrap();
        self.builder.build_return(None).unwrap();

        if let Some(bb) = saved_block {
            self.builder.position_at_end(bb);
        }
        function
    }

    /// Try to handle string function assignments like `result := CONCAT(a, b)`.
    /// Returns true if the assignment was handled as a string function call.
    fn try_compile_string_assignment(
        &mut self,
        target: &Expression,
        value: &Expression,
        function: FunctionValue<'ctx>,
    ) -> Result<bool, CodegenError> {
        let (callee, args) = match &value.kind {
            ExpressionKind::FunctionCall { callee, args } => (callee, args),
            _ => return Ok(false),
        };
        let func_name = match &callee.kind {
            ExpressionKind::Identifier(ident) => ident.name.to_uppercase(),
            _ => return Ok(false),
        };

        // Only handle known string functions
        if !matches!(func_name.as_str(), "CONCAT" | "LEFT" | "RIGHT" | "MID") {
            return Ok(false);
        }

        // Get the destination pointer and its max_len from the type
        let dest_ptr = match self.compile_lvalue_with_fn(target, function)? {
            Some(p) => p,
            None => return Ok(false),
        };

        // Determine max_len from the target's type
        let max_len = if let ExpressionKind::Identifier(ident) = &target.kind {
            if let Some((_, iec_ty)) = self.variables.get(&ident.name.to_uppercase()).cloned() {
                match iec_ty {
                    IecType::StringType { max_len } => max_len.unwrap_or(256) as i32 + 1,
                    _ => return Ok(false),
                }
            } else {
                return Ok(false);
            }
        } else {
            return Ok(false);
        };

        let i32_ty = self.context.i32_type();
        let max_len_val = i32_ty.const_int(max_len as u64, false);

        match func_name.as_str() {
            "CONCAT" => {
                if args.len() != 2 {
                    return Err(CodegenError::LlvmError("CONCAT expects 2 arguments".into()));
                }
                let s1_ptr = self
                    .compile_lvalue_with_fn(&args[0].value, function)?
                    .ok_or_else(|| {
                        CodegenError::LlvmError("CONCAT: failed to get s1 pointer".into())
                    })?;
                let s2_ptr = self
                    .compile_lvalue_with_fn(&args[1].value, function)?
                    .ok_or_else(|| {
                        CodegenError::LlvmError("CONCAT: failed to get s2 pointer".into())
                    })?;
                let concat_fn = self.get_or_create_concat_fn();
                self.builder
                    .build_call(
                        concat_fn,
                        &[
                            dest_ptr.into(),
                            s1_ptr.into(),
                            s2_ptr.into(),
                            max_len_val.into(),
                        ],
                        "",
                    )
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(true)
            }
            "LEFT" => {
                if args.len() != 2 {
                    return Err(CodegenError::LlvmError("LEFT expects 2 arguments".into()));
                }
                let src_ptr = self
                    .compile_lvalue_with_fn(&args[0].value, function)?
                    .ok_or_else(|| {
                        CodegenError::LlvmError("LEFT: failed to get src pointer".into())
                    })?;
                let n_val = self
                    .compile_expression(&args[1].value, function)?
                    .ok_or_else(|| CodegenError::LlvmError("LEFT: failed to compile n".into()))?;
                let n_i32 = if n_val.is_int_value() {
                    let iv = n_val.into_int_value();
                    if iv.get_type().get_bit_width() < 32 {
                        self.builder
                            .build_int_s_extend(iv, i32_ty, "n_ext")
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                    } else {
                        iv
                    }
                } else {
                    return Err(CodegenError::LlvmError("LEFT: n must be integer".into()));
                };
                let left_fn = self.get_or_create_left_fn();
                self.builder
                    .build_call(
                        left_fn,
                        &[
                            dest_ptr.into(),
                            src_ptr.into(),
                            n_i32.into(),
                            max_len_val.into(),
                        ],
                        "",
                    )
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(true)
            }
            "RIGHT" => {
                if args.len() != 2 {
                    return Err(CodegenError::LlvmError("RIGHT expects 2 arguments".into()));
                }
                let src_ptr = self
                    .compile_lvalue_with_fn(&args[0].value, function)?
                    .ok_or_else(|| {
                        CodegenError::LlvmError("RIGHT: failed to get src pointer".into())
                    })?;
                let n_val = self
                    .compile_expression(&args[1].value, function)?
                    .ok_or_else(|| CodegenError::LlvmError("RIGHT: failed to compile n".into()))?;
                let n_i32 = if n_val.is_int_value() {
                    let iv = n_val.into_int_value();
                    if iv.get_type().get_bit_width() < 32 {
                        self.builder
                            .build_int_s_extend(iv, i32_ty, "n_ext")
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                    } else {
                        iv
                    }
                } else {
                    return Err(CodegenError::LlvmError("RIGHT: n must be integer".into()));
                };
                let right_fn = self.get_or_create_right_fn();
                self.builder
                    .build_call(
                        right_fn,
                        &[
                            dest_ptr.into(),
                            src_ptr.into(),
                            n_i32.into(),
                            max_len_val.into(),
                        ],
                        "",
                    )
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(true)
            }
            "MID" => {
                if args.len() != 3 {
                    return Err(CodegenError::LlvmError(
                        "MID expects 3 arguments (string, length, position)".into(),
                    ));
                }
                let src_ptr = self
                    .compile_lvalue_with_fn(&args[0].value, function)?
                    .ok_or_else(|| {
                        CodegenError::LlvmError("MID: failed to get src pointer".into())
                    })?;
                let len_val = self
                    .compile_expression(&args[1].value, function)?
                    .ok_or_else(|| CodegenError::LlvmError("MID: failed to compile len".into()))?;
                let pos_val = self
                    .compile_expression(&args[2].value, function)?
                    .ok_or_else(|| CodegenError::LlvmError("MID: failed to compile pos".into()))?;
                let len_i32 = if len_val.is_int_value() {
                    let iv = len_val.into_int_value();
                    if iv.get_type().get_bit_width() < 32 {
                        self.builder
                            .build_int_s_extend(iv, i32_ty, "len_ext")
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                    } else {
                        iv
                    }
                } else {
                    return Err(CodegenError::LlvmError("MID: len must be integer".into()));
                };
                let pos_i32 = if pos_val.is_int_value() {
                    let iv = pos_val.into_int_value();
                    if iv.get_type().get_bit_width() < 32 {
                        self.builder
                            .build_int_s_extend(iv, i32_ty, "pos_ext")
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                    } else {
                        iv
                    }
                } else {
                    return Err(CodegenError::LlvmError("MID: pos must be integer".into()));
                };
                let mid_fn = self.get_or_create_mid_fn();
                self.builder
                    .build_call(
                        mid_fn,
                        &[
                            dest_ptr.into(),
                            src_ptr.into(),
                            len_i32.into(),
                            pos_i32.into(),
                            max_len_val.into(),
                        ],
                        "",
                    )
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(true)
            }
            _ => Ok(false),
        }
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
                .build_signed_int_to_float(val.into_int_value(), self.context.f32_type(), "itof")
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
            let (name, fb_type) = match decl {
                Declaration::FunctionBlock(fb) => (
                    fb.name.name.clone(),
                    IecType::FbInstance(fb.name.name.clone()),
                ),
                Declaration::Class(cls) => (
                    cls.name.name.clone(),
                    IecType::FbInstance(cls.name.name.clone()),
                ),
                // An INTERFACE-typed variable is valid 3rd-edition ST (IEC 61131-3
                // §6.6.4). It holds a *reference* to an object implementing the
                // interface, not an inline instance, so it gets a pointer slot —
                // registering it as an FbInstance would find no layout and silently
                // fall back to an i32 field, which is exactly the miscompile the
                // unknown-type error exists to prevent. Skipping it entirely made the
                // declaration a hard error.
                Declaration::Interface(iface) => (
                    iface.name.name.clone(),
                    IecType::Pointer(Box::new(IecType::FbInstance(iface.name.name.clone()))),
                ),
                _ => continue,
            };
            // TypeRegistry::register case-folds the key, so one registration covers
            // every spelling a program might use.
            self.type_registry.register(name.clone(), fb_type.clone());
            self.type_checker.types.register(name, fb_type);
        }

        // Register user TYPE declarations (STRUCT / ENUM / subrange / alias) so that a
        // `v : MyStruct;` declaration resolves instead of falling through to the
        // "unknown type" fallback. Two passes so a TYPE may reference another TYPE
        // declared later in the file.
        for _ in 0..2 {
            for decl in &unit.declarations {
                if let Declaration::TypeDecl(td) = decl {
                    let ty = self.resolve_type_spec(&td.type_spec);
                    self.type_registry
                        .register(td.name.name.clone(), ty.clone());
                    self.type_checker.types.register(td.name.name.clone(), ty);
                }
            }
        }

        // Every declared type must resolve to something real *before* we lay out any
        // struct. Previously an unknown type name (`t : TON;` with no TON in scope)
        // silently became an i32 field and every statement referring to it was dropped
        // from the generated code — a silent miscompile with no diagnostic at all.
        self.validate_declared_types(unit)?;

        // Lay out every FB/CLASS state struct *before* anything that needs to know
        // their size. This makes FB instance fields work regardless of declaration
        // order (an FB may contain an instance of an FB declared later in the file).
        self.layout_pou_structs(unit);

        // Declare the `<pou>_init` prototypes up front so init bodies can call each
        // other without regard to declaration order.
        self.declare_init_prototypes(unit);

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

        // Compile FBs, classes, and functions first so they're available when programs reference them
        for decl in &unit.declarations {
            match decl {
                Declaration::Function(f) => self.compile_function(f)?,
                Declaration::FunctionBlock(fb) => self.compile_function_block(fb)?,
                Declaration::Class(cls) => self.compile_class(cls)?,
                _ => {}
            }
        }
        // Now that every `<fb>_init` has a body, emit the global initializer that
        // initializes VAR_GLOBAL FB instances. Programs' `_init` will call it.
        self.emit_globals_init()?;

        // Then compile programs (which may instantiate FBs)
        for decl in &unit.declarations {
            if let Declaration::Program(p) = decl {
                self.compile_program(p)?;
            }
        }

        // Structurally malformed IR must never escape this function. `emit_object`
        // runs LLVM at OptimizationLevel::Default; the execution tests JIT at
        // OptimizationLevel::None. Anything the verifier rejects is a codegen bug that
        // the JIT path would otherwise hide until someone ran the real `plcc compile`.
        self.module
            .verify()
            .map_err(|e| CodegenError::InvalidModule(e.to_string()))?;
        Ok(())
    }

    /// First unresolved type name reachable from `ty`, if any.
    ///
    /// Walks through the aggregate types so `ARRAY [1..4] OF TON` and
    /// `STRUCT t : TON; END_STRUCT` are caught too, not just a bare `t : TON;`.
    fn first_unresolved(ty: &IecType) -> Option<String> {
        match ty {
            IecType::Unresolved(name) => Some(name.clone()),
            IecType::Array { element_type, .. } => Self::first_unresolved(element_type),
            IecType::Pointer(inner)
            | IecType::Alias {
                base_type: inner, ..
            } => Self::first_unresolved(inner),
            IecType::Struct { fields, .. } => {
                fields.iter().find_map(|(_, t)| Self::first_unresolved(t))
            }
            _ => None,
        }
    }

    /// Reject any declaration whose type name does not resolve.
    ///
    /// This is deliberately in codegen rather than in `plcc-hir`: `plcc compile` and
    /// `plcc sim` never run the HIR checker, so a HIR-only diagnostic would leave the
    /// actual miscompile in place. Codegen is the layer that would otherwise emit the
    /// wrong code, so codegen is where the guarantee has to hold. (A HIR diagnostic on
    /// top of this would be a nicer *message* — with a source span — but it cannot be
    /// the enforcement point.)
    fn validate_declared_types(&mut self, unit: &CompilationUnit) -> Result<(), CodegenError> {
        let mut check = |this: &mut Self,
                         pou_kind: &'static str,
                         pou_name: &str,
                         blocks: &[VarBlock]|
         -> Result<(), CodegenError> {
            for block in blocks {
                for decl in &block.declarations {
                    let ty = this.resolve_type_spec(&decl.type_spec);
                    if let Some(type_name) = Self::first_unresolved(&ty) {
                        return Err(CodegenError::UnknownType {
                            pou_kind,
                            pou_name: pou_name.to_string(),
                            var_name: decl.name.name.clone(),
                            type_name,
                        });
                    }
                }
            }
            Ok(())
        };

        for decl in &unit.declarations {
            match decl {
                Declaration::Program(p) => {
                    check(self, "PROGRAM", &p.name.name.clone(), &p.var_blocks)?
                }
                Declaration::Function(f) => {
                    check(self, "FUNCTION", &f.name.name.clone(), &f.var_blocks)?
                }
                Declaration::FunctionBlock(fb) => check(
                    self,
                    "FUNCTION_BLOCK",
                    &fb.name.name.clone(),
                    &fb.var_blocks,
                )?,
                Declaration::Class(cls) => {
                    check(self, "CLASS", &cls.name.name.clone(), &cls.var_blocks)?
                }
                Declaration::GlobalVarDecl(block) => {
                    check(self, "VAR_GLOBAL", "", std::slice::from_ref(block))?
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Name of the generated init function for a POU.
    fn init_fn_name_for(name: &str) -> String {
        format!("{}_init", name.to_lowercase())
    }

    /// Init function name recorded on a compiled FB/CLASS layout, if it has one.
    fn fb_init_fn_name(&self, fb_type_name: &str) -> String {
        self.compiled_fbs
            .get(&fb_type_name.to_uppercase())
            .map(|l| l.init_fn_name.clone())
            .unwrap_or_else(|| Self::init_fn_name_for(fb_type_name))
    }

    /// True when every FB-instance type reachable from `ty` already has a recorded layout.
    fn fb_layout_ready(&self, ty: &IecType) -> bool {
        match ty {
            IecType::FbInstance(name) => self.compiled_fbs.contains_key(&name.to_uppercase()),
            IecType::Array { element_type, .. } => self.fb_layout_ready(element_type),
            IecType::Struct { fields, .. } => fields.iter().all(|(_, t)| self.fb_layout_ready(t)),
            _ => true,
        }
    }

    /// Record (or overwrite) the struct layout for an FB/CLASS.
    fn record_pou_layout(&mut self, name: &str, fields: Vec<(String, IecType)>) {
        let field_types: Vec<BasicTypeEnum<'ctx>> = fields
            .iter()
            .map(|(_, t)| self.iec_to_llvm_type(t))
            .collect();
        let struct_type = self.context.struct_type(&field_types, false);
        self.compiled_fbs.insert(
            name.to_uppercase(),
            FbLayout {
                struct_type,
                scan_fn_name: format!("{}_scan", name.to_lowercase()),
                init_fn_name: Self::init_fn_name_for(name),
                fields,
                methods: HashMap::new(),
            },
        );
    }

    /// Resolve the ordered (name, type) field list of a POU's variable blocks.
    fn resolve_pou_fields(&mut self, var_blocks: &[VarBlock]) -> Vec<(String, IecType)> {
        let mut fields = Vec::new();
        for block in var_blocks {
            for decl in &block.declarations {
                let ty = self.resolve_type_spec(&decl.type_spec);
                fields.push((decl.name.name.clone(), ty));
            }
        }
        fields
    }

    /// Compute the LLVM struct layout of every FUNCTION_BLOCK and CLASS in the unit,
    /// in dependency order, so that nested FB instances get the right member type
    /// regardless of the order the POUs appear in the source.
    fn layout_pou_structs(&mut self, unit: &CompilationUnit) {
        let mut pending: Vec<(String, &[VarBlock])> = Vec::new();
        for decl in &unit.declarations {
            match decl {
                Declaration::FunctionBlock(fb) => {
                    pending.push((fb.name.name.clone(), &fb.var_blocks))
                }
                Declaration::Class(cls) => pending.push((cls.name.name.clone(), &cls.var_blocks)),
                _ => {}
            }
        }

        while !pending.is_empty() {
            let mut deferred: Vec<(String, &[VarBlock])> = Vec::new();
            let mut progress = false;
            for (name, blocks) in std::mem::take(&mut pending) {
                let fields = self.resolve_pou_fields(blocks);
                if fields.iter().all(|(_, t)| self.fb_layout_ready(t)) {
                    self.record_pou_layout(&name, fields);
                    progress = true;
                } else {
                    deferred.push((name, blocks));
                }
            }
            if !progress {
                // Unresolvable (recursive instantiation, or an unknown type). Lay the
                // rest out anyway with whatever iec_to_llvm_type falls back to.
                for (name, blocks) in deferred {
                    let fields = self.resolve_pou_fields(blocks);
                    self.record_pou_layout(&name, fields);
                }
                break;
            }
            pending = deferred;
        }
    }

    /// Declare `<pou>_init(ptr) -> void` for every PROGRAM, FUNCTION_BLOCK and CLASS.
    /// Bodies are filled in later; declaring them all first lets init bodies call each
    /// other regardless of declaration order.
    fn declare_init_prototypes(&mut self, unit: &CompilationUnit) {
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let fn_type = self.context.void_type().fn_type(&[ptr_ty.into()], false);
        for decl in &unit.declarations {
            let name = match decl {
                Declaration::FunctionBlock(fb) => &fb.name.name,
                Declaration::Class(cls) => &cls.name.name,
                Declaration::Program(p) => &p.name.name,
                _ => continue,
            };
            let init_name = Self::init_fn_name_for(name);
            if self.module.get_function(&init_name).is_none() {
                self.module.add_function(&init_name, fn_type, None);
            }
        }
    }

    /// Emit `plcc_globals_init()`, which initializes VAR_GLOBAL FB instances by
    /// calling their `<fb>_init`. Scalar globals are already handled by the constant
    /// aggregate initializer on the global itself; a constant aggregate cannot call a
    /// function, so FB instances need this runtime pass.
    fn emit_globals_init(&mut self) -> Result<(), CodegenError> {
        let Some((global_val, global_struct, names)) = self.global_var.clone() else {
            return Ok(());
        };
        if !names
            .iter()
            .any(|(_, t)| matches!(t, IecType::FbInstance(_)))
        {
            return Ok(());
        }

        let fn_type = self.context.void_type().fn_type(&[], false);
        let func = self.module.add_function(GLOBALS_INIT_FN, fn_type, None);
        let entry = self.context.append_basic_block(func, "entry");
        self.builder.position_at_end(entry);

        let global_ptr = global_val.as_pointer_value();
        for (i, (name, ty)) in names.iter().enumerate() {
            let IecType::FbInstance(fb_name) = ty else {
                continue;
            };
            let init_name = self.fb_init_fn_name(fb_name);
            let Some(init_fn) = self
                .module
                .get_function(&init_name)
                .filter(|f| f.count_basic_blocks() > 0)
            else {
                continue;
            };
            let ptr = self
                .builder
                .build_struct_gep(global_struct, global_ptr, i as u32, name)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            self.builder
                .build_call(init_fn, &[ptr.into()], "")
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        }

        self.builder
            .build_return(None)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        Ok(())
    }

    /// Widen/narrow/convert an initializer value so it matches the member's storage type.
    /// Without this, `x : DINT := -1` would store a 16-bit -1 into a 32-bit slot.
    ///
    /// The source type is unknown here, so widening falls back to the destination's
    /// signedness. That is right for a bare literal (`x : DINT := -1` must sign-extend)
    /// and wrong for a typed value, so anything that *has* a source type must call
    /// [`Self::coerce_value`] instead.
    fn coerce_init_value(
        &self,
        val: BasicValueEnum<'ctx>,
        ty: &IecType,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        self.coerce_value(val, None, ty)
    }

    /// True when values of `ty` are widened by zero-extension.
    fn widens_unsigned(ty: &IecType) -> bool {
        matches!(
            ty,
            IecType::Bool
                | IecType::Byte
                | IecType::Word
                | IecType::Dword
                | IecType::Lword
                | IecType::Usint
                | IecType::Uint
                | IecType::Udint
                | IecType::Ulint
                | IecType::Char
                | IecType::Wchar
        )
    }

    /// Convert `val` (of IEC type `src`, when known) to the storage type of `ty`.
    ///
    /// Widening signedness comes from the **source**, not the destination. A BYTE
    /// holding 16#FF is 255, and it is still 255 after `acc : DINT := raw_b;` — taking
    /// the sign from the DINT destination turned every ANY_BIT and ANY_UNSIGNED value
    /// assigned into a wider signed slot into a negative number.
    ///
    /// `src` is `None` for values with no static IEC type (bare literals); the
    /// destination's signedness is the fallback there.
    fn coerce_value(
        &self,
        val: BasicValueEnum<'ctx>,
        src: Option<&IecType>,
        ty: &IecType,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let target = self.iec_to_llvm_type(ty);
        let unsigned = match src {
            Some(src_ty) => Self::widens_unsigned(src_ty),
            None => Self::widens_unsigned(ty),
        };
        match (val, target) {
            (BasicValueEnum::IntValue(iv), BasicTypeEnum::IntType(it)) => {
                let (sw, tw) = (iv.get_type().get_bit_width(), it.get_bit_width());
                if sw == tw {
                    Ok(val)
                } else if sw < tw {
                    let ext = if unsigned {
                        self.builder.build_int_z_extend(iv, it, "initzext")
                    } else {
                        self.builder.build_int_s_extend(iv, it, "initsext")
                    }
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    Ok(ext.into())
                } else {
                    let tr = self
                        .builder
                        .build_int_truncate(iv, it, "inittrunc")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    Ok(tr.into())
                }
            }
            (BasicValueEnum::IntValue(iv), BasicTypeEnum::FloatType(ft)) => {
                let f = self
                    .builder
                    .build_signed_int_to_float(iv, ft, "initsitofp")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(f.into())
            }
            (BasicValueEnum::FloatValue(fv), BasicTypeEnum::FloatType(ft)) => {
                if fv.get_type() == ft {
                    Ok(val)
                } else {
                    let c = self
                        .builder
                        .build_float_cast(fv, ft, "initfpcast")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    Ok(c.into())
                }
            }
            (BasicValueEnum::FloatValue(fv), BasicTypeEnum::IntType(it)) => {
                let i = self
                    .builder
                    .build_float_to_signed_int(fv, it, "initfptosi")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(i.into())
            }
            _ => Ok(val),
        }
    }

    /// Emit the body of `<pou>_init(state_ptr)`.
    ///
    /// Walks `var_blocks` in the *same* order used to build the state struct, storing
    /// each declared initial value into its field. FB-instance fields recurse into the
    /// nested type's own `_init`, so arbitrarily deep nesting is initialized.
    fn emit_init_body(
        &mut self,
        pou_name: &str,
        var_blocks: &[VarBlock],
        struct_type: StructType<'ctx>,
        call_globals_init: bool,
    ) -> Result<(), CodegenError> {
        let init_name = Self::init_fn_name_for(pou_name);
        let init_fn = match self.module.get_function(&init_name) {
            Some(f) if f.count_basic_blocks() == 0 => f,
            Some(_) => return Ok(()), // already emitted
            None => {
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let fn_type = self.context.void_type().fn_type(&[ptr_ty.into()], false);
                self.module.add_function(&init_name, fn_type, None)
            }
        };

        let entry = self.context.append_basic_block(init_fn, "entry");
        self.builder.position_at_end(entry);

        let state_ptr = init_fn
            .get_nth_param(0)
            .ok_or_else(|| CodegenError::LlvmError("init fn missing state param".into()))?
            .into_pointer_value();

        let saved_struct_type = self.current_struct_type;
        let saved_state_ptr = self.current_state_ptr;
        self.variables.clear();
        self.current_struct_type = Some(struct_type);
        self.current_state_ptr = Some(state_ptr);

        // Globals first so that same-named POU members shadow them.
        self.add_globals_to_variables()?;

        if call_globals_init {
            if let Some(f) = self.module.get_function(GLOBALS_INIT_FN) {
                self.builder
                    .build_call(f, &[], "")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            }
        }

        let mut field_idx = 0u32;
        for block in var_blocks {
            for decl in &block.declarations {
                let iec_ty = self.resolve_type_spec(&decl.type_spec);
                let ptr = self
                    .builder
                    .build_struct_gep(struct_type, state_ptr, field_idx, &decl.name.name)
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                self.variables
                    .insert(decl.name.name.to_uppercase(), (ptr, iec_ty.clone()));

                if let IecType::FbInstance(inner) = &iec_ty {
                    // Recurse into the nested instance's own initializer.
                    let inner_init_name = self.fb_init_fn_name(inner);
                    if let Some(inner_init) = self.module.get_function(&inner_init_name) {
                        self.builder
                            .build_call(inner_init, &[ptr.into()], "")
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    }
                } else if let Some(init_expr) = &decl.initializer {
                    if let Some(val) = self.compile_expression(init_expr, init_fn)? {
                        let val = self.coerce_init_value(val, &iec_ty)?;
                        self.builder
                            .build_store(ptr, val)
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    }
                }
                field_idx += 1;
            }
        }

        self.builder
            .build_return(None)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        self.current_struct_type = saved_struct_type;
        self.current_state_ptr = saved_state_ptr;
        Ok(())
    }

    /// Evaluate a constant expression for use as a global initializer.
    fn eval_const_initializer(
        &self,
        expr: &Expression,
        _ty: &IecType,
    ) -> Option<BasicValueEnum<'ctx>> {
        match &expr.kind {
            ExpressionKind::IntegerLiteral(v) => Some(self.int_literal(*v).into()),
            ExpressionKind::RealLiteral(v) => Some(self.context.f32_type().const_float(*v).into()),
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

    /// Branch to `target` unless the block being built already ends in a terminator.
    ///
    /// LLVM's builder appends blindly at the end of the current block, so emitting a
    /// second terminator produces "Terminator found in the middle of a basic block"
    /// IR: everything after it is unreachable but still present, and the optimizer is
    /// free to do anything with it. Every structured-control-flow join goes through
    /// here so a body that already terminated (EXIT, a nested join, ...) cannot
    /// corrupt the block.
    fn branch_to_join(
        &self,
        target: inkwell::basic_block::BasicBlock<'ctx>,
    ) -> Result<(), CodegenError> {
        let Some(current) = self.builder.get_insert_block() else {
            return Ok(());
        };
        if current.get_terminator().is_some() {
            return Ok(());
        }
        self.builder
            .build_unconditional_branch(target)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        Ok(())
    }

    /// Convert any integer value to i1 for use in conditional branches.
    fn to_i1(
        &self,
        val: inkwell::values::IntValue<'ctx>,
    ) -> Result<inkwell::values::IntValue<'ctx>, CodegenError> {
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
    ) -> Result<
        (
            inkwell::values::IntValue<'ctx>,
            inkwell::values::IntValue<'ctx>,
        ),
        CodegenError,
    > {
        let aw = a.get_type().get_bit_width();
        let bw = b.get_type().get_bit_width();
        if aw == bw {
            Ok((a, b))
        } else if aw < bw {
            let ext = self
                .builder
                .build_int_s_extend(a, b.get_type(), "sext")
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            Ok((ext, b))
        } else {
            let ext = self
                .builder
                .build_int_s_extend(b, a.get_type(), "sext")
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            Ok((a, ext))
        }
    }

    /// Materialize an integer literal at the narrowest standard width that holds it.
    ///
    /// IEC 61131-3 gives an integer literal the type its context demands. Codegen has
    /// no context here, so it picks by magnitude: INT (i16) for anything that fits —
    /// which is the overwhelmingly common case and keeps ordinary INT arithmetic
    /// 16-bit — then DINT (i32), then LINT (i64). Fixing the width at i16
    /// unconditionally silently truncated every literal above 32767, so
    /// `ctr(PV := 100000)` passed -31072.
    ///
    /// Binary operations sign-extend to the wider operand, and assignment coerces to
    /// the destination, so a wider literal never leaks into a narrower slot.
    fn int_literal(&self, v: i128) -> inkwell::values::IntValue<'ctx> {
        if (i16::MIN as i128..=i16::MAX as i128).contains(&v) {
            self.context.i16_type().const_int(v as u64, true)
        } else if (i32::MIN as i128..=i32::MAX as i128).contains(&v) {
            self.context.i32_type().const_int(v as u64, true)
        } else {
            self.context.i64_type().const_int(v as u64, true)
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
                let total_size: u32 = ranges.iter().map(|(lo, hi)| (hi - lo + 1) as u32).product();
                elem_ty.array_type(total_size).into()
            }
            // TIME/LTIME stored as i64 (nanoseconds)
            IecType::Time | IecType::Ltime => self.context.i64_type().into(),
            // DATE types stored as i64 (Unix timestamp in nanoseconds)
            IecType::Date
            | IecType::Tod
            | IecType::Dt
            | IecType::Ldate
            | IecType::Ltod
            | IecType::Ldt => self.context.i64_type().into(),
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
        let fn_type = self
            .context
            .void_type()
            .fn_type(&[state_ptr_type.into()], false);
        let function = self.module.add_function(&fn_name, fn_type, None);

        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);

        let state_ptr = function.get_nth_param(0).unwrap().into_pointer_value();

        // Set up variables as GEP into the state struct, and detect FB instances
        self.variables.clear();
        self.fb_instances.clear();
        self.current_struct_type = Some(struct_type);
        self.current_state_ptr = Some(state_ptr);

        for (i, (name, iec_ty)) in field_names.iter().zip(field_iec_types.iter()).enumerate() {
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
                            methods: layout.methods.clone(),
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

        self.builder
            .build_return(None)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        // Generate _init() function that applies variable initializers, initializes
        // VAR_GLOBAL FB instances, and recursively initializes FB instance members.
        self.emit_init_body(&prog.name.name, &prog.var_blocks, struct_type, true)?;

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
        for (i, (name, iec_ty)) in param_names.iter().zip(param_iec_types.iter()).enumerate() {
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
        let fn_type = self
            .context
            .void_type()
            .fn_type(&[state_ptr_type.into()], false);
        let function = self.module.add_function(&fn_name, fn_type, None);

        // Record this FB's layout for use by parent POUs that instantiate it
        let fb_fields: Vec<(String, IecType)> = field_names
            .iter()
            .zip(field_iec_types.iter())
            .map(|(n, t)| (n.clone(), t.clone()))
            .collect();

        // Compile methods first (before the scan body) so they're available
        let mut method_infos: HashMap<String, MethodInfo> = HashMap::new();
        for method in &fb.methods {
            let method_info = self.compile_method(
                &fb.name.name,
                method,
                struct_type,
                &field_names,
                &field_iec_types,
            )?;
            method_infos.insert(method.name.name.to_uppercase(), method_info);
        }

        self.compiled_fbs.insert(
            fb.name.name.to_uppercase(),
            FbLayout {
                struct_type,
                scan_fn_name: fn_name.clone(),
                init_fn_name: Self::init_fn_name_for(&fb.name.name),
                fields: fb_fields,
                methods: method_infos,
            },
        );

        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);

        let state_ptr = function.get_nth_param(0).unwrap().into_pointer_value();

        self.variables.clear();
        self.fb_instances.clear();
        for (i, (name, iec_ty)) in field_names.iter().zip(field_iec_types.iter()).enumerate() {
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

        // Emit `<fb>_init` so declared initial values reach every instance.
        self.emit_init_body(&fb.name.name, &fb.var_blocks, struct_type, false)?;
        Ok(())
    }

    /// Compile a CLASS declaration. A CLASS is like an FB but has no scan body — only methods.
    fn compile_class(&mut self, cls: &ClassDecl) -> Result<(), CodegenError> {
        let fn_name = format!("{}_scan", cls.name.name.to_lowercase());

        let mut field_types: Vec<BasicTypeEnum<'ctx>> = Vec::new();
        let mut field_names: Vec<String> = Vec::new();
        let mut field_iec_types: Vec<IecType> = Vec::new();

        for block in &cls.var_blocks {
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

        // Create an empty scan function (classes don't have a body like FBs)
        let void_fn_type = self
            .context
            .void_type()
            .fn_type(&[state_ptr_type.into()], false);
        let scan_function = self.module.add_function(&fn_name, void_fn_type, None);
        let entry = self.context.append_basic_block(scan_function, "entry");
        self.builder.position_at_end(entry);
        self.builder
            .build_return(None)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        let fb_fields: Vec<(String, IecType)> = field_names
            .iter()
            .zip(field_iec_types.iter())
            .map(|(n, t)| (n.clone(), t.clone()))
            .collect();

        // Compile methods
        let mut method_infos: HashMap<String, MethodInfo> = HashMap::new();
        for method in &cls.methods {
            let method_info = self.compile_method(
                &cls.name.name,
                method,
                struct_type,
                &field_names,
                &field_iec_types,
            )?;
            method_infos.insert(method.name.name.to_uppercase(), method_info);
        }

        self.compiled_fbs.insert(
            cls.name.name.to_uppercase(),
            FbLayout {
                struct_type,
                scan_fn_name: fn_name,
                init_fn_name: Self::init_fn_name_for(&cls.name.name),
                fields: fb_fields,
                methods: method_infos,
            },
        );

        // Emit `<cls>_init` so declared initial values reach every instance.
        self.emit_init_body(&cls.name.name, &cls.var_blocks, struct_type, false)?;

        Ok(())
    }

    /// Compile a METHOD declaration on an FB/Class.
    /// Produces an LLVM function: `{fb_name}_{method_name}(instance_ptr, ...params) -> ret_type`
    fn compile_method(
        &mut self,
        fb_name: &str,
        method: &MethodDecl,
        fb_struct_type: StructType<'ctx>,
        fb_field_names: &[String],
        fb_field_iec_types: &[IecType],
    ) -> Result<MethodInfo, CodegenError> {
        let method_fn_name = format!(
            "{}_{}",
            fb_name.to_lowercase(),
            method.name.name.to_lowercase()
        );

        let ret_iec_ty = method
            .return_type
            .as_ref()
            .map(|t| self.resolve_type_spec(t))
            .unwrap_or(IecType::Void);

        // First param is always the instance pointer
        let state_ptr_type = self.context.ptr_type(AddressSpace::default());
        let mut param_types: Vec<BasicMetadataTypeEnum<'ctx>> = vec![state_ptr_type.into()];
        let mut param_names: Vec<String> = Vec::new();
        let mut param_iec_types: Vec<IecType> = Vec::new();

        for block in &method.var_blocks {
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

        let function = self.module.add_function(&method_fn_name, fn_type, None);
        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);

        // Save and clear current variables
        let saved_vars = std::mem::take(&mut self.variables);
        let saved_fb_instances = std::mem::take(&mut self.fb_instances);
        let saved_struct_type = self.current_struct_type.take();
        let saved_state_ptr = self.current_state_ptr.take();

        let instance_ptr = function.get_nth_param(0).unwrap().into_pointer_value();

        // Set up FB fields as variables via GEP on the instance pointer
        self.current_struct_type = Some(fb_struct_type);
        self.current_state_ptr = Some(instance_ptr);

        for (i, (name, iec_ty)) in fb_field_names
            .iter()
            .zip(fb_field_iec_types.iter())
            .enumerate()
        {
            let ptr = self
                .builder
                .build_struct_gep(fb_struct_type, instance_ptr, i as u32, name)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            self.variables
                .insert(name.to_uppercase(), (ptr, iec_ty.clone()));
        }

        // Allocate method input params as local allocas
        for (i, (name, iec_ty)) in param_names.iter().zip(param_iec_types.iter()).enumerate() {
            let llvm_ty = self.iec_to_llvm_type(iec_ty);
            let alloca = self
                .builder
                .build_alloca(llvm_ty, name)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            // Param 0 is instance_ptr, so method params start at index 1
            self.builder
                .build_store(alloca, function.get_nth_param((i + 1) as u32).unwrap())
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            self.variables
                .insert(name.to_uppercase(), (alloca, iec_ty.clone()));
        }

        // Allocate local vars (VAR, VAR_TEMP, etc. — not VAR_INPUT)
        for block in &method.var_blocks {
            if block.kind != VarBlockKind::VarInput {
                for decl in &block.declarations {
                    let iec_ty = self.resolve_type_spec(&decl.type_spec);
                    let llvm_ty = self.iec_to_llvm_type(&iec_ty);
                    let alloca = self
                        .builder
                        .build_alloca(llvm_ty, &decl.name.name)
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
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

        // Return value variable (method name = return value, like functions)
        if ret_iec_ty != IecType::Void {
            let ret_llvm = self.iec_to_llvm_type(&ret_iec_ty);
            let ret_alloca = self
                .builder
                .build_alloca(ret_llvm, &method.name.name)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            // Initialize to zero
            self.builder
                .build_store(ret_alloca, ret_llvm.const_zero())
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            self.variables.insert(
                method.name.name.to_uppercase(),
                (ret_alloca, ret_iec_ty.clone()),
            );
        }

        // Add global variables
        self.add_globals_to_variables()?;

        // Compile method body
        for stmt in &method.body {
            self.compile_statement(stmt, function)?;
        }

        // Return
        if ret_iec_ty != IecType::Void {
            let ret_llvm_ty = self.iec_to_llvm_type(&ret_iec_ty);
            if let Some((ptr, _)) = self.variables.get(&method.name.name.to_uppercase()) {
                let val = self
                    .builder
                    .build_load(ret_llvm_ty, *ptr, "retval")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                self.builder
                    .build_return(Some(&val))
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            } else {
                self.builder
                    .build_return(Some(&ret_llvm_ty.const_zero()))
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            }
        } else {
            self.builder
                .build_return(None)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        }

        // Restore saved state
        self.variables = saved_vars;
        self.fb_instances = saved_fb_instances;
        self.current_struct_type = saved_struct_type;
        self.current_state_ptr = saved_state_ptr;

        let params: Vec<(String, IecType)> = param_names
            .into_iter()
            .zip(param_iec_types.into_iter())
            .collect();

        Ok(MethodInfo {
            fn_name: method_fn_name,
            params,
            return_type: ret_iec_ty,
        })
    }

    fn compile_statement(
        &mut self,
        stmt: &Statement,
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        match &stmt.kind {
            StatementKind::Assignment { target, value } => {
                // Try string function assignment first (CONCAT, LEFT, RIGHT, MID)
                if !self.try_compile_string_assignment(target, value, function)? {
                    if let Some(ptr) = self.compile_lvalue_with_fn(target, function)? {
                        if let Some(val) = self.compile_expression(value, function)? {
                            // Match the store width to the destination. Without this a
                            // narrow RHS (integer literals default to INT/i16) stored
                            // into a wider slot wrote only part of it.
                            // Match the store width to the destination, but take the
                            // *source's* signedness: `acc : DINT := raw_byte;` where
                            // raw_byte is 16#FF must widen to 255, not to -1.
                            let val = match self.lvalue_iec_type(target) {
                                Some(ty) => {
                                    let src = self.rvalue_iec_type(value);
                                    self.coerce_value(val, src.as_ref(), &ty)?
                                }
                                None => val,
                            };
                            self.builder
                                .build_store(ptr, val)
                                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                        }
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
                    } else if ident.name.to_uppercase() == "PRINT" {
                        // PRINT('string literal') or PRINT(string_var)
                        // Emits a call to extern void plcc_print(i8*)
                        self.compile_print_call(args, function)?;
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
                } else if let ExpressionKind::MemberAccess { object, member } = &callee.kind {
                    // Method call: obj.Method(args)
                    if let ExpressionKind::Identifier(ident) = &object.kind {
                        if self.fb_instances.contains_key(&ident.name.to_uppercase()) {
                            self.compile_method_call(&ident.name, &member.name, args, function)?;
                        } else {
                            // Not an FB instance — fall through to expression compilation
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
            StatementKind::Return { .. } | StatementKind::Continue | StatementKind::Empty => {
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
            .build_struct_gep(
                parent_struct_type,
                parent_state_ptr,
                info.field_index,
                &format!("{}_ptr", instance_name),
            )
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
                        .build_struct_gep(
                            info.struct_type,
                            fb_ptr,
                            field_idx as u32,
                            &arg_name.name,
                        )
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    // Widen/narrow to the declared input type. Integer literals
                    // default to INT (i16), so `ctr(PV := 100000)` on a DINT input
                    // used to store a truncated 16-bit value into a 32-bit field.
                    let src = self.rvalue_iec_type(&arg.value);
                    let val = self.coerce_value(val, src.as_ref(), &info.fields[field_idx].1)?;
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
            .build_call(
                scan_fn,
                &[fb_ptr.into()],
                &format!("{}_call", instance_name),
            )
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        Ok(())
    }

    /// Compile a method call: `instance.MethodName(args)`
    /// Returns the method's return value (if any).
    fn compile_method_call(
        &mut self,
        instance_name: &str,
        method_name: &str,
        args: &[CallArg],
        function: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>, CodegenError> {
        let info = self
            .fb_instances
            .get(&instance_name.to_uppercase())
            .ok_or_else(|| {
                CodegenError::UndefinedVariable(format!("FB instance '{instance_name}' not found"))
            })?
            .clone();

        let method_info = info
            .methods
            .get(&method_name.to_uppercase())
            .ok_or_else(|| {
                CodegenError::UndefinedVariable(format!(
                    "method '{method_name}' not found on FB type '{}'",
                    info.fb_type_name
                ))
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
            .build_struct_gep(
                parent_struct_type,
                parent_state_ptr,
                info.field_index,
                &format!("{}_method_ptr", instance_name),
            )
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        // Build argument list: instance pointer + method params
        let mut call_args: Vec<BasicValueEnum<'ctx>> = vec![fb_ptr.into()];

        // Method params can be positional or named
        if !args.is_empty() && args[0].name.is_some() {
            // Named arguments — match by name to method param order
            for (param_name, _param_ty) in &method_info.params {
                let arg = args
                    .iter()
                    .find(|a| {
                        a.name
                            .as_ref()
                            .map(|n| n.name.eq_ignore_ascii_case(param_name))
                            .unwrap_or(false)
                    })
                    .ok_or_else(|| {
                        CodegenError::LlvmError(format!(
                            "missing argument '{param_name}' for method '{method_name}'"
                        ))
                    })?;
                if let Some(val) = self.compile_expression(&arg.value, function)? {
                    call_args.push(val);
                }
            }
        } else {
            // Positional arguments
            for arg in args {
                if let Some(val) = self.compile_expression(&arg.value, function)? {
                    call_args.push(val);
                }
            }
        }

        let method_fn = self
            .module
            .get_function(&method_info.fn_name)
            .ok_or_else(|| {
                CodegenError::LlvmError(format!(
                    "method function '{}' not found",
                    method_info.fn_name
                ))
            })?;

        let call_args_meta: Vec<inkwell::values::BasicMetadataValueEnum> =
            call_args.iter().map(|v| (*v).into()).collect();
        let call = self
            .builder
            .build_call(
                method_fn,
                &call_args_meta,
                &format!("{}_{}_call", instance_name, method_name),
            )
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        match call.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(v) => Ok(Some(v)),
            inkwell::values::ValueKind::Instruction(_) => Ok(None),
        }
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
            self.branch_to_join(merge_bb)?;
        } else {
            let else_bb = self.context.append_basic_block(function, "else");
            self.builder
                .build_conditional_branch(self.to_i1(cond_val.into_int_value())?, then_bb, else_bb)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

            self.builder.position_at_end(then_bb);
            for stmt in then_body {
                self.compile_statement(stmt, function)?;
            }
            self.branch_to_join(merge_bb)?;

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
                    // Always a fresh block, never `merge_bb` itself. Aliasing the last
                    // ELSIF's false edge onto the join meant the fall-through branch
                    // emitted below landed *in* the join block as `merge: br %merge` —
                    // an infinite self-loop, with the rest of the enclosing body
                    // appended after that terminator.
                    let elsif_else = self.context.append_basic_block(function, "elsif_else");

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
                    self.branch_to_join(merge_bb)?;

                    self.builder.position_at_end(elsif_else);
                }
            }

            if let Some(body) = else_body {
                for stmt in body {
                    self.compile_statement(stmt, function)?;
                }
            }
            self.branch_to_join(merge_bb)?;
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
        let (cur_i, to_i) =
            self.match_int_widths(cur_val.into_int_value(), to_val.into_int_value())?;
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
            .build_int_compare(predicate, cur_i, to_i, "for_cond")
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
            cur_val2
                .into_int_value()
                .get_type()
                .const_int(1, false)
                .into()
        };
        let (cur_i, step_i) =
            self.match_int_widths(cur_val2.into_int_value(), step.into_int_value())?;
        let next_val = self
            .builder
            .build_int_add(cur_i, step_i, "next")
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        self.builder
            .build_store(var_ptr, next_val)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        self.branch_to_join(loop_bb)?;

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
        // BOOL is an i8 in this ABI; a branch condition must be i1.
        let cond_bool = self.to_i1(cond_val.into_int_value())?;
        self.builder
            .build_conditional_branch(cond_bool, body_bb, end_bb)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        self.builder.position_at_end(body_bb);
        for stmt in body {
            self.compile_statement(stmt, function)?;
        }
        self.branch_to_join(cond_bb)?;

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
        self.branch_to_join(cond_bb)?;

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
        let mut cases: Vec<(
            inkwell::values::IntValue<'ctx>,
            inkwell::basic_block::BasicBlock<'ctx>,
        )> = Vec::new();
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
                            cases.push((sel_int_ty.const_int(*v as u64, true), bb));
                        }
                    }
                    CaseLabel::Range(lo, hi) => {
                        if let (
                            ExpressionKind::IntegerLiteral(lo_v),
                            ExpressionKind::IntegerLiteral(hi_v),
                        ) = (&lo.kind, &hi.kind)
                        {
                            for v in *lo_v..=*hi_v {
                                cases.push((sel_int_ty.const_int(v as u64, true), bb));
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
            self.branch_to_join(end_bb)?;
        }

        // Else body
        if let Some(body) = else_body {
            self.builder.position_at_end(else_bb);
            for stmt in body {
                self.compile_statement(stmt, function)?;
            }
            self.branch_to_join(end_bb)?;
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

    /// IEC type of an assignable expression, when it can be determined statically.
    ///
    /// Assignment stores the compiled RHS straight into the target pointer. With
    /// opaque pointers there is nothing to check the width against, so without this
    /// an `INT` expression assigned to a `TIME` (i64) field stored only two of the
    /// eight bytes and left the rest stale. `ET := 0;` was quietly broken.
    ///
    /// Returns `None` for anything not recognized, in which case the store happens
    /// unchanged — no behavior is *removed* by this, only widths corrected.
    fn lvalue_iec_type(&self, expr: &Expression) -> Option<IecType> {
        match &expr.kind {
            ExpressionKind::Identifier(ident) => self
                .variables
                .get(&ident.name.to_uppercase())
                .map(|(_, t)| t.clone()),
            ExpressionKind::Parenthesized(inner) => self.lvalue_iec_type(inner),
            ExpressionKind::ArrayIndex { array, .. } => match self.lvalue_iec_type(array)? {
                IecType::Array { element_type, .. } => Some(*element_type),
                _ => None,
            },
            ExpressionKind::MemberAccess { object, member } => {
                let ExpressionKind::Identifier(ident) = &object.kind else {
                    return None;
                };
                // FB instance field (t.ET, ctr.CV, ...)
                if let Some(info) = self.fb_instances.get(&ident.name.to_uppercase()) {
                    return info
                        .fields
                        .iter()
                        .find(|(name, _)| name.eq_ignore_ascii_case(&member.name))
                        .map(|(_, t)| t.clone());
                }
                // Plain STRUCT field
                match self
                    .variables
                    .get(&ident.name.to_uppercase())
                    .map(|(_, t)| t)
                {
                    Some(IecType::Struct { fields, .. }) => fields
                        .iter()
                        .find(|(name, _)| name.eq_ignore_ascii_case(&member.name))
                        .map(|(_, t)| t.clone()),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// IEC type of a *value-producing* expression, when it can be determined statically.
    ///
    /// Only used to decide whether widening the value is a zero-extension or a
    /// sign-extension, so it is deliberately conservative: `None` means "no static
    /// type", and the caller falls back to the destination's signedness.
    fn rvalue_iec_type(&self, expr: &Expression) -> Option<IecType> {
        match &expr.kind {
            ExpressionKind::Identifier(_)
            | ExpressionKind::ArrayIndex { .. }
            | ExpressionKind::MemberAccess { .. } => self.lvalue_iec_type(expr),
            ExpressionKind::Parenthesized(inner) => self.rvalue_iec_type(inner),
            // Negation only makes sense on a signed value, and the result is signed
            // regardless of what went in. NOT preserves its operand's type.
            ExpressionKind::UnaryOp { op, operand } => match op {
                UnaryOp::Neg => None,
                UnaryOp::Not => self.rvalue_iec_type(operand),
            },
            ExpressionKind::BinaryOp { op, left, right } => match op {
                // Comparisons yield BOOL whatever the operands were. Getting this
                // wrong would sign-extend an i1 `1` into 16#FF.
                BinaryOp::Equal
                | BinaryOp::NotEqual
                | BinaryOp::Less
                | BinaryOp::LessEqual
                | BinaryOp::Greater
                | BinaryOp::GreaterEqual => Some(IecType::Bool),
                _ => match (self.rvalue_iec_type(left), self.rvalue_iec_type(right)) {
                    // The wider operand's type governs; a literal operand contributes
                    // nothing, so the typed side wins.
                    (Some(l), Some(r)) => {
                        Some(if r.bit_size().unwrap_or(0) > l.bit_size().unwrap_or(0) {
                            r
                        } else {
                            l
                        })
                    }
                    (Some(t), None) | (None, Some(t)) => Some(t),
                    (None, None) => None,
                },
            },
            _ => None,
        }
    }

    fn compile_lvalue_inner(
        &mut self,
        expr: &Expression,
        function: Option<FunctionValue<'ctx>>,
    ) -> Result<Option<PointerValue<'ctx>>, CodegenError> {
        match &expr.kind {
            ExpressionKind::Identifier(ident) => Ok(self
                .variables
                .get(&ident.name.to_uppercase())
                .map(|(ptr, _)| *ptr)),
            ExpressionKind::ArrayIndex { array, indices } => {
                // Get the array variable's pointer and IEC type
                if let ExpressionKind::Identifier(ident) = &array.kind {
                    if let Some((arr_ptr, iec_ty)) =
                        self.variables.get(&ident.name.to_uppercase()).cloned()
                    {
                        if let IecType::Array { ref ranges, .. } = iec_ty {
                            let ranges = ranges.clone();
                            let function = function.ok_or_else(|| {
                                CodegenError::LlvmError(
                                    "array index in lvalue requires function context".into(),
                                )
                            })?;
                            let arr_llvm_ty = self.iec_to_llvm_type(&iec_ty);

                            if indices.len() == 1 {
                                let idx_val = self
                                    .compile_expression(&indices[0], function)?
                                    .ok_or_else(|| {
                                        CodegenError::LlvmError(
                                            "failed to compile array index".into(),
                                        )
                                    })?;

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
                                        .build_int_s_extend(
                                            adjusted,
                                            self.context.i32_type(),
                                            "idx_ext",
                                        )
                                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                                } else {
                                    adjusted
                                };

                                let zero = self.context.i32_type().const_zero();
                                let elem_ptr = unsafe {
                                    self.builder
                                        .build_in_bounds_gep(
                                            arr_llvm_ty,
                                            arr_ptr,
                                            &[zero, idx_i32],
                                            "arr_elem",
                                        )
                                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                                };
                                Ok(Some(elem_ptr))
                            } else {
                                // Multi-dimensional: flatten to linear index
                                let function = function;
                                let mut linear_idx = self.context.i32_type().const_zero();

                                for (dim, idx_expr) in indices.iter().enumerate() {
                                    let idx_val = self
                                        .compile_expression(idx_expr, function)?
                                        .ok_or_else(|| {
                                            CodegenError::LlvmError(
                                                "failed to compile array index".into(),
                                            )
                                        })?;

                                    let lo = ranges[dim].0;
                                    let idx_int = idx_val.into_int_value();
                                    let idx_i32 = if idx_int.get_type().get_bit_width() < 32 {
                                        self.builder
                                            .build_int_s_extend(
                                                idx_int,
                                                self.context.i32_type(),
                                                "idx_ext",
                                            )
                                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                                    } else {
                                        idx_int
                                    };

                                    let adjusted = if lo != 0 {
                                        let lo_val =
                                            self.context.i32_type().const_int(lo as u64, true);
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
                                    let stride_val =
                                        self.context.i32_type().const_int(stride as u64, false);
                                    let component = self
                                        .builder
                                        .build_int_mul(adjusted, stride_val, "dim_component")
                                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                                    linear_idx = self
                                        .builder
                                        .build_int_add(linear_idx, component, "linear_idx")
                                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                                }

                                let zero = self.context.i32_type().const_zero();
                                let elem_ptr = unsafe {
                                    self.builder
                                        .build_in_bounds_gep(
                                            arr_llvm_ty,
                                            arr_ptr,
                                            &[zero, linear_idx],
                                            "arr_elem",
                                        )
                                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                                };
                                Ok(Some(elem_ptr))
                            }
                        } else {
                            Err(CodegenError::UnsupportedType(format!(
                                "array indexing on non-array type: {}",
                                iec_ty
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
                            .build_struct_gep(
                                parent_struct_type,
                                parent_state_ptr,
                                info.field_index,
                                &format!("{}_fb_lv", ident.name),
                            )
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
                            .build_struct_gep(
                                info.struct_type,
                                fb_ptr,
                                field_idx as u32,
                                &member.name,
                            )
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                        return Ok(Some(field_ptr));
                    }

                    // Fall back to struct field access
                    if let Some((obj_ptr, iec_ty)) =
                        self.variables.get(&ident.name.to_uppercase()).cloned()
                    {
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
            ExpressionKind::IntegerLiteral(v) => Ok(Some(self.int_literal(*v).into())),
            ExpressionKind::RealLiteral(v) => {
                // Default to f32 (REAL) to match common IEC usage
                Ok(Some(self.context.f32_type().const_float(*v).into()))
            }
            ExpressionKind::BoolLiteral(v) => Ok(Some(
                self.context.i8_type().const_int(*v as u64, false).into(),
            )),
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
                    if let Some(result) = self.compile_stdlib_call(&ident.name, args, function)? {
                        return Ok(Some(result));
                    }

                    // Fall back to user-defined functions
                    if let Some(fn_val) = self.module.get_function(&ident.name.to_lowercase()) {
                        let mut compiled_args = Vec::new();
                        for arg in args {
                            if let Some(val) = self.compile_expression(&arg.value, function)? {
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
                } else if let ExpressionKind::MemberAccess { object, member } = &callee.kind {
                    // Method call: obj.Method(args)
                    if let ExpressionKind::Identifier(ident) = &object.kind {
                        if self.fb_instances.contains_key(&ident.name.to_uppercase()) {
                            return self.compile_method_call(
                                &ident.name,
                                &member.name,
                                args,
                                function,
                            );
                        }
                    }
                    Ok(None)
                } else {
                    Ok(None)
                }
            }
            ExpressionKind::TimeLiteral(s) => {
                let ns = parse_time_literal_ns(s);
                Ok(Some(
                    self.context.i64_type().const_int(ns as u64, true).into(),
                ))
            }
            ExpressionKind::DateLiteral(_)
            | ExpressionKind::TodLiteral(_)
            | ExpressionKind::DtLiteral(_) => {
                // Date/time-of-day literals — store as i64 placeholder
                Ok(Some(self.context.i64_type().const_int(0, false).into()))
            }
            ExpressionKind::StringLiteral(_) | ExpressionKind::WstringLiteral(_) => {
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
                        if let Some((_, iec_ty)) =
                            self.variables.get(&ident.name.to_uppercase()).cloned()
                        {
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
                            .build_struct_gep(
                                parent_struct_type,
                                parent_state_ptr,
                                info.field_index,
                                &format!("{}_fb", ident.name),
                            )
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
                            .build_struct_gep(
                                info.struct_type,
                                fb_ptr,
                                field_idx as u32,
                                &member.name,
                            )
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                        let val = self
                            .builder
                            .build_load(
                                field_llvm_ty,
                                field_ptr,
                                &format!("{}.{}", ident.name, member.name),
                            )
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                        return Ok(Some(val));
                    }
                }
                // Fall back to struct field access via lvalue
                if let Some(field_ptr) = self.compile_lvalue_with_fn(expr, function)? {
                    if let ExpressionKind::Identifier(ident) = &object.kind {
                        if let Some((_, iec_ty)) =
                            self.variables.get(&ident.name.to_uppercase()).cloned()
                        {
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
                    self.builder
                        .build_float_ext(fv, target_fty, "fext")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                } else {
                    fv
                }
            } else {
                self.builder
                    .build_signed_int_to_float(left.into_int_value(), target_fty, "itof")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
            };
            let r = if right.is_float_value() {
                let fv = right.into_float_value();
                if fv.get_type() != target_fty {
                    self.builder
                        .build_float_ext(fv, target_fty, "fext")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                } else {
                    fv
                }
            } else {
                self.builder
                    .build_signed_int_to_float(right.into_int_value(), target_fty, "itof")
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
                    return Err(CodegenError::UnsupportedType(format!("float op {:?}", op)));
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
                    let is_zero = self
                        .builder
                        .build_int_compare(IntPredicate::EQ, int_val, zero, "lnot")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    let result = self
                        .builder
                        .build_int_z_extend(is_zero, int_val.get_type(), "lnot_ext")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    Ok(result.into())
                } else {
                    // Bitwise NOT for wider integer types (WORD, DWORD, etc.)
                    Ok(self
                        .builder
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
