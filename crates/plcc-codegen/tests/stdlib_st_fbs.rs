// SPDX-License-Identifier: MPL-2.0

//! Execution traces for the bundled ST standard function blocks.
//!
//! Every test compiles `plcc_stdlib::combined_source()` together with a driver
//! PROGRAM, JIT-executes it scan by scan, and asserts the *whole output sequence*
//! across scans — not just a final value. Timer tests bind `plcc_monotonic_ns` to a
//! clock the test sets explicitly, so nothing depends on wall-clock timing.
//!
//! Driver programs declare all their observable variables as LINT so each one
//! occupies exactly eight bytes at a predictable offset; the FB instances come last.
//! BOOL inputs are driven from LINT values (codegen truncates on the way into the
//! FB's BOOL field) and BOOL outputs are read back widened to LINT.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, MutexGuard};

// ---------------------------------------------------------------------------
// A clock the test drives by hand.
// ---------------------------------------------------------------------------

static FAKE_NOW_NS: AtomicI64 = AtomicI64::new(0);
static CLOCK_LOCK: Mutex<()> = Mutex::new(());

extern "C" fn fake_monotonic_ns() -> i64 {
    FAKE_NOW_NS.load(Ordering::SeqCst)
}

fn take_clock() -> MutexGuard<'static, ()> {
    let g = CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    FAKE_NOW_NS.store(0, Ordering::SeqCst);
    g
}

const MS: i64 = 1_000_000;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// A JIT-compiled driver program that can be stepped one scan at a time.
struct Rig<'ctx> {
    ee: inkwell::execution_engine::ExecutionEngine<'ctx>,
    scan: extern "C" fn(*mut u8),
    state: Vec<u8>,
}

impl Rig<'_> {
    fn slot(&self, i: usize) -> i64 {
        let off = i * 8;
        let mut b = [0u8; 8];
        b.copy_from_slice(&self.state[off..off + 8]);
        i64::from_ne_bytes(b)
    }

    fn set_slot(&mut self, i: usize, v: i64) {
        let off = i * 8;
        self.state[off..off + 8].copy_from_slice(&v.to_ne_bytes());
    }

    fn step(&mut self) {
        (self.scan)(self.state.as_mut_ptr());
    }
}

/// Compile the bundled stdlib plus `driver`, JIT it, and bind the clock.
fn rig<'ctx>(
    context: &'ctx Context,
    driver: &str,
    prog: &str,
    clock: extern "C" fn() -> i64,
) -> Rig<'ctx> {
    let source = format!("{}\n{}", plcc_stdlib::combined_source(), driver);
    let (unit, errors) = plcc_st::parse(&source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");

    let mut compiler = Compiler::new(context, "stdlib_st");
    compiler.compile(&unit).expect("codegen failed");

    // Bind the clock before creating the execution engine's function addresses.
    let clock_decl = compiler.module().get_function("plcc_monotonic_ns");

    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT");
    if let Some(decl) = clock_decl {
        ee.add_global_mapping(&decl, clock as *const () as usize);
    }

    let mut state = vec![0u8; 8192];
    let ptr = state.as_mut_ptr();
    if let Ok(a) = ee.get_function_address(&format!("{prog}_init")) {
        let f: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(a) };
        f(ptr);
    }
    let a = ee
        .get_function_address(&format!("{prog}_scan"))
        .expect("scan function missing");
    let scan: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(a) };

    Rig { ee, scan, state }
}

/// Keeps the execution engine alive for the lifetime of the rig.
impl Drop for Rig<'_> {
    fn drop(&mut self) {
        let _ = &self.ee;
    }
}

// ---------------------------------------------------------------------------
// Sanity: the bundled library compiles at all, and defines what it claims to.
// ---------------------------------------------------------------------------

#[test]
fn bundled_stdlib_compiles_on_its_own() {
    let source = plcc_stdlib::combined_source();
    let (unit, errors) = plcc_st::parse(&source);
    assert!(
        errors.is_empty(),
        "the bundled stdlib must parse: {errors:?}"
    );
    let context = Context::create();
    let mut compiler = Compiler::new(&context, "stdlib_only");
    compiler
        .compile(&unit)
        .expect("the bundled stdlib must compile");
    for name in plcc_stdlib::POU_NAMES {
        let fn_name = format!("{}_scan", name.to_lowercase());
        let f = compiler
            .module()
            .get_function(&fn_name)
            .unwrap_or_else(|| panic!("missing {fn_name}"));
        assert!(
            f.count_basic_blocks() > 0,
            "{fn_name} must have a body, not be an empty declaration"
        );
    }
}

// ---------------------------------------------------------------------------
// R_TRIG / F_TRIG
// ---------------------------------------------------------------------------

#[test]
fn r_trig_pulses_for_exactly_one_scan() {
    let driver = r#"
PROGRAM RT
VAR
    d_clk : LINT;
    o_q : LINT;
    e : R_TRIG;
END_VAR
    e(CLK := d_clk);
    o_q := e.Q;
END_PROGRAM
"#;
    let ctx = Context::create();
    let mut r = rig(&ctx, driver, "rt", fake_monotonic_ns);

    //           scan:  1  2  3  4  5  6
    let clk = [0i64, 1, 1, 0, 1, 0];
    let expect_q = [0i64, 1, 0, 0, 1, 0];
    let mut got = Vec::new();
    for &c in &clk {
        r.set_slot(0, c);
        r.step();
        got.push(r.slot(1));
    }
    assert_eq!(
        got,
        expect_q.to_vec(),
        "R_TRIG.Q must be TRUE for exactly one scan per rising edge"
    );
}

#[test]
fn f_trig_pulses_for_exactly_one_scan_and_not_on_the_first() {
    let driver = r#"
PROGRAM FT
VAR
    d_clk : LINT;
    o_q : LINT;
    e : F_TRIG;
END_VAR
    e(CLK := d_clk);
    o_q := e.Q;
END_PROGRAM
"#;
    let ctx = Context::create();
    let mut r = rig(&ctx, driver, "ft", fake_monotonic_ns);

    //           scan:  1  2  3  4  5  6
    let clk = [0i64, 1, 1, 0, 0, 1];
    let expect_q = [0i64, 0, 0, 1, 0, 0];
    let mut got = Vec::new();
    for &c in &clk {
        r.set_slot(0, c);
        r.step();
        got.push(r.slot(1));
    }
    assert_eq!(
        got,
        expect_q.to_vec(),
        "F_TRIG.Q must pulse once per falling edge, and not on the first scan"
    );
}

// ---------------------------------------------------------------------------
// SR / RS — dominance
// ---------------------------------------------------------------------------

#[test]
fn sr_is_set_dominant() {
    let driver = r#"
PROGRAM SRT
VAR
    d_s1 : LINT;
    d_r : LINT;
    o_q1 : LINT;
    b : SR;
END_VAR
    b(S1 := d_s1, R := d_r);
    o_q1 := b.Q1;
END_PROGRAM
"#;
    let ctx = Context::create();
    let mut r = rig(&ctx, driver, "srt", fake_monotonic_ns);

    //          (S1, R)
    let steps = [(0i64, 0i64), (1, 0), (0, 0), (0, 1), (1, 1), (0, 1)];
    let expect = [0i64, 1, 1, 0, 1, 0];
    let mut got = Vec::new();
    for &(s1, rr) in &steps {
        r.set_slot(0, s1);
        r.set_slot(1, rr);
        r.step();
        got.push(r.slot(2));
    }
    assert_eq!(
        got,
        expect.to_vec(),
        "SR must be SET dominant: S1 and R both TRUE leaves Q1 TRUE"
    );
}

#[test]
fn rs_is_reset_dominant() {
    let driver = r#"
PROGRAM RST
VAR
    d_s : LINT;
    d_r1 : LINT;
    o_q1 : LINT;
    b : RS;
END_VAR
    b(S := d_s, R1 := d_r1);
    o_q1 := b.Q1;
END_PROGRAM
"#;
    let ctx = Context::create();
    let mut r = rig(&ctx, driver, "rst", fake_monotonic_ns);

    //          (S, R1)
    let steps = [(0i64, 0i64), (1, 0), (0, 0), (1, 1), (1, 0), (0, 1)];
    let expect = [0i64, 1, 1, 0, 1, 0];
    let mut got = Vec::new();
    for &(s, r1) in &steps {
        r.set_slot(0, s);
        r.set_slot(1, r1);
        r.step();
        got.push(r.slot(2));
    }
    assert_eq!(
        got,
        expect.to_vec(),
        "RS must be RESET dominant: S and R1 both TRUE forces Q1 FALSE"
    );
}

// ---------------------------------------------------------------------------
// CTU
// ---------------------------------------------------------------------------

#[test]
fn ctu_counts_edges_and_reset_beats_a_coincident_edge() {
    let driver = r#"
PROGRAM CU1
VAR
    d_cu : LINT;
    d_r : LINT;
    o_q : LINT;
    o_cv : LINT;
    c : CTU;
END_VAR
    c(CU := d_cu, R := d_r, PV := 3);
    o_q := c.Q;
    o_cv := c.CV;
END_PROGRAM
"#;
    let ctx = Context::create();
    let mut r = rig(&ctx, driver, "cu1", fake_monotonic_ns);

    //          (CU, R)
    let steps = [
        (0i64, 0i64), // 1: idle
        (1, 0),       // 2: rising edge -> 1
        (1, 0),       // 3: held high, no further count
        (0, 0),       // 4: falling, no count
        (1, 0),       // 5: edge -> 2
        (0, 0),       // 6
        (1, 0),       // 7: edge -> 3, Q
        (1, 1),       // 8: R with CU still high -> 0
        (0, 1),       // 9: R holds it at 0
        (1, 1),       // 10: rising edge coincident with R -> R wins, stays 0
        (1, 0),       // 11: CU still high, no new edge -> stays 0
        (0, 0),       // 12
        (1, 0),       // 13: edge -> 1
    ];
    let expect_cv = [0i64, 1, 1, 1, 2, 2, 3, 0, 0, 0, 0, 0, 1];
    let expect_q = [0i64, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];

    let mut cv = Vec::new();
    let mut q = Vec::new();
    for &(cu, rr) in &steps {
        r.set_slot(0, cu);
        r.set_slot(1, rr);
        r.step();
        q.push(r.slot(2));
        cv.push(r.slot(3));
    }
    assert_eq!(cv, expect_cv.to_vec(), "CTU.CV trace");
    assert_eq!(q, expect_q.to_vec(), "CTU.Q trace (Q := CV >= PV)");
}

#[test]
fn ctu_saturates_at_int_max_instead_of_wrapping() {
    // Preload CV to 32766 through the FB's own output field, then count twice.
    // The second count must be refused, not wrap to -32768.
    let driver = r#"
PROGRAM CUSAT
VAR
    d_cu : LINT;
    o_cv : LINT;
    seed : LINT;
    c : CTU;
END_VAR
    IF seed = 0 THEN
        c.CV := 32766;
        seed := 1;
    END_IF;
    c(CU := d_cu, R := FALSE, PV := 32767);
    o_cv := c.CV;
END_PROGRAM
"#;
    let ctx = Context::create();
    let mut r = rig(&ctx, driver, "cusat", fake_monotonic_ns);

    let steps = [0i64, 1, 0, 1, 0, 1];
    let expect = [32766i64, 32767, 32767, 32767, 32767, 32767];
    let mut got = Vec::new();
    for &cu in &steps {
        r.set_slot(0, cu);
        r.step();
        got.push(r.slot(1));
    }
    assert_eq!(
        got,
        expect.to_vec(),
        "CTU.CV must saturate at INT max, never wrap"
    );
}

// ---------------------------------------------------------------------------
// CTD
// ---------------------------------------------------------------------------

#[test]
fn ctd_loads_then_counts_down() {
    let driver = r#"
PROGRAM CD1
VAR
    d_cd : LINT;
    d_ld : LINT;
    o_q : LINT;
    o_cv : LINT;
    c : CTD;
END_VAR
    c(CD := d_cd, LD := d_ld, PV := 2);
    o_q := c.Q;
    o_cv := c.CV;
END_PROGRAM
"#;
    let ctx = Context::create();
    let mut r = rig(&ctx, driver, "cd1", fake_monotonic_ns);

    //          (CD, LD)
    let steps = [
        (0i64, 1i64), // 1: load -> 2
        (1, 0),       // 2: edge -> 1
        (1, 0),       // 3: held, no count
        (0, 0),       // 4
        (1, 0),       // 5: edge -> 0, Q
        (0, 0),       // 6
        (1, 0),       // 7: edge -> -1, Q stays
        (1, 1),       // 8: LD beats the held CD -> 2
    ];
    let expect_cv = [2i64, 1, 1, 1, 0, 0, -1, 2];
    let expect_q = [0i64, 0, 0, 0, 1, 1, 1, 0];

    let mut cv = Vec::new();
    let mut q = Vec::new();
    for &(cd, ld) in &steps {
        r.set_slot(0, cd);
        r.set_slot(1, ld);
        r.step();
        q.push(r.slot(2));
        cv.push(r.slot(3));
    }
    assert_eq!(cv, expect_cv.to_vec(), "CTD.CV trace");
    assert_eq!(q, expect_q.to_vec(), "CTD.Q trace (Q := CV <= 0)");
}

// ---------------------------------------------------------------------------
// CTUD
// ---------------------------------------------------------------------------

#[test]
fn ctud_counts_both_ways_and_ignores_simultaneous_edges() {
    let driver = r#"
PROGRAM UD1
VAR
    d_cu : LINT;
    d_cd : LINT;
    d_r : LINT;
    d_ld : LINT;
    o_qu : LINT;
    o_qd : LINT;
    o_cv : LINT;
    c : CTUD;
END_VAR
    c(CU := d_cu, CD := d_cd, R := d_r, LD := d_ld, PV := 5);
    o_qu := c.QU;
    o_qd := c.QD;
    o_cv := c.CV;
END_PROGRAM
"#;
    let ctx = Context::create();
    let mut r = rig(&ctx, driver, "ud1", fake_monotonic_ns);

    //          (CU, CD, R, LD)
    let steps = [
        (1i64, 0i64, 0i64, 0i64), // 1: up   -> 1
        (0, 0, 0, 0),             // 2: idle
        (1, 1, 0, 0),             // 3: BOTH edges in one scan -> no change
        (0, 0, 0, 0),             // 4: idle
        (0, 1, 0, 0),             // 5: down -> 0
        (1, 0, 0, 0),             // 6: up   -> 1
        (1, 0, 1, 0),             // 7: R beats the up edge -> 0
        (0, 0, 0, 1),             // 8: LD -> PV = 5
        (1, 0, 1, 1),             // 9: R beats LD -> 0
    ];
    let expect_cv = [1i64, 1, 1, 1, 0, 1, 0, 5, 0];
    let expect_qu = [0i64, 0, 0, 0, 0, 0, 0, 1, 0];
    let expect_qd = [0i64, 0, 0, 0, 1, 0, 1, 0, 1];

    let (mut cv, mut qu, mut qd) = (Vec::new(), Vec::new(), Vec::new());
    for &(cu, cd, rr, ld) in &steps {
        r.set_slot(0, cu);
        r.set_slot(1, cd);
        r.set_slot(2, rr);
        r.set_slot(3, ld);
        r.step();
        qu.push(r.slot(4));
        qd.push(r.slot(5));
        cv.push(r.slot(6));
    }
    assert_eq!(cv, expect_cv.to_vec(), "CTUD.CV trace");
    assert_eq!(qu, expect_qu.to_vec(), "CTUD.QU trace (CV >= PV)");
    assert_eq!(qd, expect_qd.to_vec(), "CTUD.QD trace (CV <= 0)");
}

// ---------------------------------------------------------------------------
// TON
// ---------------------------------------------------------------------------

const TIMER_DRIVER: &str = r#"
PROGRAM TMR
VAR
    d_in : LINT;
    o_q : LINT;
    o_et : LINT;
    t : %FB%;
END_VAR
    t(IN := d_in, PT := T#100ms);
    o_q := t.Q;
    o_et := t.ET;
END_PROGRAM
"#;

/// Run a timer driver over `(time_ns, IN)` steps, returning `(Q, ET)` per scan.
fn run_timer(fb: &str, steps: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let driver = TIMER_DRIVER.replace("%FB%", fb);
    let ctx = Context::create();
    let mut r = rig(&ctx, &driver, "tmr", fake_monotonic_ns);
    let mut out = Vec::new();
    for &(now, inp) in steps {
        FAKE_NOW_NS.store(now, Ordering::SeqCst);
        r.set_slot(0, inp);
        r.step();
        out.push((r.slot(1), r.slot(2)));
    }
    out
}

#[test]
fn ton_delays_then_caps_and_freezes_et() {
    let _g = take_clock();
    //            (t_ns,       IN)
    let steps = [
        (0, 0),         // idle
        (0, 1),         // start
        (50 * MS, 1),   // halfway
        (99 * MS, 1),   // 1ms short
        (100 * MS, 1),  // exactly PT -> Q
        (150 * MS, 1),  // past PT: ET frozen at PT
        (1000 * MS, 1), // far past: still frozen
        (1010 * MS, 0), // IN drops: reset
    ];
    let expect = [
        (0, 0),
        (0, 0),
        (0, 50 * MS),
        (0, 99 * MS),
        (1, 100 * MS),
        (1, 100 * MS),
        (1, 100 * MS),
        (0, 0),
    ];
    assert_eq!(run_timer("TON", &steps), expect.to_vec(), "TON trace");
}

#[test]
fn ton_restarts_when_in_drops_before_pt_elapses() {
    let _g = take_clock();
    let steps = [
        (0, 1),        // start at t=0
        (40 * MS, 1),  // ET = 40ms, not yet
        (50 * MS, 0),  // IN drops before PT -> abandoned
        (60 * MS, 1),  // restart at t=60
        (100 * MS, 1), // 40ms in — Q must still be FALSE
        (159 * MS, 1), // 99ms in
        (160 * MS, 1), // exactly 100ms in -> Q
    ];
    let expect = [
        (0, 0),
        (0, 40 * MS),
        (0, 0),
        (0, 0),
        (0, 40 * MS),
        (0, 99 * MS),
        (1, 100 * MS),
    ];
    assert_eq!(
        run_timer("TON", &steps),
        expect.to_vec(),
        "a TON retriggered before PT must restart from zero, not carry over"
    );
}

// ---------------------------------------------------------------------------
// TOF
// ---------------------------------------------------------------------------

#[test]
fn tof_is_the_mirror_of_ton() {
    let _g = take_clock();
    let steps = [
        (0, 0),        // idle: Q FALSE, ET 0
        (10 * MS, 1),  // IN rises: Q TRUE immediately
        (20 * MS, 1),  // held
        (30 * MS, 0),  // IN falls: off-delay starts
        (80 * MS, 0),  // 50ms in
        (129 * MS, 0), // 99ms in
        (130 * MS, 0), // exactly PT -> Q FALSE, ET frozen at PT
        (500 * MS, 0), // far past: frozen
        (510 * MS, 1), // IN rises again: Q TRUE, ET cleared
    ];
    let expect = [
        (0, 0),
        (1, 0),
        (1, 0),
        (1, 0),
        (1, 50 * MS),
        (1, 99 * MS),
        (0, 100 * MS),
        (0, 100 * MS),
        (1, 0),
    ];
    assert_eq!(run_timer("TOF", &steps), expect.to_vec(), "TOF trace");
}

#[test]
fn tof_cancels_the_off_delay_when_in_returns() {
    let _g = take_clock();
    let steps = [
        (0, 1),        // Q TRUE
        (10 * MS, 0),  // falling edge, off-delay starts
        (60 * MS, 0),  // 50ms in, Q still TRUE
        (70 * MS, 1),  // IN back: cancel, ET cleared
        (200 * MS, 1), // still TRUE, ET stays 0
        (210 * MS, 0), // new falling edge
        (250 * MS, 0), // 40ms in
        (310 * MS, 0), // 100ms in -> expires
    ];
    let expect = [
        (1, 0),
        (1, 0),
        (1, 50 * MS),
        (1, 0),
        (1, 0),
        (1, 0),
        (1, 40 * MS),
        (0, 100 * MS),
    ];
    assert_eq!(
        run_timer("TOF", &steps),
        expect.to_vec(),
        "IN returning TRUE must cancel an in-flight TOF off-delay"
    );
}

// ---------------------------------------------------------------------------
// TP
// ---------------------------------------------------------------------------

#[test]
fn tp_is_a_one_shot_pulse_that_is_not_retriggerable() {
    let _g = take_clock();
    let steps = [
        (0, 0),        // idle
        (10 * MS, 1),  // rising edge: pulse starts
        (50 * MS, 1),  // 40ms in
        (60 * MS, 0),  // IN drops mid-pulse: pulse continues
        (70 * MS, 1),  // rising edge mid-pulse: MUST NOT restart ET
        (109 * MS, 1), // 99ms in
        (110 * MS, 1), // exactly PT: Q FALSE, ET frozen at PT
        (120 * MS, 1), // IN still TRUE: ET holds at PT
        (130 * MS, 0), // IN drops: ET cleared
        (140 * MS, 1), // new rising edge: new pulse from zero
        (180 * MS, 1), // 40ms into the second pulse
    ];
    let expect = [
        (0, 0),
        (1, 0),
        (1, 40 * MS),
        (1, 50 * MS),
        (1, 60 * MS),
        (1, 99 * MS),
        (0, 100 * MS),
        (0, 100 * MS),
        (0, 0),
        (1, 0),
        (1, 40 * MS),
    ];
    assert_eq!(run_timer("TP", &steps), expect.to_vec(), "TP trace");
}

// ---------------------------------------------------------------------------
// Several standard blocks composed together, driven on an explicit clock.
// ---------------------------------------------------------------------------

#[test]
fn timers_edge_detector_and_counter_compose_into_a_blinker() {
    let _g = take_clock();
    // Same shape as tests/fixtures/programs/stdlib_blinker.st, with LINT observables
    // so the state offsets are trivial.
    let driver = r#"
PROGRAM BLINK
VAR
    o_lamp : LINT;
    o_count : LINT;
    on_delay : TON;
    off_delay : TON;
    edge : R_TRIG;
    cycles : CTU;
    lamp : BOOL := TRUE;
END_VAR
    on_delay(IN := lamp, PT := T#250ms);
    IF on_delay.Q THEN
        lamp := FALSE;
    END_IF;
    off_delay(IN := NOT lamp, PT := T#250ms);
    IF off_delay.Q THEN
        lamp := TRUE;
    END_IF;
    edge(CLK := lamp);
    cycles(CU := edge.Q, R := FALSE, PV := 1000);
    o_lamp := lamp;
    o_count := cycles.CV;
END_PROGRAM
"#;
    let ctx = Context::create();
    let mut r = rig(&ctx, driver, "blink", fake_monotonic_ns);

    // Scans every 250ms. The lamp toggles every 250ms of on-time / off-time, so the
    // full blink period is 500ms and the counter advances once per period.
    let times = [0i64, 250, 500, 750, 1000, 1250, 1500, 1750];
    let expect_lamp = [1i64, 0, 1, 1, 0, 1, 1, 0];
    let expect_count = [1i64, 1, 2, 2, 2, 3, 3, 3];

    let (mut lamp, mut count) = (Vec::new(), Vec::new());
    for &t in &times {
        FAKE_NOW_NS.store(t * MS, Ordering::SeqCst);
        r.step();
        lamp.push(r.slot(0));
        count.push(r.slot(1));
    }
    assert_eq!(lamp, expect_lamp.to_vec(), "lamp trace");
    assert_eq!(
        count,
        expect_count.to_vec(),
        "R_TRIG + CTU must count one rising edge of the lamp per blink period"
    );
}

// ---------------------------------------------------------------------------
// A user definition supersedes the prelude one.
// ---------------------------------------------------------------------------

#[test]
fn a_user_defined_fb_of_the_same_name_can_replace_the_prelude_one() {
    // This mirrors what the CLI's --stdlib=bundled-st injection does: the prelude
    // declaration is dropped when the user defines a POU with the same name. Here
    // the "prelude" is the subset that does not collide, plus the user's own CTU.
    let source = format!(
        "{}\n{}",
        plcc_stdlib::TIMERS.source,
        r#"
FUNCTION_BLOCK CTU
VAR_INPUT
    CU : BOOL;
    R : BOOL;
    PV : INT;
END_VAR
VAR_OUTPUT
    Q : BOOL;
    CV : INT;
END_VAR
    (* deliberately different: counts on level, not edge *)
    IF CU THEN
        CV := CV + 10;
    END_IF;
    Q := CV >= PV;
END_FUNCTION_BLOCK

PROGRAM UserCtu
VAR
    d_cu : LINT;
    o_cv : LINT;
    c : CTU;
END_VAR
    c(CU := d_cu, R := FALSE, PV := 100);
    o_cv := c.CV;
END_PROGRAM
"#
    );
    let (unit, errors) = plcc_st::parse(&source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let context = Context::create();
    let mut compiler = Compiler::new(&context, "user_ctu");
    compiler.compile(&unit).expect("codegen failed");

    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT");
    if let Some(f) = compiler.module().get_function("plcc_monotonic_ns") {
        ee.add_global_mapping(&f, fake_monotonic_ns as *const () as usize);
    }
    let mut state = vec![0u8; 4096];
    let ptr = state.as_mut_ptr();
    if let Ok(a) = ee.get_function_address("userctu_init") {
        let f: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(a) };
        f(ptr);
    }
    let a = ee
        .get_function_address("userctu_scan")
        .expect("scan missing");
    let scan: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(a) };

    state[0..8].copy_from_slice(&1i64.to_ne_bytes());
    for _ in 0..3 {
        scan(ptr);
    }
    let mut b = [0u8; 8];
    b.copy_from_slice(&state[8..16]);
    assert_eq!(
        i64::from_ne_bytes(b),
        30,
        "the user's level-counting CTU must be the one that ran, not the prelude's \
         edge-counting one"
    );
}
