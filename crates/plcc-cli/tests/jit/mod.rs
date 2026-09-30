// SPDX-License-Identifier: MPL-2.0

//! JIT a compilation unit and drive it through the runtime contract
//! (`plcc_get_app`), reading and writing variables by path: the harness of the
//! ladder differential tests.

#![allow(dead_code)]

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::execution_engine::ExecutionEngine;
use plcc_codegen::Compiler;
use plcc_codegen::compiler::contract::{RuntimeContract, VariableInfo};
use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, MutexGuard};

#[repr(C)]
struct Image {
    input: *mut u8,
    input_size: u32,
    output: *mut u8,
    output_size: u32,
    memory: *mut u8,
    memory_size: u32,
}

#[repr(C)]
struct Instance {
    name: *const c_char,
    program_type: *const c_char,
    init: extern "C" fn(*mut u8),
    scan: extern "C" fn(*mut u8),
    state: *mut u8,
    state_size: u64,
}

#[repr(C)]
struct Task {
    name: *const c_char,
    interval_ns: i64,
    priority: u32,
    program_count: u32,
    single: Option<extern "C" fn() -> u8>,
    programs: *const Instance,
}

#[repr(C)]
struct App {
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

/// Serialize tests that use the fake clock; starts it at 1 s.
pub fn clock() -> MutexGuard<'static, ()> {
    let g = CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    FAKE_NOW_NS.store(1_000 * MS, Ordering::SeqCst);
    g
}

pub fn advance(ns: i64) {
    FAKE_NOW_NS.fetch_add(ns, Ordering::SeqCst);
}

/// Prepend the bundled stdlib (and the Logix prelude when `logix`).
pub fn with_libs(unit: plcc_st::CompilationUnit, logix: bool) -> plcc_st::CompilationUnit {
    let mut decls = Vec::new();
    for u in plcc_stdlib::UNITS {
        let (su, errs) = plcc_st::parse(u.source);
        assert!(errs.is_empty());
        decls.extend(su.declarations);
    }
    if logix {
        let (pu, errs) = plcc_st::parse(&plcc_l5x::prelude());
        assert!(errs.is_empty());
        decls.extend(pu.declarations);
    }
    decls.extend(unit.declarations);
    plcc_st::CompilationUnit {
        declarations: decls,
        span: unit.span,
    }
}

pub struct Plc<'ctx> {
    ee: ExecutionEngine<'ctx>,
    app: *const App,
    pub contract: RuntimeContract,
}

/// Compile and JIT `unit` (libraries included by the caller) in `ctx`;
/// `plcc_init` has run.
pub fn load<'ctx>(
    ctx: &'ctx Context,
    name: &str,
    unit: &plcc_st::CompilationUnit,
) -> Result<Plc<'ctx>, String> {
    let mut compiler = Compiler::new(ctx, name);
    compiler
        .compile(unit)
        .map_err(|e| format!("{name}: codegen failed: {e}"))?;
    let contract = compiler
        .runtime_contract("x86_64-unknown-linux-gnu")
        .map_err(|e| format!("{name}: contract: {e}"))?;
    let clock = compiler.module().get_function("plcc_monotonic_ns");
    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .map_err(|e| format!("{name}: JIT: {e}"))?;
    if let Some(decl) = clock {
        ee.add_global_mapping(&decl, fake_monotonic_ns as *const () as usize);
    }
    let get = ee
        .get_function_address("plcc_get_app")
        .map_err(|e| format!("{name}: no plcc_get_app: {e:?}"))?;
    let get: extern "C" fn() -> *const App = unsafe { std::mem::transmute(get) };
    let app = get();
    unsafe {
        assert_eq!((*app).abi_version, 1);
        ((*app).init)();
    }
    Ok(Plc { ee, app, contract })
}

impl Plc<'_> {
    fn app(&self) -> &App {
        unsafe { &*self.app }
    }

    fn image(&self) -> &Image {
        unsafe { &*self.app().image }
    }

    fn instance_state(&self, symbol: &str) -> Option<*mut u8> {
        let inst = self
            .contract
            .instances
            .iter()
            .find(|i| i.symbol == symbol)?;
        let app = self.app();
        let tasks = unsafe { std::slice::from_raw_parts(app.tasks, app.task_count as usize) };
        for t in tasks {
            let progs = unsafe { std::slice::from_raw_parts(t.programs, t.program_count as usize) };
            for p in progs {
                let n = unsafe { CStr::from_ptr(p.name) }.to_string_lossy();
                if n == inst.name {
                    return Some(p.state);
                }
            }
        }
        None
    }

    fn base(&self, symbol: &str) -> Option<*mut u8> {
        let img = self.image();
        match symbol {
            "plcc_image_i" => Some(img.input),
            "plcc_image_q" => Some(img.output),
            "plcc_image_m" => Some(img.memory),
            s => self
                .instance_state(s)
                .or_else(|| self.ee.get_function_address(s).ok().map(|a| a as *mut u8)),
        }
    }

    pub fn variable(&self, path: &str) -> Option<&VariableInfo> {
        let want = path.to_ascii_uppercase();
        self.contract
            .variables
            .iter()
            .find(|v| v.path.to_ascii_uppercase() == want)
    }

    pub fn get_var(&self, v: &VariableInfo) -> Option<i64> {
        let p = unsafe { self.base(&v.symbol)?.add(v.offset as usize) };
        unsafe {
            if let Some(b) = v.bit {
                return Some(((*p >> b) & 1) as i64);
            }
            Some(match v.size {
                1 => *p as i8 as i64,
                2 => (p as *const i16).read_unaligned() as i64,
                4 => (p as *const i32).read_unaligned() as i64,
                8 => (p as *const i64).read_unaligned(),
                _ => return None,
            })
        }
    }

    pub fn set_var(&self, v: &VariableInfo, value: i64) {
        let Some(base) = self.base(&v.symbol) else {
            return;
        };
        let p = unsafe { base.add(v.offset as usize) };
        unsafe {
            if let Some(b) = v.bit {
                if value != 0 {
                    *p |= 1 << b;
                } else {
                    *p &= !(1 << b);
                }
                return;
            }
            match v.size {
                1 => *p = value as u8,
                2 => (p as *mut i16).write_unaligned(value as i16),
                4 => (p as *mut i32).write_unaligned(value as i32),
                8 => (p as *mut i64).write_unaligned(value),
                _ => {}
            }
        }
    }

    pub fn get(&self, path: &str) -> i64 {
        let v = self
            .variable(path)
            .unwrap_or_else(|| panic!("no variable {path}"));
        self.get_var(v).unwrap_or(0)
    }

    pub fn set(&self, path: &str, value: i64) {
        let v = self
            .variable(path)
            .unwrap_or_else(|| panic!("no variable {path}"))
            .clone();
        self.set_var(&v, value);
    }

    /// Run every task once, in order.
    pub fn scan(&self) {
        for i in 0..self.app().task_count {
            (self.app().run_task)(i)
        }
    }
}

/// A tiny deterministic generator (xorshift), so a failing sequence repeats.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }

    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    pub fn bool(&mut self) -> bool {
        self.next() & 1 == 1
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// Is this a variable the differential tests compare (not a hidden
/// temporary of a lowering, not a library FB's internals)?
pub fn observable(v: &VariableInfo) -> bool {
    let p = v.path.to_ascii_lowercase();
    !p.split('.')
        .any(|s| s.starts_with("_ld_") || s.starts_with("lx__"))
        && (v.bit.is_some() || matches!(v.size, 1 | 2 | 4 | 8))
}

/// Options of [`differential_with`].
#[derive(Clone, Debug, Default)]
pub struct Diff {
    /// Pair variables by their last path segment (when it is unique in both
    /// programs) instead of by full path: for programs whose instances are
    /// named differently (an L5X program vs a PLCopen POU).
    pub by_name: bool,
    /// Scans run before comparing starts (first-scan / prescan differences).
    pub skip_first: usize,
    /// Only compare variables of this byte size or bits (0: any size).
    pub bool_only: bool,
    /// Names (last segment) not compared.
    pub ignore: Vec<String>,
    /// When set, only these names (last segment) are compared.
    pub only: Option<Vec<String>>,
    /// Integer inputs (last segment), set to small random values each scan.
    pub ints: Vec<String>,
}

fn last(path: &str) -> String {
    path.rsplit('.').next().unwrap_or("").to_ascii_lowercase()
}

/// [`differential`] with options.
pub fn differential_with(
    a: &Plc,
    b: &Plc,
    inputs: &[String],
    scans: usize,
    seed: u64,
    max_step_ms: u64,
    opts: &Diff,
) -> Result<usize, String> {
    let pairs: Vec<(VariableInfo, VariableInfo)> = if opts.by_name {
        let count = |p: &Plc, n: &str| {
            p.contract
                .variables
                .iter()
                .filter(|v| observable(v) && last(&v.path) == n)
                .count()
        };
        a.contract
            .variables
            .iter()
            .filter(|v| observable(v))
            .filter(|v| count(a, &last(&v.path)) == 1 && count(b, &last(&v.path)) == 1)
            .filter_map(|v| {
                let n = last(&v.path);
                b.contract
                    .variables
                    .iter()
                    .find(|w| observable(w) && last(&w.path) == n)
                    .map(|w| (v.clone(), w.clone()))
            })
            .collect()
    } else {
        a.contract
            .variables
            .iter()
            .filter(|v| observable(v))
            .filter_map(|v| b.variable(&v.path).map(|w| (v.clone(), w.clone())))
            .collect()
    };
    let is_bool = |v: &VariableInfo| v.bit.is_some() || v.iec_type.eq_ignore_ascii_case("BOOL");
    let pairs: Vec<_> = pairs
        .into_iter()
        .filter(|(x, y)| (is_bool(x) == is_bool(y)) && (is_bool(x) || x.size == y.size))
        .filter(|(x, _)| !opts.bool_only || is_bool(x))
        .filter(|(x, _)| {
            !opts
                .ignore
                .iter()
                .any(|i| i.eq_ignore_ascii_case(&last(&x.path)))
        })
        .filter(|(x, _)| {
            opts.only
                .as_ref()
                .is_none_or(|o| o.iter().any(|i| i.eq_ignore_ascii_case(&last(&x.path))))
        })
        .collect();
    if pairs.is_empty() {
        return Err("no variables in common".into());
    }
    let is_input = |v: &VariableInfo| {
        let l = last(&v.path);
        inputs.iter().any(|n| n.eq_ignore_ascii_case(&l))
    };
    let mut rng = Rng::new(seed);
    let mut compared = 0;
    for scan in 0..scans {
        for (va, vb) in &pairs {
            if is_input(va) && is_bool(va) {
                // Inputs stay FALSE through the skipped first scans.
                let x = (rng.bool() && scan >= opts.skip_first) as i64;
                a.set_var(va, x);
                b.set_var(vb, x);
            } else if !is_bool(va)
                && opts
                    .ints
                    .iter()
                    .any(|n| n.eq_ignore_ascii_case(&last(&va.path)))
            {
                let x = rng.below(21) as i64 - 10;
                a.set_var(va, x);
                b.set_var(vb, x);
            }
        }
        advance(rng.below(max_step_ms + 1) as i64 * MS);
        a.scan();
        b.scan();
        if scan < opts.skip_first {
            continue;
        }
        for (va, vb) in &pairs {
            let (x, y) = (a.get_var(va), b.get_var(vb));
            if x != y {
                return Err(format!(
                    "scan {scan}: {} is {x:?} in the first program, {} is {y:?} in the second",
                    va.path, vb.path
                ));
            }
            compared += 1;
        }
    }
    Ok(compared)
}

/// Run `a` and `b` over `scans` scans, randomizing the `inputs` (variable
/// names, matched against the last path segment) before each, advancing the
/// clock by 0..`max_step_ms` ms, and compare every observable variable the two
/// have in common after each scan. Returns the first difference.
pub fn differential(
    a: &Plc,
    b: &Plc,
    inputs: &[String],
    ints: &[String],
    scans: usize,
    seed: u64,
    max_step_ms: u64,
) -> Result<usize, String> {
    let is_input = |v: &VariableInfo, names: &[String]| {
        let last = v.path.rsplit('.').next().unwrap_or("");
        names.iter().any(|n| n.eq_ignore_ascii_case(last))
    };
    let pairs: Vec<(VariableInfo, VariableInfo)> = a
        .contract
        .variables
        .iter()
        .filter(|v| observable(v))
        .filter_map(|v| b.variable(&v.path).map(|w| (v.clone(), w.clone())))
        .collect();
    if pairs.is_empty() {
        return Err("no variables in common".into());
    }
    let mut rng = Rng::new(seed);
    let mut compared = 0;
    for scan in 0..scans {
        for (va, vb) in &pairs {
            if is_input(va, inputs) && (va.bit.is_some() || va.size == 1) {
                let x = rng.bool() as i64;
                a.set_var(va, x);
                b.set_var(vb, x);
            } else if is_input(va, ints) {
                let x = rng.below(20) as i64 - 5;
                a.set_var(va, x);
                b.set_var(vb, x);
            }
        }
        advance(rng.below(max_step_ms + 1) as i64 * MS);
        a.scan();
        b.scan();
        for (va, vb) in &pairs {
            let (x, y) = (a.get_var(va), b.get_var(vb));
            if x != y {
                return Err(format!(
                    "scan {scan}: {} is {x:?} in the first program, {y:?} in the second",
                    va.path
                ));
            }
            compared += 1;
        }
    }
    Ok(compared)
}
