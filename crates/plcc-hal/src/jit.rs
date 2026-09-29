// SPDX-License-Identifier: MPL-2.0
//! Compile Structured Text in-process and load it as an [`Application`].
//!
//! This is the simulator path: the same module a target build would link, JIT-
//! compiled for the host, reached only through its `plcc_app` descriptor. The
//! runtime symbols it imports (`plcc_monotonic_ns`, `plcc_print`) are mapped onto
//! host implementations, and `plcc_fault` onto
//! [`unwinding_fault_handler`](crate::fault::unwinding_fault_handler), so a
//! runtime fault reaches [`ScanCycle`](crate::scan::ScanCycle) as an error.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::execution_engine::ExecutionEngine;
use plcc_codegen::Compiler;
use plcc_codegen::direct_address::Area;
use plcc_st::ast::{CompilationUnit, Declaration};

use crate::app::{Application, RawApp};

/// Options for [`JitModule::compile`].
#[derive(Clone, Debug)]
pub struct JitOptions {
    /// Compile the bundled IEC standard function blocks (TON, CTU, R_TRIG, ...)
    /// alongside the program. A POU of the same name in the program wins.
    pub stdlib: bool,
    /// Fixed process-image sizes (`--image-size`).
    pub image_sizes: Vec<(Area, u32)>,
    /// Interval of the implicit task when there is no CONFIGURATION.
    pub default_task_interval_ns: Option<i64>,
}

impl Default for JitOptions {
    fn default() -> Self {
        Self {
            stdlib: true,
            image_sizes: Vec::new(),
            default_task_interval_ns: None,
        }
    }
}

/// A JIT-compiled module. Keep it alive while its [`Application`] is in use.
pub struct JitModule {
    _compiler: Compiler<'static>,
    ee: ExecutionEngine<'static>,
}

extern "C" fn host_print(msg: *const core::ffi::c_char) {
    if !msg.is_null() {
        let s = unsafe { core::ffi::CStr::from_ptr(msg) };
        eprintln!("[PLC] {}", s.to_string_lossy());
    }
}

fn name_of(d: &Declaration) -> Option<String> {
    Some(
        match d {
            Declaration::Program(p) => &p.name.name,
            Declaration::Function(f) => &f.name.name,
            Declaration::FunctionBlock(f) => &f.name.name,
            Declaration::Class(c) => &c.name.name,
            Declaration::Interface(i) => &i.name.name,
            Declaration::TypeDecl(t) => &t.name.name,
            _ => return None,
        }
        .to_uppercase(),
    )
}

impl JitModule {
    /// Parse `sources` (name, text), compile them as one module and JIT it.
    ///
    /// The compiler context is leaked: a JIT module lives for the rest of the
    /// process, which is what a simulator wants.
    pub fn compile(sources: &[(&str, &str)], opts: &JitOptions) -> Result<Self, String> {
        let mut decls = Vec::new();
        let mut files = Vec::new();
        for (name, text) in sources {
            let (unit, errors) = plcc_st::parse(text);
            if let Some(e) = errors.first() {
                return Err(format!("{name}: {e}"));
            }
            let pous: Vec<String> = unit.declarations.iter().filter_map(name_of).collect();
            files.push((*name, *text, pous));
            decls.extend(unit.declarations);
        }
        if opts.stdlib {
            let user: std::collections::HashSet<String> =
                decls.iter().filter_map(name_of).collect();
            let mut prelude = Vec::new();
            for u in plcc_stdlib::UNITS {
                let (unit, errors) = plcc_st::parse(u.source);
                if let Some(e) = errors.first() {
                    return Err(format!("bundled stdlib {}: {e}", u.name));
                }
                prelude.extend(
                    unit.declarations
                        .into_iter()
                        .filter(|d| name_of(d).is_none_or(|n| !user.contains(&n))),
                );
            }
            prelude.extend(decls);
            decls = prelude;
        }
        let unit = CompilationUnit {
            declarations: decls,
            span: plcc_st::span::Span::empty(),
        };

        let ctx: &'static Context = Box::leak(Box::new(Context::create()));
        let mut compiler = Compiler::new(ctx, "plcc_jit");
        for (area, n) in &opts.image_sizes {
            compiler.set_image_size(*area, *n);
        }
        if let Some(ns) = opts.default_task_interval_ns {
            compiler.set_task_options(plcc_codegen::TaskOptions {
                default_interval_ns: ns,
            });
        }
        for (name, text, pous) in &files {
            compiler.add_source_file(name, text, pous);
        }
        compiler.compile(&unit).map_err(|e| e.to_string())?;
        // Runtime faults unwind back to `Application::try_run_task`.
        compiler.use_external_fault_handler();
        let ee = compiler
            .module()
            .create_jit_execution_engine(OptimizationLevel::None)
            .map_err(|e| e.to_string())?;
        if let Some(f) = compiler.module().get_function("plcc_monotonic_ns") {
            ee.add_global_mapping(
                &f,
                plcc_runtime::host_clock::plcc_monotonic_ns as *const () as usize,
            );
        }
        if let Some(f) = compiler.module().get_function("plcc_print") {
            ee.add_global_mapping(&f, host_print as *const () as usize);
        }
        if let Some(f) = compiler.module().get_function("plcc_fault") {
            ee.add_global_mapping(
                &f,
                crate::fault::unwinding_fault_handler as *const () as usize,
            );
        }
        Ok(Self {
            _compiler: compiler,
            ee,
        })
    }

    /// The module's application descriptor.
    pub fn application(&self) -> Result<Application, String> {
        let addr = self
            .ee
            .get_function_address("plcc_get_app")
            .map_err(|e| e.to_string())?;
        let get: extern "C" fn() -> *const RawApp = unsafe { core::mem::transmute(addr) };
        // Safety: `self` keeps the JIT-compiled code and data alive; callers keep
        // `self` alive while the Application is in use (see the type docs).
        unsafe { Application::from_get_app(get) }.map_err(|e| e.to_string())
    }

    /// The module's runtime contract for the host (variable offsets, AT bindings,
    /// tasks) — what `plcc compile --emit-symbols` writes.
    pub fn contract(&self) -> Result<plcc_codegen::RuntimeContract, String> {
        let triple = inkwell::targets::TargetMachine::get_default_triple();
        self._compiler
            .runtime_contract(&triple.as_str().to_string_lossy())
            .map_err(|e| e.to_string())
    }

    /// Address of any exported function (e.g. a legacy `<program>_scan`).
    pub fn function_address(&self, name: &str) -> Option<usize> {
        self.ee.get_function_address(name).ok()
    }
}
