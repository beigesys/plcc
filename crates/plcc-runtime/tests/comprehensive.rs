// SPDX-License-Identifier: MPL-2.0

//! Comprehensive tests for all standard FBs and functions in plcc-runtime.

use plcc_runtime::fb::*;
use plcc_runtime::functions::*;
use plcc_runtime::traits::FunctionBlock;

const EPSILON: f64 = 1e-6;

// ═══════════════════════════════════════════════════════════════════════════
// Function Block Tests
// ═══════════════════════════════════════════════════════════════════════════

// ── 1. SR flip-flop: set-dominant behavior ──

#[test]
fn sr_set_then_latch() {
    let mut sr = SR::default();
    sr.set1 = true;
    sr.reset = false;
    sr.scan();
    assert!(sr.q1, "SR: set1=true should set Q1");

    // Latch: set goes false, output stays
    sr.set1 = false;
    sr.scan();
    assert!(sr.q1, "SR: Q1 should remain latched after set1 goes false");
}

#[test]
fn sr_reset_clears() {
    let mut sr = SR::default();
    sr.set1 = true;
    sr.scan();
    assert!(sr.q1);

    sr.set1 = false;
    sr.reset = true;
    sr.scan();
    assert!(!sr.q1, "SR: reset should clear Q1");
}

#[test]
fn sr_set_dominant_simultaneous() {
    let mut sr = SR::default();
    sr.set1 = true;
    sr.reset = true;
    sr.scan();
    assert!(
        sr.q1,
        "SR: set+reset simultaneously => Q1 stays true (set-dominant)"
    );
}

#[test]
fn sr_set_dominant_when_already_set() {
    let mut sr = SR::default();
    // First set it
    sr.set1 = true;
    sr.scan();
    assert!(sr.q1);

    // Now both true simultaneously
    sr.set1 = true;
    sr.reset = true;
    sr.scan();
    assert!(sr.q1, "SR: set+reset while already set => Q1 stays true");
}

// ── 2. RS flip-flop: reset-dominant behavior ──

#[test]
fn rs_set_then_latch() {
    let mut rs = RS::default();
    rs.set = true;
    rs.reset1 = false;
    rs.scan();
    assert!(rs.q1, "RS: set=true should set Q1");

    rs.set = false;
    rs.scan();
    assert!(rs.q1, "RS: Q1 should remain latched");
}

#[test]
fn rs_reset_dominant_simultaneous() {
    let mut rs = RS::default();
    rs.set = true;
    rs.reset1 = true;
    rs.scan();
    assert!(
        !rs.q1,
        "RS: set+reset simultaneously => Q1 false (reset-dominant)"
    );
}

#[test]
fn rs_reset_dominant_when_already_set() {
    let mut rs = RS::default();
    rs.set = true;
    rs.scan();
    assert!(rs.q1);

    rs.set = true;
    rs.reset1 = true;
    rs.scan();
    assert!(!rs.q1, "RS: set+reset while already set => Q1 goes false");
}

// ── 3. R_TRIG: rising edge detector ──

#[test]
fn r_trig_full_sequence() {
    let mut rt = RTrig::default();

    // LOW -> no pulse
    rt.clk = false;
    rt.scan();
    assert!(!rt.q, "R_TRIG: initial LOW => no pulse");

    // LOW -> HIGH => pulse
    rt.clk = true;
    rt.scan();
    assert!(rt.q, "R_TRIG: LOW->HIGH => pulse");

    // HIGH -> HIGH => no pulse
    rt.clk = true;
    rt.scan();
    assert!(!rt.q, "R_TRIG: HIGH->HIGH => no pulse");

    // HIGH -> LOW => no pulse
    rt.clk = false;
    rt.scan();
    assert!(!rt.q, "R_TRIG: HIGH->LOW => no pulse");

    // LOW -> HIGH => pulse again
    rt.clk = true;
    rt.scan();
    assert!(rt.q, "R_TRIG: LOW->HIGH again => pulse");
}

// ── 4. F_TRIG: falling edge detector ──

#[test]
fn f_trig_full_sequence() {
    let mut ft = FTrig::default();

    // Start HIGH
    ft.clk = true;
    ft.scan();
    assert!(
        !ft.q,
        "F_TRIG: initial HIGH => no pulse (prev was false, but clk is true)"
    );

    // HIGH -> LOW => pulse
    ft.clk = false;
    ft.scan();
    assert!(ft.q, "F_TRIG: HIGH->LOW => pulse");

    // LOW -> LOW => no pulse
    ft.clk = false;
    ft.scan();
    assert!(!ft.q, "F_TRIG: LOW->LOW => no pulse");

    // LOW -> HIGH => no pulse
    ft.clk = true;
    ft.scan();
    assert!(!ft.q, "F_TRIG: LOW->HIGH => no pulse");

    // HIGH -> LOW => pulse again
    ft.clk = false;
    ft.scan();
    assert!(ft.q, "F_TRIG: HIGH->LOW again => pulse");
}

// ── 5. CTU: count up ──

#[test]
fn ctu_counts_on_rising_edges_only() {
    let mut ctu = CTU::default();
    ctu.pv = 5;

    // Pulse CU: rising edge increments
    for i in 0..5 {
        ctu.cu = true;
        ctu.scan();
        assert_eq!(
            ctu.cv,
            i + 1,
            "CTU: CV should be {} after pulse {}",
            i + 1,
            i + 1
        );

        // Holding CU high should NOT increment again
        ctu.scan();
        assert_eq!(
            ctu.cv,
            i + 1,
            "CTU: CV should not increment while CU held high"
        );

        ctu.cu = false;
        ctu.scan();
    }
}

#[test]
fn ctu_q_goes_true_exactly_at_pv() {
    let mut ctu = CTU::default();
    ctu.pv = 3;

    // Count to PV-1, Q should be false
    for _ in 0..2 {
        ctu.cu = true;
        ctu.scan();
        ctu.cu = false;
        ctu.scan();
    }
    assert_eq!(ctu.cv, 2);
    assert!(!ctu.q, "CTU: Q should be false when CV < PV");

    // Count to PV, Q should go true
    ctu.cu = true;
    ctu.scan();
    assert_eq!(ctu.cv, 3);
    assert!(ctu.q, "CTU: Q should be true when CV >= PV");
}

#[test]
fn ctu_reset_clears_count() {
    let mut ctu = CTU::default();
    ctu.pv = 10;

    // Count a few
    for _ in 0..5 {
        ctu.cu = true;
        ctu.scan();
        ctu.cu = false;
        ctu.scan();
    }
    assert_eq!(ctu.cv, 5);

    // Reset
    ctu.reset = true;
    ctu.scan();
    assert_eq!(ctu.cv, 0, "CTU: reset should zero CV");
    assert!(!ctu.q, "CTU: reset should clear Q");
}

// ── 6. CTD: count down ──

#[test]
fn ctd_counts_down_from_pv() {
    let mut ctd = CTD::default();
    ctd.pv = 3;

    // Load PV
    ctd.load = true;
    ctd.scan();
    assert_eq!(ctd.cv, 3, "CTD: load should set CV to PV");
    assert!(!ctd.q, "CTD: Q should be false when CV > 0");

    ctd.load = false;

    // Count down to 0
    for i in 0..3 {
        ctd.cd = true;
        ctd.scan();
        assert_eq!(
            ctd.cv,
            2 - i as i32,
            "CTD: CV should decrement on rising edge"
        );
        ctd.cd = false;
        ctd.scan();
    }

    assert_eq!(ctd.cv, 0);
    assert!(ctd.q, "CTD: Q should be true when CV <= 0");
}

#[test]
fn ctd_q_true_at_zero() {
    let mut ctd = CTD::default();
    ctd.pv = 1;
    ctd.load = true;
    ctd.scan();
    ctd.load = false;

    assert_eq!(ctd.cv, 1);
    assert!(!ctd.q);

    ctd.cd = true;
    ctd.scan();
    assert_eq!(ctd.cv, 0);
    assert!(ctd.q, "CTD: Q should go true exactly at CV=0");
}

// ── 7. CTUD: count up/down ──

#[test]
fn ctud_counts_up_and_down_independently() {
    let mut ctud = CTUD::default();
    ctud.pv = 5;

    // Count up 3 times
    for _ in 0..3 {
        ctud.cu = true;
        ctud.scan();
        ctud.cu = false;
        ctud.scan();
    }
    assert_eq!(ctud.cv, 3);
    assert!(!ctud.qu, "CTUD: QU should be false when CV < PV");
    assert!(!ctud.qd, "CTUD: QD should be false when CV > 0");

    // Count down 1
    ctud.cd = true;
    ctud.scan();
    ctud.cd = false;
    ctud.scan();
    assert_eq!(ctud.cv, 2);

    // Count up to PV
    for _ in 0..3 {
        ctud.cu = true;
        ctud.scan();
        ctud.cu = false;
        ctud.scan();
    }
    assert_eq!(ctud.cv, 5);
    assert!(ctud.qu, "CTUD: QU should be true when CV >= PV");
    assert!(!ctud.qd);
}

#[test]
fn ctud_qd_at_zero() {
    let mut ctud = CTUD::default();
    ctud.pv = 2;

    // CV starts at 0, which is <= 0
    ctud.scan();
    assert!(ctud.qd, "CTUD: QD should be true when CV <= 0");

    // Count up once
    ctud.cu = true;
    ctud.scan();
    ctud.cu = false;
    ctud.scan();
    assert_eq!(ctud.cv, 1);
    assert!(!ctud.qd, "CTUD: QD should be false when CV > 0");
}

#[test]
fn ctud_reset_and_load() {
    let mut ctud = CTUD::default();
    ctud.pv = 10;

    // Load sets CV to PV
    ctud.load = true;
    ctud.scan();
    assert_eq!(ctud.cv, 10);
    ctud.load = false;

    // Reset zeroes CV
    ctud.reset = true;
    ctud.scan();
    assert_eq!(ctud.cv, 0);
}

// ── 8. TON: on-delay timer ──

#[test]
fn ton_delays_output() {
    let mut ton = TON::default();
    ton.pt_ms = 100;
    ton.input = true;

    // Q stays false before PT elapsed
    for _ in 0..99 {
        ton.scan_with_time(1);
        assert!(!ton.q, "TON: Q should be false before PT elapsed");
    }

    // Q goes true at exactly PT
    ton.scan_with_time(1);
    assert!(ton.q, "TON: Q should go true once PT elapsed");
    assert_eq!(ton.et_ms, 100);
}

#[test]
fn ton_input_false_resets() {
    let mut ton = TON::default();
    ton.pt_ms = 100;
    ton.input = true;

    // Partially elapsed
    for _ in 0..50 {
        ton.scan_with_time(1);
    }
    assert!(!ton.q);
    assert_eq!(ton.et_ms, 50);

    // Input goes false => reset
    ton.input = false;
    ton.scan_with_time(1);
    assert!(!ton.q, "TON: Q should be false after input goes false");
    assert_eq!(ton.et_ms, 0, "TON: ET should reset to 0");
}

// ── 9. TOF: off-delay timer ──

#[test]
fn tof_delays_off() {
    let mut tof = TOF::default();
    tof.pt_ms = 50;

    // Input true => Q immediately true
    tof.input = true;
    tof.scan_with_time(1);
    assert!(tof.q, "TOF: Q should be true when input is true");

    // Input goes false => Q stays true for PT ms
    tof.input = false;
    for i in 1..50 {
        tof.scan_with_time(1);
        assert!(
            tof.q,
            "TOF: Q should stay true during off-delay (tick {})",
            i
        );
    }

    // After PT elapsed => Q goes false
    tof.scan_with_time(1);
    assert!(!tof.q, "TOF: Q should go false after PT elapsed");
}

#[test]
fn tof_input_true_resets_timer() {
    let mut tof = TOF::default();
    tof.pt_ms = 100;

    tof.input = true;
    tof.scan_with_time(1);
    assert!(tof.q);

    // Start off-delay
    tof.input = false;
    for _ in 0..40 {
        tof.scan_with_time(1);
    }
    assert!(tof.q, "TOF: Q should still be true mid-delay");

    // Input goes true again => resets
    tof.input = true;
    tof.scan_with_time(1);
    assert!(tof.q);
    assert_eq!(tof.et_ms, 0, "TOF: ET should reset when input goes true");
}

// ── 10. TP: pulse timer ──

#[test]
fn tp_pulse_duration() {
    let mut tp = TP::default();
    tp.pt_ms = 50;

    // Rising edge starts pulse
    tp.input = true;
    tp.scan_with_time(1);
    assert!(tp.q, "TP: Q should be true on rising edge");

    // Pulse stays for PT ms
    for i in 2..50 {
        tp.scan_with_time(1);
        assert!(tp.q, "TP: Q should stay true during pulse (tick {})", i);
    }

    // Pulse ends after PT
    tp.scan_with_time(1);
    assert!(!tp.q, "TP: Q should go false after PT elapsed");
}

#[test]
fn tp_ignores_edges_during_pulse() {
    let mut tp = TP::default();
    tp.pt_ms = 100;

    // Start pulse
    tp.input = true;
    tp.scan_with_time(1);
    assert!(tp.q);

    // Toggle input during pulse -- should be ignored
    tp.input = false;
    tp.scan_with_time(1);
    assert!(
        tp.q,
        "TP: pulse should continue regardless of input changes"
    );

    tp.input = true;
    tp.scan_with_time(1);
    assert!(tp.q, "TP: rising edge during pulse should be ignored");
}

// ── 11. RTC: real-time clock ──

#[test]
fn rtc_enabled() {
    let mut rtc = RTC::default();
    rtc.en = true;
    rtc.pdt = 1647820800; // some timestamp
    rtc.scan();
    assert!(rtc.q, "RTC: Q should be true when EN is true");
    assert_eq!(rtc.cdt, rtc.pdt, "RTC: CDT should equal PDT when enabled");
}

#[test]
fn rtc_disabled() {
    let mut rtc = RTC::default();
    rtc.en = false;
    rtc.pdt = 1647820800;
    rtc.scan();
    assert!(!rtc.q, "RTC: Q should be false when EN is false");
}

// ── 12. SR reset behavior (trait reset) ──

#[test]
fn sr_trait_reset_zeroes_state() {
    let mut sr = SR::default();
    sr.set1 = true;
    sr.scan();
    assert!(sr.q1);

    FunctionBlock::reset(&mut sr);
    assert!(!sr.q1, "SR: reset() should zero Q1");
    assert!(!sr.set1, "SR: reset() should zero set1");
    assert!(!sr.reset, "SR: reset() should zero reset");
}

// ── 13. CTU reset during count ──

#[test]
fn ctu_trait_reset_mid_count() {
    let mut ctu = CTU::default();
    ctu.pv = 10;

    // Count to 7
    for _ in 0..7 {
        ctu.cu = true;
        ctu.scan();
        ctu.cu = false;
        ctu.scan();
    }
    assert_eq!(ctu.cv, 7);

    // Trait reset
    FunctionBlock::reset(&mut ctu);
    assert_eq!(ctu.cv, 0, "CTU: trait reset should zero CV");
    assert!(!ctu.q, "CTU: trait reset should clear Q");
    assert_eq!(ctu.pv, 0, "CTU: trait reset should zero PV (full default)");
}

// ── 14. TON multiple on/off cycles ──

#[test]
fn ton_multiple_cycles() {
    let mut ton = TON::default();
    ton.pt_ms = 10;

    // Cycle 1: input on, wait for Q
    ton.input = true;
    for _ in 0..10 {
        ton.scan_with_time(1);
    }
    assert!(ton.q, "TON cycle 1: Q should be true after PT");

    // Input off => reset
    ton.input = false;
    ton.scan_with_time(1);
    assert!(!ton.q, "TON: Q should reset when input goes false");

    // Cycle 2: input on again, timer should restart from 0
    ton.input = true;
    for _ in 0..5 {
        ton.scan_with_time(1);
    }
    assert!(!ton.q, "TON cycle 2: Q should be false before PT");

    for _ in 0..5 {
        ton.scan_with_time(1);
    }
    assert!(
        ton.q,
        "TON cycle 2: Q should be true after PT elapsed again"
    );

    // Cycle 3: partial, then off
    ton.input = false;
    ton.scan_with_time(1);
    ton.input = true;
    for _ in 0..3 {
        ton.scan_with_time(1);
    }
    assert!(
        !ton.q,
        "TON cycle 3: partial timing, Q should still be false"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Standard Function Tests
// ═══════════════════════════════════════════════════════════════════════════

// ── 15. abs_real ──

#[test]
fn test_abs_real() {
    // Deliberately not 3.14 / 2.71 — clippy's approx_constant lint reads those
    // as botched PI and E. The values here are arbitrary; abs is what's tested.
    assert!((abs_real(3.5) - 3.5).abs() < EPSILON);
    assert!((abs_real(-2.25) - 2.25).abs() < EPSILON);
    assert!((abs_real(0.0)).abs() < EPSILON);
    assert!((abs_real(-0.0)).abs() < EPSILON);
}

// ── 16. abs_int ──

#[test]
fn test_abs_int() {
    assert_eq!(abs_int(42), 42);
    assert_eq!(abs_int(-42), 42);
    assert_eq!(abs_int(0), 0);
    assert_eq!(abs_int(1), 1);
    assert_eq!(abs_int(-1), 1);
}

// ── 17. sqrt ──

#[test]
fn test_sqrt() {
    assert!((sqrt(0.0)).abs() < EPSILON);
    assert!((sqrt(1.0) - 1.0).abs() < EPSILON);
    assert!((sqrt(4.0) - 2.0).abs() < EPSILON);
    assert!((sqrt(9.0) - 3.0).abs() < EPSILON);
    assert!((sqrt(16.0) - 4.0).abs() < EPSILON);
    assert!((sqrt(100.0) - 10.0).abs() < EPSILON);
}

// ── 18. ln, log, exp ──

#[test]
fn test_ln() {
    assert!(
        (ln(std::f64::consts::E) - 1.0).abs() < EPSILON,
        "ln(e) should be 1"
    );
    assert!((ln(1.0)).abs() < EPSILON, "ln(1) should be 0");
}

#[test]
fn test_log10() {
    assert!((log(10.0) - 1.0).abs() < EPSILON, "log(10) should be 1");
    assert!((log(100.0) - 2.0).abs() < EPSILON, "log(100) should be 2");
    assert!((log(1.0)).abs() < EPSILON, "log(1) should be 0");
}

#[test]
fn test_exp() {
    assert!((exp(0.0) - 1.0).abs() < EPSILON, "exp(0) should be 1");
    assert!(
        (exp(1.0) - std::f64::consts::E).abs() < EPSILON,
        "exp(1) should be e"
    );
    assert!((exp(2.0) - std::f64::consts::E * std::f64::consts::E).abs() < EPSILON);
}

// ── 19. trig functions ──

#[test]
fn test_sin() {
    assert!((sin(0.0)).abs() < EPSILON, "sin(0) should be 0");
    assert!(
        (sin(std::f64::consts::FRAC_PI_2) - 1.0).abs() < EPSILON,
        "sin(pi/2) should be 1"
    );
    assert!(
        (sin(std::f64::consts::PI)).abs() < EPSILON,
        "sin(pi) should be ~0"
    );
}

#[test]
fn test_cos() {
    assert!((cos(0.0) - 1.0).abs() < EPSILON, "cos(0) should be 1");
    assert!(
        (cos(std::f64::consts::PI) - (-1.0)).abs() < EPSILON,
        "cos(pi) should be -1"
    );
    assert!(
        (cos(std::f64::consts::FRAC_PI_2)).abs() < EPSILON,
        "cos(pi/2) should be ~0"
    );
}

#[test]
fn test_tan() {
    assert!((tan(0.0)).abs() < EPSILON, "tan(0) should be 0");
    assert!(
        (tan(std::f64::consts::FRAC_PI_4) - 1.0).abs() < EPSILON,
        "tan(pi/4) should be 1"
    );
}

#[test]
fn test_asin_acos_atan() {
    assert!((asin(0.0)).abs() < EPSILON, "asin(0) should be 0");
    assert!(
        (asin(1.0) - std::f64::consts::FRAC_PI_2).abs() < EPSILON,
        "asin(1) should be pi/2"
    );
    assert!((acos(1.0)).abs() < EPSILON, "acos(1) should be 0");
    assert!((atan(0.0)).abs() < EPSILON, "atan(0) should be 0");
    assert!(
        (atan(1.0) - std::f64::consts::FRAC_PI_4).abs() < EPSILON,
        "atan(1) should be pi/4"
    );
}

// ── 20. atan2 ──

#[test]
fn test_atan2_quadrants() {
    assert!(
        (atan2(1.0, 1.0) - std::f64::consts::FRAC_PI_4).abs() < EPSILON,
        "atan2(1,1) should be pi/4"
    );
    assert!(
        (atan2(-1.0, 1.0) - (-std::f64::consts::FRAC_PI_4)).abs() < EPSILON,
        "atan2(-1,1) should be -pi/4"
    );
    assert!((atan2(0.0, 1.0)).abs() < EPSILON, "atan2(0,1) should be 0");
    assert!(
        (atan2(1.0, 0.0) - std::f64::consts::FRAC_PI_2).abs() < EPSILON,
        "atan2(1,0) should be pi/2"
    );
}

// ── 21. expt ──

#[test]
fn test_expt() {
    assert!(
        (expt(2.0, 10.0) - 1024.0).abs() < EPSILON,
        "2^10 should be 1024"
    );
    assert!((expt(3.0, 3.0) - 27.0).abs() < EPSILON, "3^3 should be 27");
    assert!((expt(5.0, 0.0) - 1.0).abs() < EPSILON, "x^0 should be 1");
    assert!((expt(10.0, 1.0) - 10.0).abs() < EPSILON, "x^1 should be x");
}

// ── 22. sel ──

#[test]
fn test_sel() {
    assert_eq!(sel(false, 10, 20), 10, "sel(false) should return in0");
    assert_eq!(sel(true, 10, 20), 20, "sel(true) should return in1");
    assert_eq!(sel(false, -5, 5), -5);
    assert_eq!(sel(true, -5, 5), 5);

    // Float variant
    let r = sel(true, 1.0_f64, 2.0_f64);
    assert!((r - 2.0).abs() < EPSILON);
}

// ── 23. max/min ──

#[test]
fn test_max_min_real() {
    assert!((max_real(3.0, 5.0) - 5.0).abs() < EPSILON);
    assert!((max_real(-1.0, -3.0) - (-1.0)).abs() < EPSILON);
    assert!(
        (max_real(7.0, 7.0) - 7.0).abs() < EPSILON,
        "max of equal values"
    );

    assert!((min_real(3.0, 5.0) - 3.0).abs() < EPSILON);
    assert!((min_real(-1.0, -3.0) - (-3.0)).abs() < EPSILON);
    assert!(
        (min_real(7.0, 7.0) - 7.0).abs() < EPSILON,
        "min of equal values"
    );
}

#[test]
fn test_max_min_int() {
    assert_eq!(max_int(3, 5), 5);
    assert_eq!(max_int(-1, -3), -1);
    assert_eq!(max_int(7, 7), 7);
    assert_eq!(max_int(0, -100), 0);

    assert_eq!(min_int(3, 5), 3);
    assert_eq!(min_int(-1, -3), -3);
    assert_eq!(min_int(7, 7), 7);
    assert_eq!(min_int(0, -100), -100);
}

// ── 24. limit ──

#[test]
fn test_limit_real() {
    // Below min
    assert!(
        (limit_real(0.0, -5.0, 100.0) - 0.0).abs() < EPSILON,
        "limit_real: below min should clamp to min"
    );
    // In range
    assert!(
        (limit_real(0.0, 50.0, 100.0) - 50.0).abs() < EPSILON,
        "limit_real: in range should pass through"
    );
    // Above max
    assert!(
        (limit_real(0.0, 150.0, 100.0) - 100.0).abs() < EPSILON,
        "limit_real: above max should clamp to max"
    );
    // Exactly at boundaries
    assert!((limit_real(0.0, 0.0, 100.0) - 0.0).abs() < EPSILON);
    assert!((limit_real(0.0, 100.0, 100.0) - 100.0).abs() < EPSILON);
}

#[test]
fn test_limit_int() {
    assert_eq!(limit_int(0, -5, 100), 0, "limit_int: below min");
    assert_eq!(limit_int(0, 50, 100), 50, "limit_int: in range");
    assert_eq!(limit_int(0, 150, 100), 100, "limit_int: above max");
    assert_eq!(limit_int(-10, -10, 10), -10, "limit_int: at min boundary");
    assert_eq!(limit_int(-10, 10, 10), 10, "limit_int: at max boundary");
}

// ── 25. type conversions ──

#[test]
fn test_bool_to_int() {
    assert_eq!(bool_to_int(true), 1);
    assert_eq!(bool_to_int(false), 0);
}

#[test]
fn test_int_to_real() {
    assert!((int_to_real(42) - 42.0).abs() < EPSILON);
    assert!((int_to_real(-10) - (-10.0)).abs() < EPSILON);
    assert!((int_to_real(0)).abs() < EPSILON);
}

#[test]
fn test_real_to_int_truncation() {
    assert_eq!(
        real_to_int(3.7),
        3,
        "real_to_int should truncate toward zero"
    );
    assert_eq!(real_to_int(3.0), 3);
    assert_eq!(
        real_to_int(-2.3),
        -2,
        "real_to_int should truncate toward zero for negatives"
    );
    assert_eq!(real_to_int(-2.9), -2);
    assert_eq!(real_to_int(0.0), 0);
}

#[test]
fn test_dint_to_real() {
    assert!((dint_to_real(1_000_000i64) - 1_000_000.0).abs() < EPSILON);
    assert!((dint_to_real(-42i64) - (-42.0)).abs() < EPSILON);
    assert!((dint_to_real(0i64)).abs() < EPSILON);
}

#[test]
fn test_real_to_dint() {
    assert_eq!(real_to_dint(99.9), 99);
    assert_eq!(real_to_dint(-3.1), -3);
    assert_eq!(real_to_dint(0.0), 0);
}

// ── 26. trunc ──

#[test]
fn test_trunc() {
    assert!(
        (trunc(3.7) - 3.0).abs() < EPSILON,
        "trunc(3.7) should be 3.0"
    );
    assert!(
        (trunc(-2.3) - (-2.0)).abs() < EPSILON,
        "trunc(-2.3) should be -2.0"
    );
    assert!((trunc(0.0)).abs() < EPSILON);
    assert!(
        (trunc(5.0) - 5.0).abs() < EPSILON,
        "trunc of integer value is unchanged"
    );
    assert!(
        (trunc(-0.9) - (0.0)).abs() < EPSILON,
        "trunc(-0.9) should be 0.0"
    );
}

// ── 27. bit string: shl, shr, rol, ror ──

#[test]
fn test_shl_dword() {
    assert_eq!(shl_dword(1, 4), 16);
    assert_eq!(shl_dword(1, 0), 1);
    assert_eq!(shl_dword(1, 31), 0x8000_0000);
    assert_eq!(shl_dword(0xFF, 8), 0xFF00);
}

#[test]
fn test_shr_dword() {
    assert_eq!(shr_dword(16, 4), 1);
    assert_eq!(shr_dword(1, 0), 1);
    assert_eq!(shr_dword(0x8000_0000, 31), 1);
    assert_eq!(shr_dword(0xFF00, 8), 0xFF);
}

#[test]
fn test_rol_dword_wraparound() {
    // MSB wraps around to LSB
    assert_eq!(rol_dword(0x8000_0000, 1), 1);
    // Full rotation returns original
    assert_eq!(rol_dword(0xDEAD_BEEF, 32), 0xDEAD_BEEF);
    // Partial rotation
    assert_eq!(rol_dword(0x0000_0001, 4), 0x0000_0010);
}

#[test]
fn test_ror_dword_wraparound() {
    // LSB wraps around to MSB
    assert_eq!(ror_dword(1, 1), 0x8000_0000);
    // Full rotation returns original
    assert_eq!(ror_dword(0xDEAD_BEEF, 32), 0xDEAD_BEEF);
    // Partial rotation
    assert_eq!(ror_dword(0x0000_0010, 4), 0x0000_0001);
}

// ── 28. shr on large values ──

#[test]
fn test_shr_dword_large_values() {
    // shr_dword is unsigned (u32), so no sign extension
    assert_eq!(shr_dword(0xFFFF_FFFF, 1), 0x7FFF_FFFF);
    assert_eq!(shr_dword(0xFFFF_FFFF, 16), 0x0000_FFFF);
    assert_eq!(shr_dword(0xFFFF_FFFF, 31), 1);
    assert_eq!(shr_dword(0xF000_0000, 4), 0x0F00_0000);
}
