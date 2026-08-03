// SPDX-License-Identifier: MPL-2.0

//! Execution tests for the `plcc_monotonic_ns` time hook.
//!
//! ST has no way to ask for the time, so timers cannot exist without one imported
//! symbol the platform supplies. `MONOTONIC_NS()` compiles to a call to the external
//! `plcc_monotonic_ns() -> i64`. These tests JIT-execute programs that call it —
//! once against a fake clock the test drives explicitly (so the assertions are on
//! exact values), and once against the real host clock from `plcc-runtime`.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, MutexGuard};

// ---------------------------------------------------------------------------
// A clock the test drives by hand.
// ---------------------------------------------------------------------------

static FAKE_NOW_NS: AtomicI64 = AtomicI64::new(0);
static FAKE_CLOCK_LOCK: Mutex<()> = Mutex::new(());

extern "C" fn fake_monotonic_ns() -> i64 {
    FAKE_NOW_NS.load(Ordering::SeqCst)
}

/// Serializes the tests that share `FAKE_NOW_NS`, and resets it to zero.
fn take_fake_clock() -> MutexGuard<'static, ()> {
    let guard = FAKE_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    FAKE_NOW_NS.store(0, Ordering::SeqCst);
    guard
}

fn set_now_ns(ns: i64) {
    FAKE_NOW_NS.store(ns, Ordering::SeqCst);
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn read_i64(state: &[u8], off: usize) -> i64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&state[off..off + 8]);
    i64::from_ne_bytes(b)
}

/// Runs `scans` scans of `<prog>_scan`, calling `before_scan(scan_index)` before each,
/// and returns the final state bytes.
fn run_with_clock(
    source: &str,
    prog: &str,
    clock: extern "C" fn() -> i64,
    scans: usize,
    mut before_scan: impl FnMut(usize),
) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");

    let context = Context::create();
    let mut compiler = Compiler::new(&context, "clock_test");
    compiler.compile(&unit).expect("codegen failed");

    let clock_decl = compiler
        .module()
        .get_function("plcc_monotonic_ns")
        .expect("the program uses the clock, so the module must import plcc_monotonic_ns");

    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT");
    ee.add_global_mapping(&clock_decl, clock as *const () as usize);

    let mut state = vec![0u8; 4096];
    let ptr = state.as_mut_ptr();
    if let Ok(a) = ee.get_function_address(&format!("{prog}_init")) {
        let f: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(a) };
        f(ptr);
    }
    let a = ee
        .get_function_address(&format!("{prog}_scan"))
        .expect("scan function missing");
    let scan: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(a) };

    for i in 0..scans {
        before_scan(i);
        scan(ptr);
    }
    state
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn monotonic_ns_emits_an_external_import() {
    let source = r#"
PROGRAM Clk
VAR
    t : LINT;
END_VAR
    t := MONOTONIC_NS();
END_PROGRAM
"#;
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let context = Context::create();
    let mut compiler = Compiler::new(&context, "import_test");
    compiler.compile(&unit).expect("codegen failed");
    let f = compiler
        .module()
        .get_function("plcc_monotonic_ns")
        .expect("plcc_monotonic_ns must be declared");
    assert_eq!(
        f.count_basic_blocks(),
        0,
        "plcc_monotonic_ns must be an import, not a definition"
    );
}

#[test]
fn clock_readings_land_in_state_exactly() {
    let _g = take_fake_clock();
    let source = r#"
PROGRAM Clk
VAR
    now : LINT;
END_VAR
    now := MONOTONIC_NS();
END_PROGRAM
"#;
    // One scan at t = 123_456_789 ns.
    let state = run_with_clock(source, "clk", fake_monotonic_ns, 1, |_| {
        set_now_ns(123_456_789)
    });
    assert_eq!(
        read_i64(&state, 0),
        123_456_789,
        "the exact clock reading must reach the program's state"
    );
}

#[test]
fn clock_is_monotonic_non_decreasing_across_scans_fake() {
    let _g = take_fake_clock();
    // The program latches the previous reading, computes the delta, and ORs together
    // a "went backwards" flag. If the clock ever decreased, `backwards` would be 1.
    let source = r#"
PROGRAM Mono
VAR
    prev : LINT;
    now : LINT;
    delta : LINT;
    total : LINT;
    backwards : BOOL := FALSE;
    first : BOOL := TRUE;
END_VAR
    prev := now;
    now := MONOTONIC_NS();
    IF first THEN
        first := FALSE;
    ELSE
        delta := now - prev;
        total := total + delta;
        IF delta < 0 THEN
            backwards := TRUE;
        END_IF;
    END_IF;
END_PROGRAM
"#;
    // 6 scans, advancing 1_000_000 ns (1 ms) each time, starting at 5_000_000.
    let state = run_with_clock(source, "mono", fake_monotonic_ns, 6, |i| {
        set_now_ns(5_000_000 + i as i64 * 1_000_000)
    });

    // Layout: prev@0, now@8, delta@16, total@24, backwards@32, first@33
    assert_eq!(read_i64(&state, 8), 10_000_000, "final clock reading");
    assert_eq!(read_i64(&state, 0), 9_000_000, "previous clock reading");
    assert_eq!(read_i64(&state, 16), 1_000_000, "last delta");
    assert_eq!(
        read_i64(&state, 24),
        5_000_000,
        "5 deltas of 1ms accumulated after the first scan"
    );
    assert_eq!(state[32], 0, "the clock must never go backwards");
    assert_eq!(state[33], 0, "`first` must have been cleared");
}

#[test]
fn clock_is_monotonic_non_decreasing_across_scans_real_host_clock() {
    // Same program, but against the real host clock from plcc-runtime — the
    // implementation `plcc sim` binds. Wall-clock values cannot be asserted exactly,
    // so this asserts the property: never decreasing, and strictly advancing over a
    // sleep.
    let source = r#"
PROGRAM RealMono
VAR
    prev : LINT;
    now : LINT;
    delta : LINT;
    backwards : BOOL := FALSE;
    first : BOOL := TRUE;
END_VAR
    prev := now;
    now := MONOTONIC_NS();
    IF first THEN
        first := FALSE;
    ELSE
        delta := now - prev;
        IF delta < 0 THEN
            backwards := TRUE;
        END_IF;
    END_IF;
END_PROGRAM
"#;
    let state = run_with_clock(
        source,
        "realmono",
        plcc_runtime::host_clock::plcc_monotonic_ns,
        4,
        |i| {
            if i > 0 {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        },
    );

    let prev = read_i64(&state, 0);
    let now = read_i64(&state, 8);
    let delta = read_i64(&state, 16);
    assert_eq!(state[24], 0, "the host clock must never go backwards");
    assert!(now >= prev, "now {now} < prev {prev}");
    assert!(
        delta >= 1_000_000,
        "a 2ms sleep between scans must show up as >=1ms of delta, got {delta}ns"
    );
    assert!(now > 0, "the host clock must return a nonzero reading");
}

#[test]
fn monotonic_ns_rejects_arguments() {
    let source = r#"
PROGRAM Bad
VAR
    t : LINT;
END_VAR
    t := MONOTONIC_NS(5);
END_PROGRAM
"#;
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let context = Context::create();
    let mut compiler = Compiler::new(&context, "bad_clock");
    let err = compiler
        .compile(&unit)
        .expect_err("MONOTONIC_NS takes no arguments");
    assert!(
        err.to_string().contains("MONOTONIC_NS"),
        "got: {}",
        err.to_string()
    );
}
