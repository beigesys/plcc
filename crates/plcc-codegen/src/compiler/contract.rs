// SPDX-License-Identifier: MPL-2.0

//! The runtime contract: program instances, the task table, `plcc_init` /
//! `plcc_run_task`, the retain table and the `plcc_app` descriptor that ties them
//! together. A generic runtime links against these symbols and never needs to know
//! a program's name or layout. See `docs/process-image.md`.

use super::image::{AtBinding, IMAGE_INIT_FN};
use super::*;
use crate::direct_address::Area;
use inkwell::module::Linkage;
use inkwell::targets::{ByteOrdering, TargetData};
use inkwell::types::AnyType;
use inkwell::values::{IntValue, StructValue};

/// Version of the `plcc_app` descriptor layout. Bumped on any incompatible change.
pub const ABI_VERSION: u32 = 1;

/// Interval of the implicit task used when the source has no CONFIGURATION.
/// CODESYS's standard project creates its `MainTask` cyclic at T#20ms.
pub(crate) const DEFAULT_TASK_INTERVAL_NS: i64 = 20_000_000;

/// Name of the implicit task used when the source has no CONFIGURATION.
pub const DEFAULT_TASK_NAME: &str = "MainTask";

/// Name of the implicit task that runs program instances declared without `WITH`.
pub const BACKGROUND_TASK_NAME: &str = "__background";

/// Priority of the background task: the lowest possible (IEC: 0 is highest).
pub const BACKGROUND_PRIORITY: u32 = u32::MAX;

/// Options for the implicit task (no CONFIGURATION).
#[derive(Clone, Copy, Debug)]
pub struct TaskOptions {
    pub default_interval_ns: i64,
}

/// Parse a duration such as `T#10ms`, `10ms` or `1s500ms` into nanoseconds.
pub fn parse_duration_ns(text: &str) -> Option<i64> {
    let t = text.trim();
    if !t.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let with_prefix = if t.contains('#') { t.to_string() } else { format!("T#{t}") };
    let ns = super::parse_time_literal_ns(&with_prefix).ok()?;
    (ns >= 0).then_some(ns)
}

/// Move CONFIGURATION / RESOURCE `VAR_GLOBAL` blocks to top level so the ordinary
/// global machinery lays them out. Resource-level scoping is flattened: every such
/// variable is visible to every POU (documented in docs/process-image.md).
pub(crate) fn hoist_configuration_globals(unit: &CompilationUnit) -> Option<CompilationUnit> {
    let has = unit.declarations.iter().any(|d| match d {
        Declaration::Configuration(c) => {
            !c.global_vars.is_empty() || c.resources.iter().any(|r| !r.global_vars.is_empty())
        }
        _ => false,
    });
    if !has {
        return None;
    }
    let mut out = unit.clone();
    for d in &unit.declarations {
        if let Declaration::Configuration(c) = d {
            for b in c
                .global_vars
                .iter()
                .chain(c.resources.iter().flat_map(|r| r.global_vars.iter()))
            {
                out.declarations.push(Declaration::GlobalVarDecl(b.clone()));
            }
        }
    }
    Some(out)
}

/// A PROGRAM type as laid out in LLVM.
#[derive(Clone, Debug)]
pub(super) struct ProgramType<'ctx> {
    pub(super) name: String,
    pub(super) struct_type: StructType<'ctx>,
    pub(super) fields: Vec<PouField>,
    pub(super) retain: Vec<bool>,
}

#[derive(Clone, Debug)]
pub(super) struct InstanceRec<'ctx> {
    pub(super) name: String,
    pub(super) program: usize,
    pub(super) symbol: String,
    pub(super) global: GlobalValue<'ctx>,
    pub(super) connections: Vec<CallArg>,
    pub(super) task: usize,
}

#[derive(Clone, Debug)]
pub(super) struct TaskRec {
    pub(super) name: String,
    pub(super) interval_ns: i64,
    pub(super) priority: u32,
    pub(super) single: Option<Expression>,
    pub(super) instances: Vec<usize>,
}

#[derive(Clone, Debug)]
pub(super) struct RetainRec<'ctx> {
    pub(super) path: String,
    pub(super) symbol: String,
    pub(super) base: StructType<'ctx>,
    pub(super) indices: Vec<u32>,
    pub(super) ty: IecType,
}

/// What `finish_runtime_contract` decided, kept for header / symbol emission.
#[derive(Clone, Debug)]
pub(super) struct ContractLayout<'ctx> {
    pub(super) programs: Vec<ProgramType<'ctx>>,
    pub(super) instances: Vec<InstanceRec<'ctx>>,
    pub(super) tasks: Vec<TaskRec>,
    pub(super) retain: Vec<RetainRec<'ctx>>,
    pub(super) retain_signature: u32,
    /// `plcc_globals` fields: (name, type, retain, AT address display).
    pub(super) globals: Option<(StructType<'ctx>, Vec<(String, IecType, bool, Option<String>)>)>,
}

fn located(message: impl Into<String>, span: plcc_st::span::Span) -> CodegenError {
    CodegenError::Located {
        message: message.into(),
        span,
    }
}

fn llvm(e: impl std::fmt::Display) -> CodegenError {
    CodegenError::LlvmError(e.to_string())
}

pub(super) fn c_ident(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect()
}

/// FNV-1a, 32-bit: the retain-layout signature.
fn fnv1a(data: &[u8], mut h: u32) -> u32 {
    for b in data {
        h ^= *b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

impl<'ctx> Compiler<'ctx> {
    /// Interval of the implicit task used when the source has no CONFIGURATION.
    pub fn set_task_options(&mut self, opts: TaskOptions) {
        self.rt.default_task_interval_ns = opts.default_interval_ns;
    }

    fn program_type(&mut self, prog: &ProgramDecl) -> ProgramType<'ctx> {
        let fields = self.resolve_pou_fields(&prog.var_blocks);
        let types: Vec<BasicTypeEnum<'ctx>> = fields
            .iter()
            .map(|f| self.iec_to_llvm_type(&f.stored))
            .collect();
        let retain = prog
            .var_blocks
            .iter()
            .flat_map(|b| b.declarations.iter().map(move |_| b.is_retain))
            .collect();
        ProgramType {
            name: prog.name.name.clone(),
            // Literal struct types are uniqued by content: this is the very type
            // `compile_program` built.
            struct_type: self.context.struct_type(&types, false),
            fields,
            retain,
        }
    }

    /// Evaluate a TASK property to an integer constant.
    fn task_property_int(&self, expr: &Expression, ty: &IecType) -> Option<i64> {
        match &expr.kind {
            ExpressionKind::TimeLiteral(s) => super::parse_time_literal_ns(s).ok(),
            ExpressionKind::Parenthesized(inner) => self.task_property_int(inner, ty),
            _ => {
                let v = self.eval_const_initializer(expr, ty)?;
                match v {
                    BasicValueEnum::IntValue(iv) => iv.get_sign_extended_constant(),
                    _ => None,
                }
            }
        }
    }

    /// Decide tasks and program instances from the CONFIGURATION (or the implicit
    /// default), then emit everything the runtime links against.
    pub(crate) fn finish_runtime_contract(
        &mut self,
        unit: &CompilationUnit,
    ) -> Result<(), CodegenError> {
        for name in ["plcc_init", "plcc_run_task", "plcc_app", "plcc_get_app"] {
            if self.module.get_function(name).is_some() || self.module.get_global(name).is_some()
            {
                return Err(CodegenError::UnsupportedType(format!(
                    "`{name}` is reserved for the runtime entry points (a PROGRAM named `plcc` \
                     would define `plcc_init`); rename the POU"
                )));
            }
        }

        let prog_decls: Vec<&ProgramDecl> = unit
            .declarations
            .iter()
            .filter_map(|d| match d {
                Declaration::Program(p) => Some(p),
                _ => None,
            })
            .collect();
        let mut programs = Vec::new();
        for p in &prog_decls {
            programs.push(self.program_type(p));
        }
        let configs: Vec<&ConfigurationDecl> = unit
            .declarations
            .iter()
            .filter_map(|d| match d {
                Declaration::Configuration(c) => Some(c),
                _ => None,
            })
            .collect();
        if configs.len() > 1 {
            return Err(located(
                format!(
                    "only one CONFIGURATION per compiled module is supported (`{}` is the \
                     second)",
                    configs[1].name.name
                ),
                configs[1].name.span,
            ));
        }

        let mut tasks: Vec<TaskRec> = Vec::new();
        // (instance name, program index, symbol, connections, task index)
        let mut planned: Vec<(String, usize, String, Vec<CallArg>, usize)> = Vec::new();
        let mut used_symbols: std::collections::HashSet<String> = Default::default();
        let mut unique = |base: String| {
            let mut s = base.clone();
            let mut n = 1;
            while !used_symbols.insert(s.clone()) {
                n += 1;
                s = format!("{base}_{n}");
            }
            s
        };

        match configs.first() {
            None => {
                if !programs.is_empty() {
                    tasks.push(TaskRec {
                        name: DEFAULT_TASK_NAME.to_string(),
                        interval_ns: self.rt.default_task_interval_ns,
                        priority: 1,
                        single: None,
                        instances: Vec::new(),
                    });
                    for (i, p) in programs.iter().enumerate() {
                        let sym = unique(format!("plcc_inst_{}", c_ident(&p.name)));
                        planned.push((p.name.clone(), i, sym, Vec::new(), 0));
                    }
                }
            }
            Some(cfg) => {
                let multi = cfg.resources.len() > 1;
                let mut background: Option<usize> = None;
                for res in &cfg.resources {
                    let mut local: HashMap<String, usize> = HashMap::new();
                    for t in &res.tasks {
                        let key = t.name.name.to_uppercase();
                        if local.contains_key(&key) {
                            return Err(located(
                                format!("TASK `{}` is declared twice", t.name.name),
                                t.name.span,
                            ));
                        }
                        let mut rec = TaskRec {
                            name: if multi {
                                format!("{}.{}", res.name.name, t.name.name)
                            } else {
                                t.name.name.clone()
                            },
                            interval_ns: 0,
                            priority: 0,
                            single: None,
                            instances: Vec::new(),
                        };
                        for (k, v) in &t.properties {
                            match k.name.to_uppercase().as_str() {
                                "INTERVAL" => {
                                    rec.interval_ns = self
                                        .task_property_int(v, &IecType::Time)
                                        .filter(|n| *n >= 0)
                                        .ok_or_else(|| {
                                            located(
                                                "INTERVAL must be a constant, non-negative \
                                                 TIME (e.g. T#10ms)",
                                                v.span,
                                            )
                                        })?;
                                }
                                "PRIORITY" => {
                                    rec.priority = self
                                        .task_property_int(v, &IecType::Udint)
                                        .and_then(|n| u32::try_from(n).ok())
                                        .ok_or_else(|| {
                                            located(
                                                "PRIORITY must be a constant, non-negative \
                                                 integer (0 is the highest priority)",
                                                v.span,
                                            )
                                        })?;
                                }
                                "SINGLE" => rec.single = Some(v.clone()),
                                other => {
                                    return Err(located(
                                        format!(
                                            "unknown TASK property `{other}` (expected \
                                             INTERVAL, PRIORITY or SINGLE)"
                                        ),
                                        k.span,
                                    ));
                                }
                            }
                        }
                        local.insert(key, tasks.len());
                        tasks.push(rec);
                    }
                    for pc in &res.program_configs {
                        let Some(pidx) = programs
                            .iter()
                            .position(|p| p.name.eq_ignore_ascii_case(&pc.program_type.name))
                        else {
                            return Err(located(
                                format!(
                                    "program instance `{}` names `{}`, which is not a PROGRAM",
                                    pc.name.name, pc.program_type.name
                                ),
                                pc.program_type.span,
                            ));
                        };
                        let tidx = match &pc.task {
                            Some(t) => *local.get(&t.name.to_uppercase()).ok_or_else(|| {
                                located(
                                    format!(
                                        "program instance `{}` runs WITH `{}`, which is not a \
                                         TASK of RESOURCE `{}`",
                                        pc.name.name, t.name, res.name.name
                                    ),
                                    t.span,
                                )
                            })?,
                            None => *background.get_or_insert_with(|| {
                                tasks.push(TaskRec {
                                    name: BACKGROUND_TASK_NAME.to_string(),
                                    interval_ns: 0,
                                    priority: BACKGROUND_PRIORITY,
                                    single: None,
                                    instances: Vec::new(),
                                });
                                tasks.len() - 1
                            }),
                        };
                        let name = format!("{}.{}", res.name.name, pc.name.name);
                        if planned.iter().any(|p| p.0.eq_ignore_ascii_case(&name)) {
                            return Err(located(
                                format!("program instance `{}` is declared twice", pc.name.name),
                                pc.name.span,
                            ));
                        }
                        // `PROGRAM Main WITH t : Main;` — the one instance, named
                        // like its program, of a program other POUs may call
                        // (TwinCAT's model): it shares the callable instance.
                        let callable = pc.name.name.eq_ignore_ascii_case(&pc.program_type.name)
                            && self
                                .program_instances
                                .contains_key(&pc.program_type.name.to_uppercase());
                        let sym = if callable {
                            unique(format!("plcc_inst_{}", c_ident(&pc.program_type.name)))
                        } else {
                            unique(format!(
                                "plcc_inst_{}_{}",
                                c_ident(&res.name.name),
                                c_ident(&pc.name.name)
                            ))
                        };
                        planned.push((name, pidx, sym, pc.connections.clone(), tidx));
                    }
                }
            }
        }

        // Statically allocated program instance state.
        let mut instances = Vec::new();
        for (name, pidx, sym, connections, tidx) in planned {
            let st = programs[pidx].struct_type;
            // A program another POU calls already has its instance (see
            // `layout_callable_programs`); it runs when called, not as a task.
            let called = self.called_programs.contains(&name.to_uppercase());
            let g = match self.module.get_global(&sym) {
                Some(g) => g,
                None => {
                    let g = self.module.add_global(st, None, &sym);
                    g.set_initializer(&st.const_zero());
                    g.set_alignment(8);
                    g
                }
            };
            if !called {
                tasks[tidx].instances.push(instances.len());
            }
            instances.push(InstanceRec {
                name,
                program: pidx,
                symbol: sym,
                global: g,
                connections,
                task: tidx,
            });
        }

        let single_fns = self.emit_single_fns(&tasks)?;
        self.emit_plcc_init(&programs, &instances)?;
        self.emit_run_task(&programs, &instances, &tasks)?;

        // Everything that can name an image location has been emitted.
        self.finish_process_image()?;

        let (retain, globals) = self.collect_retain(unit, &programs, &instances);
        let mut sig = 0x811c_9dc5u32;
        for r in &retain {
            sig = fnv1a(format!("{}:{};", r.path, r.ty).as_bytes(), sig);
        }
        self.emit_tables(&programs, &instances, &tasks, &single_fns, &retain, sig)?;

        self.rt.layout = Some(ContractLayout {
            programs,
            instances,
            tasks,
            retain,
            retain_signature: sig,
            globals,
        });
        Ok(())
    }

    /// `uint8_t plcc_task_single_<i>(void)`: the current value of a task's SINGLE.
    fn emit_single_fns(
        &mut self,
        tasks: &[TaskRec],
    ) -> Result<Vec<Option<FunctionValue<'ctx>>>, CodegenError> {
        let mut out = Vec::new();
        for (i, t) in tasks.iter().enumerate() {
            let Some(expr) = &t.single else {
                out.push(None);
                continue;
            };
            let i8t = self.context.i8_type();
            let f = self.module.add_function(
                &format!("plcc_task_single_{i}"),
                i8t.fn_type(&[], false),
                Some(Linkage::Internal),
            );
            let entry = self.context.append_basic_block(f, "entry");
            self.builder.position_at_end(entry);
            self.variables.clear();
            self.add_globals_to_variables()?;
            let Some(v) = self.compile_expression(expr, f)? else {
                return Err(located(
                    format!("SINGLE of TASK `{}` must be a BOOL variable or expression", t.name),
                    expr.span,
                ));
            };
            let src = self.rvalue_iec_type(expr);
            let v = self.coerce_value(v, src.as_ref(), &IecType::Bool)?;
            self.builder.build_return(Some(&v)).map_err(llvm)?;
            out.push(Some(f));
        }
        Ok(out)
    }

    /// `void plcc_init(void)`: VAR_GLOBAL AT initializers, then each instance's
    /// `<program>_init` (which also initializes VAR_GLOBALs).
    fn emit_plcc_init(
        &mut self,
        programs: &[ProgramType<'ctx>],
        instances: &[InstanceRec<'ctx>],
    ) -> Result<(), CodegenError> {
        let f = self
            .module
            .add_function("plcc_init", self.context.void_type().fn_type(&[], false), None);
        let entry = self.context.append_basic_block(f, "entry");
        self.builder.position_at_end(entry);
        if let Some(g) = self.module.get_function(GLOBALS_INIT_FN) {
            self.builder.build_call(g, &[], "").map_err(llvm)?;
        }
        if let Some(g) = self.module.get_function(IMAGE_INIT_FN) {
            self.builder.build_call(g, &[], "").map_err(llvm)?;
        }
        for inst in instances {
            let init = self.declare_state_fn(&Self::init_fn_name_for(&programs[inst.program].name));
            self.builder
                .build_call(init, &[inst.global.as_pointer_value().into()], "")
                .map_err(llvm)?;
        }
        // A callable program no task runs (under a CONFIGURATION that does not
        // instantiate it) still gets its initial values.
        let mut callable: Vec<(String, GlobalValue<'ctx>)> = self
            .program_instances
            .iter()
            .filter(|(_, g)| !instances.iter().any(|i| i.global == **g))
            .map(|(n, g)| (n.clone(), *g))
            .collect();
        callable.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, g) in callable {
            let init = self.declare_state_fn(&Self::init_fn_name_for(&name));
            self.builder
                .build_call(init, &[g.as_pointer_value().into()], "")
                .map_err(llvm)?;
        }
        self.builder.build_return(None).map_err(llvm)?;
        Ok(())
    }

    /// `void plcc_run_task(uint32_t task)`: run every program instance of one task,
    /// in declaration order, with its input / output connections.
    fn emit_run_task(
        &mut self,
        programs: &[ProgramType<'ctx>],
        instances: &[InstanceRec<'ctx>],
        tasks: &[TaskRec],
    ) -> Result<(), CodegenError> {
        let i32t = self.context.i32_type();
        let f = self.module.add_function(
            "plcc_run_task",
            self.context.void_type().fn_type(&[i32t.into()], false),
            None,
        );
        let entry = self.context.append_basic_block(f, "entry");
        let done = self.context.append_basic_block(f, "done");
        let mut cases = Vec::new();
        for (ti, task) in tasks.iter().enumerate() {
            let bb = self.context.append_basic_block(f, &format!("task_{ti}"));
            self.builder.position_at_end(bb);
            for &ii in &task.instances {
                self.emit_instance_scan(&programs[instances[ii].program], &instances[ii], f)?;
            }
            self.branch_to_join(done)?;
            cases.push((i32t.const_int(ti as u64, false), bb));
        }
        self.builder.position_at_end(entry);
        let idx = f.get_nth_param(0).ok_or_else(|| llvm("missing task param"))?;
        self.builder
            .build_switch(idx.into_int_value(), done, &cases)
            .map_err(llvm)?;
        self.builder.position_at_end(done);
        self.builder.build_return(None).map_err(llvm)?;
        Ok(())
    }

    fn emit_instance_scan(
        &mut self,
        prog: &ProgramType<'ctx>,
        inst: &InstanceRec<'ctx>,
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        const CONN: &str = "__PLCC_CONNECTION";
        let state = inst.global.as_pointer_value();
        let (inputs, outputs): (Vec<&CallArg>, Vec<&CallArg>) =
            inst.connections.iter().partition(|c| !c.is_output);

        // Bind the connected field under a reserved name and reuse the ordinary
        // assignment path, so coercions (and AT/bit targets) behave exactly as in a
        // program body.
        let connect = |this: &mut Self, arg: &CallArg, input: bool| -> Result<(), CodegenError> {
            let Some(pname) = &arg.name else {
                return Err(located(
                    "program connections must be named (`param := value` or `param => target`)",
                    arg.span,
                ));
            };
            let Some(idx) = prog
                .fields
                .iter()
                .position(|fld| fld.name.eq_ignore_ascii_case(&pname.name))
            else {
                return Err(located(
                    format!("PROGRAM `{}` has no variable `{}`", prog.name, pname.name),
                    pname.span,
                ));
            };
            let field = prog.fields[idx].clone();
            if field.is_in_out || field.at.is_some() {
                return Err(located(
                    format!(
                        "`{}` cannot be connected: VAR_IN_OUT and AT variables have no \
                         instance-owned value",
                        pname.name
                    ),
                    pname.span,
                ));
            }
            this.variables.clear();
            this.add_globals_to_variables()?;
            let slot = this
                .builder
                .build_struct_gep(prog.struct_type, state, idx as u32, CONN)
                .map_err(llvm)?;
            this.variables.insert(CONN.into(), (slot, field.declared.clone()));
            let conn = Expression {
                kind: ExpressionKind::Identifier(Ident::new(CONN, pname.span)),
                span: pname.span,
            };
            let stmt = Statement {
                kind: if input {
                    StatementKind::Assignment {
                        target: conn,
                        value: arg.value.clone(),
                    }
                } else {
                    StatementKind::Assignment {
                        target: arg.value.clone(),
                        value: conn,
                    }
                },
                span: arg.span,
            };
            this.compile_statement(&stmt, function)
        };
        for arg in inputs {
            connect(self, arg, true)?;
        }
        let scan = self.declare_state_fn(&Self::scan_fn_name_for(&prog.name));
        self.builder
            .build_call(scan, &[state.into()], "")
            .map_err(llvm)?;
        for arg in outputs {
            connect(self, arg, false)?;
        }
        Ok(())
    }

    /// Every RETAIN variable reachable from a static instance or VAR_GLOBAL.
    #[allow(clippy::type_complexity)]
    fn collect_retain(
        &mut self,
        unit: &CompilationUnit,
        programs: &[ProgramType<'ctx>],
        instances: &[InstanceRec<'ctx>],
    ) -> (
        Vec<RetainRec<'ctx>>,
        Option<(StructType<'ctx>, Vec<(String, IecType, bool, Option<String>)>)>,
    ) {
        let fb_decls: HashMap<String, Vec<VarBlock>> = unit
            .declarations
            .iter()
            .filter_map(|d| match d {
                Declaration::FunctionBlock(f) => {
                    Some((f.name.name.to_uppercase(), f.var_blocks.clone()))
                }
                Declaration::Class(c) => Some((c.name.name.to_uppercase(), c.var_blocks.clone())),
                _ => None,
            })
            .collect();
        let mut out = Vec::new();

        for inst in instances {
            let prog = &programs[inst.program];
            for (i, field) in prog.fields.iter().enumerate() {
                self.retain_walk(
                    &fb_decls,
                    &format!("{}.{}", inst.name, field.name),
                    &inst.symbol,
                    prog.struct_type,
                    vec![0, i as u32],
                    field,
                    prog.retain[i],
                    &mut out,
                    0,
                );
            }
        }

        let mut globals = None;
        if let Some((_, gstruct, names)) = self.global_var.clone() {
            let retain_flags: Vec<bool> = unit
                .declarations
                .iter()
                .filter_map(|d| match d {
                    Declaration::GlobalVarDecl(b) => Some(b),
                    _ => None,
                })
                .flat_map(|b| b.declarations.iter().map(move |_| b.is_retain))
                .collect();
            let mut desc = Vec::new();
            for (i, (name, ty)) in names.iter().enumerate() {
                let retain = retain_flags.get(i).copied().unwrap_or(false);
                let at = self
                    .rt
                    .global_at
                    .iter()
                    .find(|g| g.0 == name.to_uppercase())
                    .map(|g| g.1);
                let mut field = PouField::plain(name.clone(), ty.clone());
                field.at = at;
                self.retain_walk(
                    &fb_decls,
                    &format!("GLOBAL.{name}"),
                    "plcc_globals",
                    gstruct,
                    vec![0, i as u32],
                    &field,
                    retain,
                    &mut out,
                    0,
                );
                desc.push((name.clone(), ty.clone(), retain, at.map(|a| a.to_string())));
            }
            globals = Some((gstruct, desc));
        }
        (out, globals)
    }

    #[allow(clippy::too_many_arguments)]
    fn retain_walk(
        &mut self,
        fb_decls: &HashMap<String, Vec<VarBlock>>,
        path: &str,
        symbol: &str,
        base: StructType<'ctx>,
        indices: Vec<u32>,
        field: &PouField,
        retain: bool,
        out: &mut Vec<RetainRec<'ctx>>,
        depth: usize,
    ) {
        if field.at.is_some() || field.is_in_out || depth > 16 {
            return;
        }
        if retain {
            out.push(RetainRec {
                path: path.to_string(),
                symbol: symbol.to_string(),
                base,
                indices,
                ty: field.declared.clone(),
            });
            return;
        }
        // IEC: RETAIN declared inside a FUNCTION_BLOCK applies to each instance.
        if let IecType::FbInstance(fb) = &field.declared
            && let Some(blocks) = fb_decls.get(&fb.to_uppercase()).cloned()
        {
            let inner = self.resolve_pou_fields(&blocks);
            let flags: Vec<bool> = blocks
                .iter()
                .flat_map(|b| {
                    b.declarations
                        .iter()
                        .map(move |_| b.is_retain && b.kind != VarBlockKind::VarTemp)
                })
                .collect();
            for (j, f) in inner.iter().enumerate() {
                let mut idx = indices.clone();
                idx.push(j as u32);
                self.retain_walk(
                    fb_decls,
                    &format!("{path}.{}", f.name),
                    symbol,
                    base,
                    idx,
                    f,
                    flags.get(j).copied().unwrap_or(false),
                    out,
                    depth + 1,
                );
            }
        }
    }

    /// `sizeof(T)` as the constant `end - p`, where `end` is one past a real object
    /// `p`. The usual `ptrtoint (gep T, null, 1)` idiom needs a non-inbounds GEP on
    /// a null pointer; this one stays within (one past) a real object, so every GEP
    /// in the module is `inbounds`. It folds to a plain integer at code generation.
    fn const_sizeof_at(&self, ty: BasicTypeEnum<'ctx>, p: PointerValue<'ctx>) -> IntValue<'ctx> {
        let i64t = self.context.i64_type();
        let one = self.context.i32_type().const_int(1, false);
        let end = unsafe { p.const_in_bounds_gep(ty, &[one]) };
        end.const_to_int(i64t).const_sub(p.const_to_int(i64t))
    }

    fn cstring(&self, s: &str, name: &str) -> PointerValue<'ctx> {
        let v = self.context.const_string(s.as_bytes(), true);
        let g = self.module.add_global(v.get_type(), None, name);
        g.set_initializer(&v);
        g.set_constant(true);
        g.set_linkage(Linkage::Private);
        g.set_unnamed_addr(true);
        g.as_pointer_value()
    }

    fn const_global<T: BasicType<'ctx>>(
        &self,
        ty: T,
        name: &str,
        init: &dyn BasicValue<'ctx>,
    ) -> GlobalValue<'ctx> {
        let g = self.module.add_global(ty, None, name);
        g.set_initializer(init);
        g.set_constant(true);
        g
    }

    /// Emit `plcc_tasks`, `plcc_process_image`, `plcc_retain_regions`, `plcc_app`
    /// and `plcc_get_app`.
    fn emit_tables(
        &mut self,
        programs: &[ProgramType<'ctx>],
        instances: &[InstanceRec<'ctx>],
        tasks: &[TaskRec],
        single_fns: &[Option<FunctionValue<'ctx>>],
        retain: &[RetainRec<'ctx>],
        retain_signature: u32,
    ) -> Result<(), CodegenError> {
        let ptr = self.context.ptr_type(AddressSpace::default());
        let i32t = self.context.i32_type();
        let i64t = self.context.i64_type();
        let null = ptr.const_null();

        // plcc_program_instance_t
        let inst_ty = self.context.struct_type(
            &[ptr.into(), ptr.into(), ptr.into(), ptr.into(), ptr.into(), i64t.into()],
            false,
        );
        // plcc_task_t
        let task_ty = self.context.struct_type(
            &[
                ptr.into(),
                i64t.into(),
                i32t.into(),
                i32t.into(),
                ptr.into(),
                ptr.into(),
            ],
            false,
        );
        let mut task_vals: Vec<StructValue<'ctx>> = Vec::new();
        for (ti, task) in tasks.iter().enumerate() {
            let mut rows = Vec::new();
            for &ii in &task.instances {
                let inst = &instances[ii];
                let prog = &programs[inst.program];
                let init = self.declare_state_fn(&Self::init_fn_name_for(&prog.name));
                let scan = self.declare_state_fn(&Self::scan_fn_name_for(&prog.name));
                let size =
                    self.const_sizeof_at(prog.struct_type.into(), inst.global.as_pointer_value());
                rows.push(inst_ty.const_named_struct(&[
                    self.cstring(&inst.name, &format!("plcc_str_inst_{ii}")).into(),
                    self.cstring(&prog.name, &format!("plcc_str_prog_{ii}")).into(),
                    init.as_global_value().as_pointer_value().into(),
                    scan.as_global_value().as_pointer_value().into(),
                    inst.global.as_pointer_value().into(),
                    size.into(),
                ]));
            }
            let arr_ty = inst_ty.array_type(rows.len() as u32);
            let progs = self.const_global(
                arr_ty,
                &format!("plcc_task_{ti}_programs"),
                &inst_ty.const_array(&rows),
            );
            let single = match single_fns[ti] {
                Some(f) => f.as_global_value().as_pointer_value(),
                None => null,
            };
            task_vals.push(task_ty.const_named_struct(&[
                self.cstring(&task.name, &format!("plcc_str_task_{ti}")).into(),
                i64t.const_int(task.interval_ns as u64, true).into(),
                i32t.const_int(task.priority as u64, false).into(),
                i32t.const_int(task.instances.len() as u64, false).into(),
                single.into(),
                progs.as_pointer_value().into(),
            ]));
        }
        let tasks_g = self.const_global(
            task_ty.array_type(task_vals.len() as u32),
            "plcc_tasks",
            &task_ty.const_array(&task_vals),
        );

        // plcc_process_image_t — the same layout as plcc_hal::ProcessImageLayout.
        let img_ty = self.context.struct_type(
            &[
                ptr.into(),
                i32t.into(),
                ptr.into(),
                i32t.into(),
                ptr.into(),
                i32t.into(),
            ],
            false,
        );
        let mut img_fields: Vec<BasicValueEnum<'ctx>> = Vec::new();
        for area in Area::ALL {
            let g = self
                .module
                .get_global(area.symbol())
                .ok_or_else(|| llvm("process image not finalized"))?;
            img_fields.push(g.as_pointer_value().into());
            img_fields.push(i32t.const_int(self.rt.sizes[area.index()] as u64, false).into());
        }
        let image_g = self.const_global(
            img_ty,
            "plcc_process_image",
            &img_ty.const_named_struct(&img_fields),
        );

        // plcc_retain_region_t
        let ret_ty = self
            .context
            .struct_type(&[ptr.into(), ptr.into(), i64t.into()], false);
        let mut ret_vals = Vec::new();
        for (ri, r) in retain.iter().enumerate() {
            let base = self
                .module
                .get_global(&r.symbol)
                .ok_or_else(|| llvm(format!("retain base `{}` missing", r.symbol)))?;
            let idx: Vec<_> = r
                .indices
                .iter()
                .map(|i| i32t.const_int(*i as u64, false))
                .collect();
            let data = unsafe { base.as_pointer_value().const_in_bounds_gep(r.base, &idx) };
            let size = self.const_sizeof_at(self.iec_to_llvm_type(&r.ty), data);
            ret_vals.push(ret_ty.const_named_struct(&[
                self.cstring(&r.path, &format!("plcc_str_retain_{ri}")).into(),
                data.into(),
                size.into(),
            ]));
        }
        let retain_g = self.const_global(
            ret_ty.array_type(ret_vals.len() as u32),
            "plcc_retain_regions",
            &ret_ty.const_array(&ret_vals),
        );

        // plcc_app_t
        let app_ty = self.context.struct_type(
            &[
                i32t.into(),
                i32t.into(),
                ptr.into(),
                ptr.into(),
                ptr.into(),
                ptr.into(),
                ptr.into(),
                i32t.into(),
                i32t.into(),
            ],
            false,
        );
        let init_fn = self
            .module
            .get_function("plcc_init")
            .ok_or_else(|| llvm("plcc_init missing"))?;
        let run_fn = self
            .module
            .get_function("plcc_run_task")
            .ok_or_else(|| llvm("plcc_run_task missing"))?;
        let app = self.const_global(
            app_ty,
            "plcc_app",
            &app_ty.const_named_struct(&[
                i32t.const_int(ABI_VERSION as u64, false).into(),
                i32t.const_int(tasks.len() as u64, false).into(),
                tasks_g.as_pointer_value().into(),
                image_g.as_pointer_value().into(),
                init_fn.as_global_value().as_pointer_value().into(),
                run_fn.as_global_value().as_pointer_value().into(),
                retain_g.as_pointer_value().into(),
                i32t.const_int(retain.len() as u64, false).into(),
                i32t.const_int(retain_signature as u64, false).into(),
            ]),
        );

        // `plcc_get_app()`: the same descriptor through a function, for loaders that
        // resolve functions more easily than data (JIT engines, some dlopen shims).
        let get = self
            .module
            .add_function("plcc_get_app", ptr.fn_type(&[], false), None);
        let entry = self.context.append_basic_block(get, "entry");
        self.builder.position_at_end(entry);
        self.builder
            .build_return(Some(&app.as_pointer_value()))
            .map_err(llvm)?;
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Target-specific description for the C header and the symbol table.
// ─────────────────────────────────────────────────────────────────────────────

/// How one field is spelled in C.
#[derive(Clone, Debug, PartialEq)]
pub enum CType {
    /// `uint8_t`, `int16_t`, `float`, `void *`, ...
    Scalar(String),
    /// `elem name[len]`
    Array(Box<CType>, u64),
    /// A typedef emitted in the header: `plcc_fb_ton_t`.
    Named(String),
    /// Anything without a faithful C spelling: `_Alignas(align) uint8_t name[size]`.
    Opaque { size: u64, align: u64 },
}

/// One field of a struct in the header / symbol table.
#[derive(Clone, Debug)]
pub struct FieldInfo {
    pub name: String,
    pub iec_type: String,
    pub offset: u64,
    pub size: u64,
    pub c_type: CType,
    pub retain: bool,
    /// `%IX0.0` when the variable lives in the process image (its slot here is
    /// then unused).
    pub at: Option<String>,
}

/// A struct typedef in the header.
#[derive(Clone, Debug)]
pub struct StructInfo {
    /// C typedef name, e.g. `plcc_prog_main_t`, `plcc_fb_ton_t`.
    pub c_name: String,
    /// IEC name of the POU or TYPE.
    pub iec_name: String,
    pub size: u64,
    pub align: u64,
    pub fields: Vec<FieldInfo>,
}

#[derive(Clone, Debug)]
pub struct AtInfo {
    /// `Main`, `MotorFb`, or `GLOBAL`.
    pub scope: String,
    pub name: String,
    pub area: char,
    pub size_prefix: char,
    pub byte_offset: u32,
    pub bit: Option<u8>,
    pub address: String,
    pub iec_type: String,
    pub size: u64,
}

#[derive(Clone, Debug)]
pub struct ProgramInfo {
    pub name: String,
    /// Index into [`RuntimeContract::types`].
    pub type_index: usize,
    pub init_fn: String,
    pub scan_fn: String,
}

#[derive(Clone, Debug)]
pub struct InstanceInfo {
    pub name: String,
    pub program: String,
    pub symbol: String,
    pub task: usize,
}

#[derive(Clone, Debug)]
pub struct TaskInfo {
    pub name: String,
    pub interval_ns: i64,
    pub priority: u32,
    pub has_single: bool,
    pub instances: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct RetainInfo {
    pub path: String,
    pub symbol: String,
    pub offset: u64,
    pub size: u64,
    pub iec_type: String,
}

/// A flattened, addressable variable (for HMI / Modbus mapping).
#[derive(Clone, Debug)]
pub struct VariableInfo {
    pub path: String,
    /// Symbol whose address `offset` is relative to (an instance, `plcc_globals`,
    /// or an image area for AT variables).
    pub symbol: String,
    pub offset: u64,
    pub size: u64,
    pub iec_type: String,
    pub retain: bool,
    pub bit: Option<u8>,
}

/// The complete, target-specific runtime contract of a compiled module.
#[derive(Clone, Debug)]
pub struct RuntimeContract {
    pub abi_version: u32,
    pub triple: String,
    /// LLVM CPU (`generic` unless chosen).
    pub cpu: String,
    /// LLVM target features, comma-separated (may be empty).
    pub features: String,
    /// The device manifest the program was built for (`plcc compile --device`);
    /// the caller fills it in.
    pub device: Option<DeviceStamp>,
    pub pointer_size: u32,
    pub big_endian: bool,
    /// Sizes of the `%I`, `%Q`, `%M` areas in bytes.
    pub image_sizes: [u32; 3],
    pub at_bindings: Vec<AtInfo>,
    /// Typedefs in dependency order (FBs and STRUCTs first, then programs).
    pub types: Vec<StructInfo>,
    pub programs: Vec<ProgramInfo>,
    pub instances: Vec<InstanceInfo>,
    pub tasks: Vec<TaskInfo>,
    pub retain: Vec<RetainInfo>,
    pub retain_signature: u32,
    /// `plcc_globals`, when the program has VAR_GLOBALs.
    pub globals: Option<StructInfo>,
    pub variables: Vec<VariableInfo>,
}

/// Which device manifest a build used: its id and version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceStamp {
    pub id: String,
    pub version: u32,
}

struct TypeCtx<'a, 'ctx> {
    td: &'a TargetData,
    types: Vec<StructInfo>,
    by_key: HashMap<String, usize>,
    anon: usize,
    _p: std::marker::PhantomData<&'ctx ()>,
}

impl<'ctx> Compiler<'ctx> {
    fn c_scalar(&self, ty: &IecType, llvm_ty: BasicTypeEnum<'ctx>) -> Option<String> {
        let signed = ty.is_any_signed()
            || matches!(
                ty,
                IecType::Time
                    | IecType::Ltime
                    | IecType::Date
                    | IecType::Tod
                    | IecType::Dt
                    | IecType::Ldate
                    | IecType::Ltod
                    | IecType::Ldt
            );
        Some(match llvm_ty {
            BasicTypeEnum::IntType(t) => {
                let w = t.get_bit_width();
                if !matches!(w, 8 | 16 | 32 | 64) {
                    return None;
                }
                if matches!(ty, IecType::Char) {
                    "char".into()
                } else {
                    format!("{}int{w}_t", if signed { "" } else { "u" })
                }
            }
            BasicTypeEnum::FloatType(t) => {
                if t == self.context.f64_type() {
                    "double".into()
                } else {
                    "float".into()
                }
            }
            BasicTypeEnum::PointerType(_) => "void *".into(),
            _ => return None,
        })
    }

    fn c_type_of(&self, cx: &mut TypeCtx<'_, 'ctx>, ty: &IecType) -> CType {
        let llvm_ty = self.iec_to_llvm_type(ty);
        let opaque = |cx: &TypeCtx| CType::Opaque {
            size: cx.td.get_abi_size(&llvm_ty.as_any_type_enum()),
            align: cx.td.get_abi_alignment(&llvm_ty.as_any_type_enum()) as u64,
        };
        match ty {
            IecType::StringType { max_len } => {
                CType::Array(Box::new(CType::Scalar("char".into())), max_len.unwrap_or(plcc_hir::types::DEFAULT_STRING_LEN) as u64 + 1)
            }
            IecType::WstringType { max_len } => CType::Array(
                Box::new(CType::Scalar("uint16_t".into())),
                max_len.unwrap_or(plcc_hir::types::DEFAULT_STRING_LEN) as u64 + 1,
            ),
            IecType::Array {
                ranges,
                element_type,
            } => {
                let n: u64 = ranges
                    .iter()
                    .map(|(lo, hi)| (hi - lo + 1).max(0) as u64)
                    .product();
                CType::Array(Box::new(self.c_type_of(cx, element_type)), n)
            }
            IecType::Struct { name, fields } => {
                let key = format!("S:{llvm_ty}");
                let _ = name;
                if let Some(&i) = cx.by_key.get(&key) {
                    return CType::Named(cx.types[i].c_name.clone());
                }
                let BasicTypeEnum::StructType(st) = llvm_ty else {
                    return opaque(cx);
                };
                let pf: Vec<PouField> = fields
                    .iter()
                    .map(|(n, t)| PouField::plain(n.clone(), t.clone()))
                    .collect();
                // A TYPE's resolved STRUCT does not carry its name; recover it from
                // the declared TYPEs so the typedef reads `plcc_struct_recipe_t`.
                let name = if name.is_empty() {
                    let mut names: Vec<&String> = self.type_specs.keys().collect();
                    names.sort();
                    names
                        .into_iter()
                        .find(|k| self.type_checker.types.resolve(k).as_ref() == Some(ty))
                        .cloned()
                        .unwrap_or_default()
                } else {
                    name.clone()
                };
                let c_name = if name.is_empty() {
                    cx.anon += 1;
                    format!("plcc_struct_anon{}_t", cx.anon)
                } else {
                    format!("plcc_struct_{}_t", c_ident(&name))
                };
                let iec_name = if name.is_empty() { "STRUCT".into() } else { name.clone() };
                let idx = self.struct_info(cx, &c_name, &iec_name, st, &pf, &[]);
                cx.by_key.insert(key, idx);
                CType::Named(c_name)
            }
            IecType::FbInstance(fb) => {
                let key = format!("FB:{}", fb.to_uppercase());
                if let Some(&i) = cx.by_key.get(&key) {
                    return CType::Named(cx.types[i].c_name.clone());
                }
                let Some(layout) = self.compiled_fbs.get(&fb.to_uppercase()).cloned() else {
                    return opaque(cx);
                };
                let c_name = format!("plcc_fb_{}_t", c_ident(fb));
                let idx = self.struct_info(cx, &c_name, fb, layout.struct_type, &layout.fields, &[]);
                cx.by_key.insert(key, idx);
                CType::Named(c_name)
            }
            IecType::Enum { base_type, .. }
            | IecType::Subrange { base_type, .. }
            | IecType::Alias { base_type, .. } => {
                match self.c_scalar(base_type, llvm_ty) {
                    Some(s) => CType::Scalar(s),
                    None => opaque(cx),
                }
            }
            _ => match self.c_scalar(ty, llvm_ty) {
                Some(s) => CType::Scalar(s),
                None => opaque(cx),
            },
        }
    }

    fn struct_info(
        &self,
        cx: &mut TypeCtx<'_, 'ctx>,
        c_name: &str,
        iec_name: &str,
        st: StructType<'ctx>,
        fields: &[PouField],
        retain: &[bool],
    ) -> usize {
        let mut out = Vec::new();
        for (i, f) in fields.iter().enumerate() {
            let fty = st
                .get_field_type_at_index(i as u32)
                .unwrap_or(self.context.i8_type().into());
            let c_type = if f.is_in_out {
                CType::Scalar("void *".into())
            } else {
                self.c_type_of(cx, &f.stored)
            };
            out.push(FieldInfo {
                name: f.name.clone(),
                iec_type: f.declared.to_string(),
                offset: cx.td.offset_of_element(&st, i as u32).unwrap_or(0),
                size: cx.td.get_abi_size(&fty.as_any_type_enum()),
                c_type,
                retain: retain.get(i).copied().unwrap_or(false),
                at: f.at.map(|a| a.to_string()),
            });
        }
        cx.types.push(StructInfo {
            c_name: c_name.to_string(),
            iec_name: iec_name.to_string(),
            size: cx.td.get_abi_size(&st.as_any_type_enum()),
            align: cx.td.get_abi_alignment(&st.as_any_type_enum()) as u64,
            fields: out,
        });
        cx.types.len() - 1
    }

    /// Flatten `ty` at `offset` into addressable leaves.
    fn flatten(
        &self,
        cx: &TypeCtx<'_, 'ctx>,
        path: &str,
        symbol: &str,
        offset: u64,
        ty: &IecType,
        retain: bool,
        out: &mut Vec<VariableInfo>,
    ) {
        let llvm_ty = self.iec_to_llvm_type(ty);
        let fields: Option<(StructType<'ctx>, Vec<(String, IecType)>)> = match ty {
            IecType::Struct { fields, .. } => match llvm_ty {
                BasicTypeEnum::StructType(st) => Some((st, fields.clone())),
                _ => None,
            },
            IecType::FbInstance(fb) => self.compiled_fbs.get(&fb.to_uppercase()).map(|l| {
                (
                    l.struct_type,
                    l.fields
                        .iter()
                        .map(|f| (f.name.clone(), f.stored.clone()))
                        .collect(),
                )
            }),
            _ => None,
        };
        match fields {
            Some((st, fs)) => {
                for (i, (n, t)) in fs.iter().enumerate() {
                    let off = cx.td.offset_of_element(&st, i as u32).unwrap_or(0);
                    self.flatten(cx, &format!("{path}.{n}"), symbol, offset + off, t, retain, out);
                }
            }
            None => out.push(VariableInfo {
                path: path.to_string(),
                symbol: symbol.to_string(),
                offset,
                size: cx.td.get_abi_size(&llvm_ty.as_any_type_enum()),
                iec_type: ty.to_string(),
                retain,
                bit: None,
            }),
        }
    }

    /// Describe the compiled module's runtime contract for `triple`: image sizes,
    /// AT bindings, struct layouts as LLVM lays them out for that target, tasks,
    /// instances and retain regions. Valid after a successful `compile`.
    pub fn runtime_contract(&self, triple: &str) -> Result<RuntimeContract, CodegenError> {
        let layout = self
            .rt
            .layout
            .as_ref()
            .ok_or_else(|| CodegenError::TargetError("runtime_contract before compile".into()))?;
        let machine = self.target_machine(triple)?;
        let td = machine.get_target_data();
        let mut cx = TypeCtx {
            td: &td,
            types: Vec::new(),
            by_key: HashMap::new(),
            anon: 0,
            _p: std::marker::PhantomData,
        };

        let mut programs = Vec::new();
        for p in &layout.programs {
            let c_name = format!("plcc_prog_{}_t", c_ident(&p.name));
            let idx = self.struct_info(&mut cx, &c_name, &p.name, p.struct_type, &p.fields, &p.retain);
            programs.push(ProgramInfo {
                name: p.name.clone(),
                type_index: idx,
                init_fn: Self::init_fn_name_for(&p.name),
                scan_fn: Self::scan_fn_name_for(&p.name),
            });
        }
        let globals = layout.globals.as_ref().map(|(st, desc)| {
            let pf: Vec<PouField> = desc
                .iter()
                .map(|(n, t, _, _)| {
                    let mut f = PouField::plain(n.clone(), t.clone());
                    f.at = self
                        .rt
                        .global_at
                        .iter()
                        .find(|g| g.0 == n.to_uppercase())
                        .map(|g| g.1);
                    f
                })
                .collect();
            let retain: Vec<bool> = desc.iter().map(|d| d.2).collect();
            let idx = self.struct_info(&mut cx, "plcc_globals_t", "VAR_GLOBAL", *st, &pf, &retain);
            cx.types.remove(idx)
        });

        let retain = layout
            .retain
            .iter()
            .map(|r| {
                // Offset of a constant GEP path through nested structs.
                let mut off = 0u64;
                let mut cur: BasicTypeEnum<'ctx> = r.base.into();
                for &i in &r.indices[1..] {
                    if let BasicTypeEnum::StructType(st) = cur {
                        off += td.offset_of_element(&st, i).unwrap_or(0);
                        cur = st.get_field_type_at_index(i).unwrap_or(cur);
                    }
                }
                RetainInfo {
                    path: r.path.clone(),
                    symbol: r.symbol.clone(),
                    offset: off,
                    size: td.get_abi_size(&self.iec_to_llvm_type(&r.ty).as_any_type_enum()),
                    iec_type: r.ty.to_string(),
                }
            })
            .collect();

        let at_bindings: Vec<AtInfo> = self
            .rt
            .bindings
            .iter()
            .map(|b: &AtBinding| AtInfo {
                scope: b.scope.clone(),
                name: b.name.clone(),
                area: b.addr.area.letter(),
                size_prefix: b.addr.size.letter(),
                byte_offset: b.addr.byte,
                bit: b.addr.bit,
                address: b.addr.to_string(),
                iec_type: b.ty.to_string(),
                size: b.size,
            })
            .collect();

        // Flattened variables: every instance's non-AT leaves, VAR_GLOBAL leaves,
        // and every AT binding at its image location.
        let mut variables = Vec::new();
        for inst in &layout.instances {
            let p = &layout.programs[inst.program];
            for (i, f) in p.fields.iter().enumerate() {
                if f.at.is_some() || f.is_in_out {
                    continue;
                }
                let off = td.offset_of_element(&p.struct_type, i as u32).unwrap_or(0);
                self.flatten(
                    &cx,
                    &format!("{}.{}", inst.name, f.name),
                    &inst.symbol,
                    off,
                    &f.declared,
                    p.retain[i],
                    &mut variables,
                );
            }
        }
        if let Some((st, desc)) = &layout.globals {
            for (i, (n, t, r, at)) in desc.iter().enumerate() {
                if at.is_some() {
                    continue;
                }
                let off = td.offset_of_element(st, i as u32).unwrap_or(0);
                self.flatten(&cx, &format!("GLOBAL.{n}"), "plcc_globals", off, t, *r, &mut variables);
            }
        }
        for b in &self.rt.bindings {
            variables.push(VariableInfo {
                path: format!("{}.{}", b.scope, b.name),
                symbol: b.addr.area.symbol().to_string(),
                offset: b.addr.byte as u64,
                size: b.size,
                iec_type: b.ty.to_string(),
                retain: false,
                bit: b.addr.bit,
            });
        }

        Ok(RuntimeContract {
            abi_version: ABI_VERSION,
            triple: triple.to_string(),
            cpu: self.machine.cpu.clone(),
            features: self.machine.features.clone(),
            device: None,
            pointer_size: td.get_pointer_byte_size(None),
            big_endian: matches!(td.get_byte_ordering(), ByteOrdering::BigEndian),
            image_sizes: self.rt.sizes,
            at_bindings,
            types: cx.types,
            programs,
            instances: layout
                .instances
                .iter()
                .map(|i| InstanceInfo {
                    name: i.name.clone(),
                    program: layout.programs[i.program].name.clone(),
                    symbol: i.symbol.clone(),
                    task: i.task,
                })
                .collect(),
            tasks: layout
                .tasks
                .iter()
                .map(|t| TaskInfo {
                    name: t.name.clone(),
                    interval_ns: t.interval_ns,
                    priority: t.priority,
                    has_single: t.single.is_some(),
                    instances: t.instances.clone(),
                })
                .collect(),
            retain,
            retain_signature: layout.retain_signature,
            globals,
            variables,
        })
    }
}
