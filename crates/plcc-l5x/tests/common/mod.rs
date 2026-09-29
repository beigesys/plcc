// SPDX-License-Identifier: MPL-2.0

//! Shared harness: load an L5X fixture, lower it, compile it with the Logix
//! prelude and the bundled stdlib, JIT it, and drive it through the runtime contract
//! (`plcc_get_app`) exactly like a runtime would — reading and writing variables
//! by name through the contract's symbol table.

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
        .join("../../tests/fixtures/l5x")
        .join(name)
}

pub fn read_fixture(name: &str) -> String {
    std::fs::read_to_string(fixture(name)).expect("fixture exists")
}

fn render(name: &str, src: &str, errors: Vec<plcc_l5x::L5xError>) -> String {
    let mut msg = String::new();
    for e in errors {
        let r = miette::Report::new(e)
            .with_source_code(miette::NamedSource::new(name, src.to_string()));
        msg.push_str(&format!("{r:?}\n"));
    }
    msg
}

/// Lower L5X source; panic with rendered diagnostics on any error (warnings
/// are allowed).
pub fn lower_src(name: &str, src: &str, opts: &plcc_l5x::Options) -> plcc_st::CompilationUnit {
    let (unit, errors) = plcc_l5x::parse_with(src, opts);
    if errors.iter().any(|e| !e.is_warning()) {
        let (st, _) = plcc_l5x::to_st(src, opts);
        panic!(
            "{name} failed to lower:\n{}\n--- generated ST ---\n{st}",
            render(name, src, errors)
        );
    }
    unit
}

pub fn lower(name: &str) -> plcc_st::CompilationUnit {
    lower_src(name, &read_fixture(name), &plcc_l5x::Options::default())
}

/// Lowering diagnostics of a fixture (errors and warnings), as strings.
pub fn diagnostics(name: &str) -> Vec<String> {
    let (_, errors) = plcc_l5x::parse(&read_fixture(name));
    errors.iter().map(|e| e.to_string()).collect()
}

/// Prepend the Logix prelude and the bundled stdlib, like the CLI does.
pub fn with_preludes(unit: plcc_st::CompilationUnit) -> plcc_st::CompilationUnit {
    let mut decls = Vec::new();
    let prelude = plcc_l5x::prelude();
    let (pu, errs) = plcc_st::parse(&prelude);
    assert!(errs.is_empty(), "prelude: {errs:?}");
    let names: Vec<String> = pu
        .declarations
        .iter()
        .filter_map(|d| match d {
            plcc_st::Declaration::Function(f) => Some(f.name.name.to_ascii_uppercase()),
            _ => None,
        })
        .collect();
    decls.extend(pu.declarations);
    for u in plcc_stdlib::UNITS {
        let (su, errs) = plcc_st::parse(u.source);
        assert!(errs.is_empty());
        for d in su.declarations {
            let clash = match &d {
                plcc_st::Declaration::FunctionBlock(f) => {
                    names.contains(&f.name.name.to_ascii_uppercase())
                }
                _ => false,
            };
            if !clash {
                decls.push(d);
            }
        }
    }
    decls.extend(unit.declarations);
    plcc_st::CompilationUnit {
        declarations: decls,
        span: unit.span,
    }
}

/// Type-check a lowered fixture with the preludes; return error messages.
pub fn check_errors(name: &str) -> Vec<String> {
    let unit = with_preludes(lower(name));
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
    globals: *mut u8,
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
        // `name[i]`: an element of a one-dimensional array variable.
        if let Some(open) = path.rfind('[')
            && path.ends_with(']')
            && !path[..open].contains('[')
        {
            let i: u64 = path[open + 1..path.len() - 1]
                .trim()
                .parse()
                .expect("index");
            let base = &path[..open];
            let v = self.var(base);
            // Logix arrays are zero-based; the element size follows the type.
            let ty = v.iec_type.to_ascii_uppercase();
            let elem_ty = ty.rsplit(" OF ").next().unwrap_or("").trim().to_string();
            let elem = match elem_ty.as_str() {
                "BOOL" | "SINT" | "USINT" | "BYTE" => 1,
                "INT" | "UINT" | "WORD" => 2,
                "DINT" | "UDINT" | "REAL" | "DWORD" => 4,
                "LINT" | "ULINT" | "LREAL" | "LWORD" => 8,
                other => panic!("{base}: element type {other} ({})", v.iec_type),
            };
            let (p, _, _) = self.locate(base);
            return (unsafe { p.add((i * elem) as usize) }, None, elem);
        }
        let v = self.var(path);
        let img = self.image();
        let base = match v.symbol.as_str() {
            "plcc_image_i" => img.input,
            "plcc_image_q" => img.output,
            "plcc_image_m" => img.memory,
            "plcc_globals" => self.globals,
            s => self.instance_state(s),
        };
        (unsafe { base.add(v.offset as usize) }, v.bit, v.size)
    }

    fn var(&self, path: &str) -> &plcc_codegen::compiler::contract::VariableInfo {
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
        v
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

    /// A REAL (4 bytes) or LREAL (8 bytes) variable.
    pub fn get_real(&self, path: &str) -> f64 {
        let (p, _, size) = self.locate(path);
        unsafe {
            match size {
                4 => (p as *const f32).read_unaligned() as f64,
                8 => (p as *const f64).read_unaligned(),
                n => panic!("{path}: not a REAL ({n} bytes)"),
            }
        }
    }

    pub fn set_real(&self, path: &str, v: f64) {
        let (p, _, size) = self.locate(path);
        unsafe {
            match size {
                4 => (p as *mut f32).write_unaligned(v as f32),
                8 => (p as *mut f64).write_unaligned(v),
                n => panic!("{path}: not a REAL ({n} bytes)"),
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

    /// Run the task called `name` once.
    pub fn run_task(&self, name: &str) {
        let i = (0..self.tasks().len())
            .find(|&i| self.task_name(i).eq_ignore_ascii_case(name))
            .unwrap_or_else(|| panic!("no task {name}"));
        (self.app.run_task)(i as u32)
    }

    /// Scan `n` times, advancing the fake clock by `ms` before each scan.
    pub fn scans(&self, n: usize, ms: i64) {
        for _ in 0..n {
            self.advance(ms * MS);
            self.scan();
        }
    }

    /// Advance the fake clock.
    pub fn advance(&self, ns: i64) {
        FAKE_NOW_NS.fetch_add(ns, Ordering::SeqCst);
    }
}

/// Lower, compile and JIT a fixture, then hand the running PLC to `f`. The fake
/// clock starts at 1 s and is serialized across tests.
pub fn with_plc<R>(name: &str, f: impl FnOnce(&Plc) -> R) -> R {
    with_plc_src(name, &read_fixture(name), &plcc_l5x::Options::default(), f)
}

pub fn with_plc_src<R>(
    name: &str,
    src: &str,
    opts: &plcc_l5x::Options,
    f: impl FnOnce(&Plc) -> R,
) -> R {
    let _clock: MutexGuard<'_, ()> = CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    FAKE_NOW_NS.store(1_000 * MS, Ordering::SeqCst);
    let unit = with_preludes(lower_src(name, src, opts));
    let ctx = Context::create();
    let mut compiler = Compiler::new(&ctx, name);
    if let Err(e) = compiler.compile(&unit) {
        let detail = match e.span() {
            Some(s) if s.end <= src.len() => format!(" at `{}`", &src[s.start..s.end]),
            _ => String::new(),
        };
        let (st, _) = plcc_l5x::to_st(src, opts);
        panic!("{name}: codegen failed: {e}{detail}\n--- generated ST ---\n{st}");
    }
    let contract = compiler
        .runtime_contract("x86_64-unknown-linux-gnu")
        .expect("contract");
    // An accessor for the address of the VAR_GLOBAL block (controller tags).
    if let Some(gv) = compiler.module().get_global("plcc_globals") {
        let module = compiler.module();
        let ptr_ty = ctx.ptr_type(inkwell::AddressSpace::default());
        let accessor = module.add_function("test_globals_addr", ptr_ty.fn_type(&[], false), None);
        let bb = ctx.append_basic_block(accessor, "entry");
        let b = ctx.create_builder();
        b.position_at_end(bb);
        b.build_return(Some(&gv.as_pointer_value())).expect("ret");
    }
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
    let globals = match ee.get_function_address("test_globals_addr") {
        Ok(a) => {
            let acc: extern "C" fn() -> *mut u8 = unsafe { std::mem::transmute(a) };
            acc()
        }
        Err(_) => std::ptr::null_mut(),
    };
    (app.init)();
    f(&Plc {
        app,
        contract,
        globals,
    })
}
