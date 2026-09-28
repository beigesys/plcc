// SPDX-License-Identifier: MPL-2.0

//! Ladder Diagram bodies: lowered, compiled, JIT-executed over several scans.

mod common;
use common::*;

#[test]
fn seal_in_latches_and_stops() {
    with_plc("ld_seal_in.xml", |plc| {
        assert_eq!(plc.task_name(0), "MainTask");
        assert_eq!(plc.tasks()[0].interval_ns, 20 * MS);
        let start = |v: bool| plc.set_bool("SealIn.Start", v);
        let stop = |v: bool| plc.set_bool("SealIn.Stop", v);

        plc.scan();
        assert!(!plc.get_bool("SealIn.Motor"));
        start(true);
        plc.scan();
        assert!(plc.get_bool("SealIn.Motor"), "Start energizes the coil");
        assert_eq!(plc.output_byte(0) & 1, 1, "%QX0.0 is the process image bit");
        start(false);
        plc.scan();
        plc.scan();
        assert!(
            plc.get_bool("SealIn.Motor"),
            "the Motor contact seals it in"
        );
        stop(true);
        plc.scan();
        assert!(!plc.get_bool("SealIn.Motor"), "Stop breaks the seal");
        stop(false);
        plc.scan();
        assert!(!plc.get_bool("SealIn.Motor"), "and it stays off");
    });
}

#[test]
fn seal_in_lowers_to_one_assignment() {
    let unit = lower("ld_seal_in.xml");
    let plcc_st::Declaration::Program(p) = &unit.declarations[0] else {
        panic!("expected a program")
    };
    assert_eq!(p.body.len(), 1, "{:#?}", p.body);
    // The statement points at the coil element in the XML.
    let src = read_fixture("ld_seal_in.xml");
    let s = p.body[0].span;
    assert!(src[s.start..s.end].starts_with("<coil localId=\"5\""));
}

#[test]
fn lowered_ladder_type_checks() {
    // (Fixtures without standard FBs: `plcc check` does not load the stdlib.)
    for f in ["ld_seal_in.xml", "ld_branches.xml", "ld_jumps.xml"] {
        let errs = check_messages(f);
        assert!(errs.is_empty(), "{f}: {errs:#?}");
    }
}

#[test]
fn ton_tof_and_ctu_blocks() {
    with_plc("ld_timer_counter.xml", |plc| {
        assert_eq!(plc.tasks()[0].interval_ns, 10 * MS, "PT0.01S");
        let b = |n: &str| plc.get_bool(&format!("machine.{n}"));

        plc.set_bool("machine.Enable", true);
        plc.scan();
        assert!(!b("Done"));
        assert!(b("Lamp"), "TOF output follows IN at once");
        plc.advance(60 * MS);
        plc.scan();
        assert!(!b("Done"));
        assert_eq!(plc.get("machine.Elapsed"), 60 * MS, "ET via outVariable");
        plc.advance(50 * MS);
        plc.scan();
        assert!(b("Done"), "TON Q after PT = 100 ms");

        plc.set_bool("machine.Enable", false);
        plc.scan();
        assert!(!b("Done"));
        assert!(b("Lamp"), "TOF holds for PT");
        plc.advance(60 * MS);
        plc.scan();
        assert!(!b("Lamp"), "TOF off after 50 ms");

        // Counter: counts rising edges of Pulse, only while EN (CountEn) is on.
        let pulse = |v: bool| {
            plc.set_bool("machine.Pulse", v);
            plc.scan();
        };
        pulse(true);
        pulse(false);
        assert_eq!(plc.get("machine.Count"), 0, "EN off: the CTU is not called");
        assert!(!b("Enabled"), "ENO follows EN");
        plc.set_bool("machine.CountEn", true);
        for _ in 0..2 {
            pulse(true);
            pulse(false);
        }
        assert!(b("Enabled"));
        assert_eq!(plc.get("machine.Count"), 2);
        assert!(!b("Reached"));
        pulse(true);
        assert_eq!(plc.get("machine.Count"), 3);
        assert!(b("Reached"), "Q at PV = 3");
        plc.set_bool("machine.Reset", true);
        plc.scan();
        assert_eq!(plc.get("machine.Count"), 0);
        assert!(!b("Reached"));
    });
}

#[test]
fn set_reset_negated_and_edge_coils() {
    with_plc("ld_coils_edges.xml", |plc| {
        let b = |n: &str| plc.get_bool(&format!("Coils.{n}"));
        let set = |n: &str, v: bool| plc.set_bool(&format!("Coils.{n}"), v);

        plc.scan();
        assert!(!b("Latch"));
        assert!(b("NotLatch"), "negated coil");
        set("SetBtn", true);
        plc.scan();
        set("SetBtn", false);
        plc.scan();
        assert!(b("Latch"), "set coil holds");
        assert!(!b("NotLatch"));
        set("ResetBtn", true);
        plc.scan();
        assert!(!b("Latch"), "reset coil clears");
        set("ResetBtn", false);

        // Edges: In goes high for three scans, then low for two.
        let mut trace = Vec::new();
        for v in [true, true, true, false, false] {
            set("In", v);
            plc.scan();
            trace.push((
                b("RisePulse"),
                b("FallPulse"),
                b("PulseCoil"),
                b("DropCoil"),
            ));
        }
        assert_eq!(
            trace,
            [
                (true, false, true, false),
                (false, false, false, false),
                (false, false, false, false),
                (false, true, false, true),
                (false, false, false, false),
            ]
        );
        assert_eq!(
            plc.get("Coils.Rises"),
            1,
            "ADD with EN runs only on the pulse"
        );
        set("In", true);
        plc.scan();
        assert_eq!(plc.get("Coils.Rises"), 2);
    });
}

#[test]
fn parallel_branches_fanout_and_connectors() {
    with_plc("ld_branches.xml", |plc| {
        let set = |n: &str, v: bool| plc.set_bool(&format!("Main.br.{n}"), v);
        let b = |n: &str| plc.get_bool(&format!("Main.br.{n}"));
        for bits in 0..16u8 {
            let (a, bb, c, d) = (bits & 1 != 0, bits & 2 != 0, bits & 4 != 0, bits & 8 != 0);
            set("A", a);
            set("B", bb);
            set("C", c);
            set("D", d);
            plc.scan();
            assert_eq!(
                b("Out1"),
                (a && bb) || (c && !d),
                "A={a} B={bb} C={c} D={d}"
            );
        }
        for bits in 0..4u8 {
            let (e, f) = (bits & 1 != 0, bits & 2 != 0);
            set("E", e);
            set("F", f);
            plc.scan();
            assert_eq!(b("X"), e);
            assert_eq!(b("Z"), e, "coil in series passes power");
            assert_eq!(b("Y"), e && f);
        }
        for bits in 0..4u8 {
            let (g, h) = (bits & 1 != 0, bits & 2 != 0);
            set("G", g);
            set("H", h);
            plc.scan();
            assert_eq!(b("W"), g && h, "connector/continuation carry power");
        }
    });
}

#[test]
fn jumps_labels_and_return() {
    with_plc("ld_jumps.xml", |plc| {
        let g = |n: &str| plc.get(&format!("Jumps.{n}"));
        plc.scan();
        assert_eq!(g("Count"), 1);
        assert_eq!(g("Always"), 1);
        assert_eq!(g("Loops"), 5, "backward jump loops until Loops >= Limit");
        assert_eq!(g("AfterReturn"), 1);
        plc.scan();
        assert_eq!(g("Count"), 2);
        assert_eq!(g("Loops"), 6, "no jump once Loops >= Limit");

        plc.set_bool("Jumps.Skip", true);
        plc.scan();
        plc.scan();
        assert_eq!(g("Count"), 2, "jump skips the ADD rung");
        assert_eq!(g("Always"), 1, "rungs after the label still run");

        plc.set_bool("Jumps.Quit", true);
        plc.set_bool("Jumps.AfterReturn", false);
        plc.scan();
        assert_eq!(g("AfterReturn"), 0, "RETURN ends the scan early");
    });
}
