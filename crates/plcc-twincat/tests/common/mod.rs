// SPDX-License-Identifier: MPL-2.0

//! Shared harness: load a TwinCAT project (its .plcproj, every object file and
//! its task configuration, as the CLI does), compile it with the bundled
//! stdlib, JIT it, and drive it through the runtime contract (`plcc_get_app`),
//! reading and writing variables by name through the contract's symbol table.

#![allow(dead_code)]

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;
use plcc_codegen::compiler::contract::RuntimeContract;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, MutexGuard};

#[repr(C)]
pub struct Image {
    input: *mut u8,
    input_size: u32,
    output: *mut u8,
    output_size: u32,
    memory: *mut u8,
    memory_size: u32,
}

#[repr(C)]
pub struct Instance {
    name: *const c_char,
    program_type: *const c_char,
    init: extern "C" fn(*mut u8),
    scan: extern "C" fn(*mut u8),
    state: *mut u8,
    state_size: u64,
}

#[repr(C)]
pub struct Task {
    pub name: *const c_char,
    pub interval_ns: i64,
    pub priority: u32,
    program_count: u32,
    single: Option<extern "C" fn() -> u8>,
    programs: *const Instance,
}

#[repr(C)]
pub struct App {
    abi_version: u32,
    task_count: u32,
    tasks: *const Task,
    image: *const Image,
    init: extern "C" fn(),
    run_task: extern "C" fn(u32),
    retain: *const u8,
    retain_count: u32,
    retain_signature: u32,
}

static FAKE_NOW_NS: AtomicI64 = AtomicI64::new(0);
static CLOCK_LOCK: Mutex<()> = Mutex::new(());

extern "C" fn fake_monotonic_ns() -> i64 {
    FAKE_NOW_NS.load(Ordering::SeqCst)
}

pub const MS: i64 = 1_000_000;

pub fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/twincat")
        .join(name)
}

/// Load a `.plcproj` (relative to the TwinCAT fixtures) the way the CLI does:
/// every object file it compiles, then a CONFIGURATION from its tasks. Panics
/// with rendered diagnostics on any parse error.
pub fn load_project(project: &str) -> plcc_st::CompilationUnit {
    let path = fixture(project);
    let src = std::fs::read_to_string(&path).expect("project exists");
    let files = plcc_twincat::project_files(&path, &src).expect("project files");
    let mut decls = Vec::new();
    let mut tasks = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(&f).expect("object file");
        if plcc_twincat::is_task_file(&f) {
            tasks.extend(plcc_twincat::parse_task(&text).expect("task"));
            continue;
        }
        let (unit, errors) = plcc_twincat::parse(&text);
        let errors: Vec<_> = errors.into_iter().filter(|e| !e.is_warning()).collect();
        if !errors.is_empty() {
            let mut msg = String::new();
            for e in errors {
                let r = miette::Report::new(e).with_source_code(miette::NamedSource::new(
                    f.display().to_string(),
                    text.clone(),
                ));
                msg.push_str(&format!("{r:?}\n"));
            }
            panic!("{} failed to parse:\n{msg}", f.display());
        }
        decls.extend(unit.declarations);
    }
    if let Some(cfg) = plcc_twincat::configuration_source(&tasks) {
        let (unit, errors) = plcc_st::parse(&cfg);
        assert!(errors.is_empty(), "{errors:?}");
        decls.extend(unit.declarations);
    }
    plcc_st::CompilationUnit {
        declarations: decls,
        span: plcc_st::Span::new(0, 0),
    }
}

/// Prepend the bundled stdlib, like the CLI does.
pub fn with_stdlib(unit: plcc_st::CompilationUnit) -> plcc_st::CompilationUnit {
    let mut decls = Vec::new();
    for u in plcc_stdlib::UNITS {
        let (su, errs) = plcc_st::parse(u.source);
        assert!(errs.is_empty());
        decls.extend(su.declarations);
    }
    decls.extend(unit.declarations);
    plcc_st::CompilationUnit {
        declarations: decls,
        span: unit.span,
    }
}

/// Type-check a project (with the stdlib, as `plcc check` does); return the
/// error messages (warnings left out).
pub fn check_errors(project: &str) -> Vec<String> {
    let unit = with_stdlib(load_project(project));
    let (_, errors) = plcc_hir::check(&unit);
    errors
        .iter()
        .filter(|e| !e.is_warning())
        .map(|e| e.to_string())
        .collect()
}

pub struct Plc<'a> {
    app: &'a App,
    contract: RuntimeContract,
    /// Address of `plcc_globals` (VAR_GLOBALs), when there are any.
    globals: Option<*mut u8>,
}

impl Plc<'_> {
    fn image(&self) -> &Image {
        unsafe { &*self.app.image }
    }

    pub fn tasks(&self) -> &[Task] {
        unsafe { std::slice::from_raw_parts(self.app.tasks, self.app.task_count as usize) }
    }

    pub fn task_name(&self, i: usize) -> String {
        unsafe { CStr::from_ptr(self.tasks()[i].name) }
            .to_string_lossy()
            .into_owned()
    }

    fn instance_state(&self, symbol: &str) -> *mut u8 {
        let inst = self
            .contract
            .instances
            .iter()
            .find(|i| i.symbol == symbol)
            .unwrap_or_else(|| panic!("no instance {symbol}"));
        for t in self.tasks() {
            let progs = unsafe { std::slice::from_raw_parts(t.programs, t.program_count as usize) };
            for p in progs {
                let n = unsafe { CStr::from_ptr(p.name) }.to_string_lossy();
                if n == inst.name {
                    return p.state;
                }
            }
        }
        panic!("instance {} not in the task table", inst.name)
    }

    fn locate(&self, path: &str) -> (*mut u8, Option<u8>, u64) {
        let want = path.to_ascii_uppercase();
        let v = self
            .contract
            .variables
            .iter()
            .find(|v| v.path.to_ascii_uppercase() == want)
            .or_else(|| {
                let suffix = format!(".{want}");
                self.contract
                    .variables
                    .iter()
                    .find(|v| v.path.to_ascii_uppercase().ends_with(&suffix))
            })
            .unwrap_or_else(|| {
                let all: Vec<_> = self.contract.variables.iter().map(|v| &v.path).collect();
                panic!("no variable {path}; have {all:?}")
            });
        let img = self.image();
        let base = match v.symbol.as_str() {
            "plcc_image_i" => img.input,
            "plcc_image_q" => img.output,
            "plcc_image_m" => img.memory,
            "plcc_globals" => self.globals.expect("plcc_globals"),
            s => self.instance_state(s),
        };
        (unsafe { base.add(v.offset as usize) }, v.bit, v.size)
    }

    pub fn get(&self, path: &str) -> i64 {
        let (p, bit, size) = self.locate(path);
        unsafe {
            if let Some(b) = bit {
                return ((*p >> b) & 1) as i64;
            }
            match size {
                1 => *p as i8 as i64,
                2 => (p as *const i16).read_unaligned() as i64,
                4 => (p as *const i32).read_unaligned() as i64,
                8 => (p as *const i64).read_unaligned(),
                n => panic!("{path}: unsupported size {n}"),
            }
        }
    }

    pub fn get_bool(&self, path: &str) -> bool {
        self.get(path) != 0
    }

    pub fn set(&self, path: &str, v: i64) {
        let (p, bit, size) = self.locate(path);
        unsafe {
            if let Some(b) = bit {
                if v != 0 {
                    *p |= 1 << b;
                } else {
                    *p &= !(1 << b);
                }
                return;
            }
            match size {
                1 => *p = v as u8,
                2 => (p as *mut i16).write_unaligned(v as i16),
                4 => (p as *mut i32).write_unaligned(v as i32),
                8 => (p as *mut i64).write_unaligned(v),
                n => panic!("{path}: unsupported size {n}"),
            }
        }
    }

    pub fn set_bool(&self, path: &str, v: bool) {
        self.set(path, v as i64);
    }

    pub fn input_byte(&self, i: usize) -> &mut u8 {
        unsafe { &mut *self.image().input.add(i) }
    }

    pub fn output_byte(&self, i: usize) -> u8 {
        unsafe { *self.image().output.add(i) }
    }

    /// Run task 0 once.
    pub fn scan(&self) {
        (self.app.run_task)(0)
    }

    /// Advance the fake clock.
    pub fn advance(&self, ns: i64) {
        FAKE_NOW_NS.fetch_add(ns, Ordering::SeqCst);
    }
}

/// Compile and JIT a project, then hand the running PLC to `f`. The fake
/// clock starts at 1 s and is serialized across tests.
pub fn with_plc<R>(project: &str, f: impl FnOnce(&Plc) -> R) -> R {
    let _clock: MutexGuard<'_, ()> = CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    FAKE_NOW_NS.store(1_000 * MS, Ordering::SeqCst);
    let unit = with_stdlib(load_project(project));
    let ctx = Context::create();
    let mut compiler = Compiler::new(&ctx, project);
    if let Err(e) = compiler.compile(&unit) {
        panic!("{project}: codegen failed: {e}");
    }
    if let Err(e) = compiler.module().verify() {
        panic!("{project}: invalid IR: {e}");
    }
    let contract = compiler
        .runtime_contract("x86_64-unknown-linux-gnu")
        .expect("contract");
    let clock = compiler.module().get_function("plcc_monotonic_ns");
    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("JIT");
    if let Some(decl) = clock {
        ee.add_global_mapping(&decl, fake_monotonic_ns as *const () as usize);
    }
    let get = ee
        .get_function_address("plcc_get_app")
        .expect("plcc_get_app");
    let get: extern "C" fn() -> *const App = unsafe { std::mem::transmute(get) };
    let app = unsafe { &*get() };
    assert_eq!(app.abi_version, 1);
    (app.init)();
    // MCJIT resolves any symbol by name, data included.
    let globals = ee
        .get_function_address("plcc_globals")
        .ok()
        .map(|a| a as *mut u8);
    f(&Plc {
        app,
        contract,
        globals,
    })
}
