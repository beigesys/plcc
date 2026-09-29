// SPDX-License-Identifier: MPL-2.0

//! Shared JIT harness: compile ST (with the bundled standard library), run `p_init`
//! then `p_scan` a number of times, and read PROGRAM `p`'s variables back **by
//! name**, using the offsets from the runtime contract — so a test does not depend
//! on declaration order or padding.

#![allow(dead_code)]

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::passes::PassBuilderOptions;
use inkwell::targets::{CodeModel, InitializationConfig, RelocMode, Target, TargetMachine};
use plcc_codegen::Compiler;
use std::collections::HashMap;


thread_local! {
    static CLOCK_NS: std::cell::Cell<i64> = const { std::cell::Cell::new(0) };
}

extern "C" fn test_clock() -> i64 {
    CLOCK_NS.with(|c| c.get())
}

/// A `plcc_fault` call: the code and the site string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fault {
    pub code: u32,
    pub site: String,
}

/// The test's `plcc_fault`: unwind out of the JIT-compiled scan (it must not
/// return), carrying the fault to the `catch_unwind` around the scan call.
/// `resume_unwind` does not run the panic hook, so nothing is printed.
extern "C-unwind" fn test_fault(code: u32, site: *const std::ffi::c_char) {
    let site = if site.is_null() {
        String::new()
    } else {
        unsafe { std::ffi::CStr::from_ptr(site) }.to_string_lossy().into_owned()
    };
    std::panic::resume_unwind(Box::new(Fault { code, site }));
}

/// Values of PROGRAM `p` after the run, by (case-insensitive) variable name.
pub struct State {
    bytes: Vec<u8>,
    fields: HashMap<String, (usize, usize)>,
    /// The runtime fault that stopped the run, if any. Scans stop at a fault.
    pub fault: Option<Fault>,
    /// Scans that completed.
    pub scans: usize,
}

impl State {
    fn raw(&self, name: &str) -> &[u8] {
        let (o, n) = self
            .fields
            .get(&name.to_uppercase())
            .unwrap_or_else(|| panic!("no variable `{name}` in p"));
        &self.bytes[*o..*o + *n]
    }
    pub fn i64(&self, name: &str) -> i64 {
        let b = self.raw(name);
        match b.len() {
            1 => b[0] as i8 as i64,
            2 => i16::from_ne_bytes(b.try_into().unwrap()) as i64,
            4 => i32::from_ne_bytes(b.try_into().unwrap()) as i64,
            8 => i64::from_ne_bytes(b.try_into().unwrap()),
            n => panic!("`{name}` is {n} bytes, not an integer"),
        }
    }
    pub fn u64(&self, name: &str) -> u64 {
        let b = self.raw(name);
        match b.len() {
            1 => b[0] as u64,
            2 => u16::from_ne_bytes(b.try_into().unwrap()) as u64,
            4 => u32::from_ne_bytes(b.try_into().unwrap()) as u64,
            8 => u64::from_ne_bytes(b.try_into().unwrap()),
            n => panic!("`{name}` is {n} bytes, not an integer"),
        }
    }
    pub fn bool(&self, name: &str) -> bool {
        self.raw(name)[0] != 0
    }
    pub fn f64(&self, name: &str) -> f64 {
        let b = self.raw(name);
        match b.len() {
            4 => f32::from_ne_bytes(b.try_into().unwrap()) as f64,
            8 => f64::from_ne_bytes(b.try_into().unwrap()),
            n => panic!("`{name}` is {n} bytes, not a float"),
        }
    }
    pub fn str(&self, name: &str) -> String {
        let b = self.raw(name);
        let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
        String::from_utf8_lossy(&b[..end]).into_owned()
    }
    pub fn wstr(&self, name: &str) -> String {
        let w: Vec<u16> = self
            .raw(name)
            .chunks(2)
            .map(|c| u16::from_ne_bytes([c[0], c[1]]))
            .take_while(|&c| c != 0)
            .collect();
        String::from_utf16_lossy(&w)
    }
}

/// Compile `src` (plus the bundled stdlib), JIT it, run `scans` scans of `p`.
/// The clock advances by `dt_ms` after every scan.
pub fn run_with(src: &str, scans: usize, dt_ms: i64, optimize: bool) -> State {
    try_run_with(src, scans, dt_ms, optimize).unwrap_or_else(|e| panic!("{e}"))
}

/// Like [`run_with`], but a codegen error is returned instead of panicking.
pub fn try_run_with(src: &str, scans: usize, dt_ms: i64, optimize: bool) -> Result<State, String> {
    let source = format!("{}\n{}", plcc_stdlib::combined_source(), src);
    let (unit, errors) = plcc_st::parse(&source);
    if !errors.is_empty() {
        return Err(format!("parse errors: {errors:?}"));
    }
    let ctx = Context::create();
    let mut compiler = Compiler::new(&ctx, "hunt");
    compiler.compile(&unit).map_err(|e| e.to_string())?;
    if let Err(e) = compiler.module().verify() {
        panic!("invalid IR: {e}");
    }
    let contract = compiler
        .runtime_contract("x86_64-unknown-linux-gnu")
        .map_err(|e| e.to_string())?;
    let prog = contract
        .programs
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case("p"))
        .expect("PROGRAM p");
    let ty = &contract.types[prog.type_index];
    let fields = ty
        .fields
        .iter()
        .map(|f| (f.name.to_uppercase(), (f.offset as usize, f.size as usize)))
        .collect();
    if optimize {
        Target::initialize_native(&InitializationConfig::default()).unwrap();
        let triple = TargetMachine::get_default_triple();
        let tm = Target::from_triple(&triple)
            .unwrap()
            .create_target_machine(
                &triple,
                "generic",
                "",
                OptimizationLevel::Aggressive,
                RelocMode::Default,
                CodeModel::Default,
            )
            .unwrap();
        // The optimizer folds struct GEPs into byte offsets, so the module has to
        // carry the target's data layout first, or it lays out i64 fields with the
        // LLVM default (4-byte) alignment and disagrees with the contract offsets.
        compiler.module().set_triple(&triple);
        compiler
            .module()
            .set_data_layout(&tm.get_target_data().get_data_layout());
        compiler
            .module()
            .run_passes("default<O3>", &tm, PassBuilderOptions::create())
            .unwrap();
    }
    // Replace the module's weak default `plcc_fault` (a trap) with the test's
    // handler: drop the default body, then map the declaration.
    if let Some(f) = compiler.module().get_function("plcc_fault") {
        for bb in f.get_basic_blocks() {
            unsafe { bb.delete() }.unwrap();
        }
        f.set_linkage(inkwell::module::Linkage::External);
    }
    let ee = compiler
        .module()
        .create_jit_execution_engine(if optimize {
            OptimizationLevel::Aggressive
        } else {
            OptimizationLevel::None
        })
        .unwrap();
    if let Some(f) = compiler.module().get_function("plcc_monotonic_ns") {
        ee.add_global_mapping(&f, test_clock as *const () as usize);
    }
    if let Some(f) = compiler.module().get_function("plcc_fault") {
        ee.add_global_mapping(&f, test_fault as *const () as usize);
    }
    let mut bytes = vec![0u8; ty.size as usize + 64];
    CLOCK_NS.with(|c| c.set(0));
    let mut fault = None;
    let mut done = 0;
    // Run one JIT-compiled call, catching a fault unwound out of it.
    let mut call = |f: extern "C-unwind" fn(*mut u8), p: *mut u8| -> bool {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(p))) {
            Ok(()) => true,
            Err(payload) => match payload.downcast::<Fault>() {
                Ok(fl) => {
                    fault = Some(*fl);
                    false
                }
                Err(other) => std::panic::resume_unwind(other),
            },
        }
    };
    unsafe {
        let p = bytes.as_mut_ptr();
        let init_ok = match ee.get_function_address(&prog.init_fn) {
            Ok(a) => call(std::mem::transmute::<usize, extern "C-unwind" fn(*mut u8)>(a), p),
            Err(_) => true,
        };
        if init_ok {
            let a = ee.get_function_address(&prog.scan_fn).unwrap();
            let f: extern "C-unwind" fn(*mut u8) = std::mem::transmute(a);
            for _ in 0..scans {
                if !call(f, p) {
                    break;
                }
                done += 1;
                CLOCK_NS.with(|c| c.set(c.get() + dt_ms * 1_000_000));
            }
        }
    }
    Ok(State { bytes, fields, fault, scans: done })
}

/// One scan, unoptimized.
pub fn run(src: &str) -> State {
    run_with(src, 1, 10, false)
}

/// One scan, after the LLVM `default<O3>` pipeline — UB-dependent bugs only show
/// up here.
pub fn run_o3(src: &str) -> State {
    run_with(src, 1, 10, true)
}

/// The codegen error for `src`, panicking if it compiles.
pub fn compile_error(src: &str) -> String {
    match try_run_with(src, 0, 0, false) {
        Ok(_) => panic!("expected a compile error"),
        Err(e) => e,
    }
}
