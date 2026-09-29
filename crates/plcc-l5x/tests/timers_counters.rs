// SPDX-License-Identifier: MPL-2.0

//! TON / TOF / RTO / CTU / CTD / RES against the flow charts of 1756-RM003
//! "Timer and Counter Instructions", on a fake clock.

mod common;
use common::*;

#[test]
fn ton_times_while_enabled_and_resets_when_false() {
    with_plc("timers_counters.L5X", |plc| {
        // The rung's preset operand initializes T1.PRE (pseudo-operand).
        plc.scan();
        assert_eq!(plc.get("T1.PRE"), 100);
        assert!(!plc.get_bool("T1.EN"));
        plc.set_bool("In1", true);
        plc.scan();
        // First true scan: EN and TT set, timing starts, nothing accumulated.
        assert!(plc.get_bool("T1.EN") && plc.get_bool("T1.TT") && !plc.get_bool("T1.DN"));
        assert_eq!(plc.get("T1.ACC"), 0);
        plc.scans(1, 60);
        assert_eq!(plc.get("T1.ACC"), 60);
        plc.advance(MS / 2);
        plc.scans(1, 30);
        assert_eq!(
            plc.get("T1.ACC"),
            90,
            "whole milliseconds; the remainder carries"
        );
        plc.advance(MS / 2);
        plc.scans(1, 15);
        // 106 >= 100: done, TT cleared. ACC is not clamped to PRE.
        assert!(plc.get_bool("T1.DN") && !plc.get_bool("T1.TT"));
        assert_eq!(plc.get("T1.ACC"), 106);
        plc.scans(1, 50);
        assert_eq!(plc.get("T1.ACC"), 106, "a done timer stops accumulating");
        plc.set_bool("In1", false);
        plc.scan();
        assert!(!plc.get_bool("T1.EN") && !plc.get_bool("T1.TT") && !plc.get_bool("T1.DN"));
        assert_eq!(plc.get("T1.ACC"), 0);
    });
}

#[test]
fn tof_delays_the_falling_edge() {
    with_plc("timers_counters.L5X", |plc| {
        plc.scan();
        // Prescan: TOF status cleared, ACC = PRE.
        assert_eq!(plc.get("T2.ACC"), 50);
        assert!(!plc.get_bool("T2.DN"));
        plc.set_bool("In1", true);
        plc.scan();
        assert!(plc.get_bool("T2.DN") && plc.get_bool("T2.EN") && !plc.get_bool("T2.TT"));
        assert_eq!(plc.get("T2.ACC"), 0);
        plc.set_bool("In1", false);
        plc.scans(1, 10);
        // First false scan: timing starts, DN still set.
        assert!(plc.get_bool("T2.DN") && plc.get_bool("T2.TT") && !plc.get_bool("T2.EN"));
        plc.scans(1, 30);
        assert!(plc.get_bool("T2.DN"));
        assert_eq!(plc.get("T2.ACC"), 30);
        plc.scans(1, 25);
        assert!(!plc.get_bool("T2.DN") && !plc.get_bool("T2.TT"));
        assert_eq!(plc.get("T2.ACC"), 55);
    });
}

#[test]
fn rto_retains_time_until_reset() {
    with_plc("timers_counters.L5X", |plc| {
        plc.set_bool("In1", true);
        plc.scan();
        plc.scans(1, 100);
        assert_eq!(plc.get("T3.ACC"), 100);
        plc.set_bool("In1", false);
        plc.scans(3, 100);
        assert_eq!(plc.get("T3.ACC"), 100, "retained while disabled");
        assert!(!plc.get_bool("T3.EN") && !plc.get_bool("T3.TT"));
        plc.set_bool("In1", true);
        plc.scans(1, 100);
        // Re-enabled: the first true scan restarts the clock without adding.
        assert_eq!(plc.get("T3.ACC"), 100);
        plc.scans(1, 60);
        assert!(plc.get_bool("T3.DN"));
        assert_eq!(plc.get("T3.ACC"), 160);
        plc.set_bool("In1", false);
        plc.scan();
        assert!(plc.get_bool("T3.DN"), "RTO keeps DN when disabled");
        plc.set_bool("ResetT3", true);
        plc.scan();
        assert!(!plc.get_bool("T3.DN"));
        assert_eq!(plc.get("T3.ACC"), 0);
    });
}

#[test]
fn ctu_ctd_count_edges() {
    with_plc("timers_counters.L5X", |plc| {
        // Count input already TRUE on the first scan: prescan sets CU, so the
        // first scan does not count.
        plc.set_bool("CountIn", true);
        plc.scan();
        assert_eq!(plc.get("C1.ACC"), 0);
        assert_eq!(plc.get("C1.PRE"), 3);
        for n in 1..=3 {
            plc.set_bool("CountIn", false);
            plc.scan();
            plc.set_bool("CountIn", true);
            plc.scan();
            plc.scan();
            assert_eq!(plc.get("C1.ACC"), n);
        }
        assert!(plc.get_bool("C1.DN"));
        plc.set_bool("DownIn", true);
        plc.scan();
        assert_eq!(plc.get("C1.ACC"), 2);
        assert!(!plc.get_bool("C1.DN"));
        plc.set_bool("ResC", true);
        plc.scan();
        assert_eq!(plc.get("C1.ACC"), 0);
        // RES cleared CU and CD: with the inputs still TRUE, the next scan
        // counts again (up, then down).
        plc.set_bool("ResC", false);
        plc.scan();
        assert_eq!(plc.get("C1.ACC"), 0);
        assert!(plc.get_bool("C1.CU") && plc.get_bool("C1.CD"));
    });
}

#[test]
fn ctu_overflow_wraps_and_sets_ov() {
    with_plc("timers_counters.L5X", |plc| {
        plc.scan();
        for _ in 0..2 {
            plc.set_bool("C2In", true);
            plc.scan();
            plc.set_bool("C2In", false);
            plc.scan();
        }
        assert_eq!(plc.get("C2.ACC"), i32::MIN as i64);
        assert!(plc.get_bool("C2.OV"));
        // DN is not updated while OV is set.
        assert!(plc.get_bool("C2.DN"));
    });
}
