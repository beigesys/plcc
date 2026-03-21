// SPDX-License-Identifier: MPL-2.0

use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine, TargetTriple,
};
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum, StructType};
use inkwell::values::{BasicValue, BasicValueEnum, FunctionValue, PointerValue};
use inkwell::{AddressSpace, FloatPredicate, IntPredicate, OptimizationLevel};
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;
use plcc_hir::types::{IecType, TypeRegistry};
use plcc_hir::check::TypeChecker;
use plcc_st::ast::*;

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

pub struct Compiler<'ctx> {
    context: &'ctx Context,
    module: Module<'ctx>,
    builder: Builder<'ctx>,
    variables: HashMap<String, (PointerValue<'ctx>, IecType)>,
    type_registry: TypeRegistry,
    type_checker: TypeChecker,
    /// Target block for EXIT statements inside loops.
    loop_exit_bb: Option<inkwell::basic_block::BasicBlock<'ctx>>,
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
        }
    }

    pub fn compile(&mut self, unit: &CompilationUnit) -> Result<(), CodegenError> {
        // Register types and POUs
        for decl in &unit.declarations {
            if let Declaration::FunctionBlock(fb) = decl {
                self.type_registry.register(
                    fb.name.name.to_uppercase(),
                    IecType::FbInstance(fb.name.name.clone()),
                );
            }
        }

        for decl in &unit.declarations {
            match decl {
                Declaration::Program(p) => self.compile_program(p)?,
                Declaration::Function(f) => self.compile_function(f)?,
                Declaration::FunctionBlock(fb) => self.compile_function_block(fb)?,
                _ => {}
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
            _ => self.context.i32_type().into(), // Fallback
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

        // Set up variables as GEP into the state struct
        self.variables.clear();
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
                let ptr = self
                    .builder
                    .build_struct_gep(struct_type, init_state_ptr, field_idx, &decl.name.name)
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                self.variables
                    .insert(decl.name.name.to_uppercase(), (ptr, iec_ty));

                if let Some(init_expr) = &decl.initializer {
                    if let Some(val) = self.compile_expression(init_expr, init_fn)? {
                        self.builder
                            .build_store(ptr, val)
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
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

        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);

        let state_ptr = function.get_nth_param(0).unwrap().into_pointer_value();

        self.variables.clear();
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
                if let Some(ptr) = self.compile_lvalue(target)? {
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
                // Try to compile as a function call expression
                let call_expr = Expression {
                    kind: ExpressionKind::FunctionCall {
                        callee: Box::new(callee.clone()),
                        args: args.clone(),
                    },
                    span: stmt.span,
                };
                self.compile_expression(&call_expr, function)?;
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

    fn compile_lvalue(
        &mut self,
        expr: &Expression,
    ) -> Result<Option<PointerValue<'ctx>>, CodegenError> {
        match &expr.kind {
            ExpressionKind::Identifier(ident) => {
                Ok(self
                    .variables
                    .get(&ident.name.to_uppercase())
                    .map(|(ptr, _)| *ptr))
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
                    if let Some(fn_val) = self.module.get_function(&ident.name.to_lowercase()) {
                        let mut arg_vals: Vec<BasicMetadataTypeEnum<'ctx>> = Vec::new();
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
