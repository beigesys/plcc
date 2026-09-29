// SPDX-License-Identifier: MPL-2.0

//! Ladder (RLL) routines from L5X fixtures, JIT-run scan by scan through the
//! runtime contract. Expected behaviour is the Logix one (1756-RM003), not
//! IEC's: see docs/l5x.md.

mod common;
use common::*;

#[test]
fn one_shots_fire_once_per_edge() {
    with_plc("bits_branches.L5X", |plc| {
        // Input already TRUE on the first scan: the prescan set the ONS and
        // OSR storage bits, so neither fires (1756-RM003 ONS/OSR "Prescan").
        plc.set_bool("In", true);
        plc.scan();
        assert_eq!(plc.get("Pulses"), 0);
        assert!(!plc.get_bool("OsrOut"));
        plc.set_bool("In", false);
        plc.scan();
        assert!(
            !plc.get_bool("OnsBit"),
            "a false rung clears the ONS storage bit"
        );
        assert!(plc.get_bool("OsfOut"), "OSF fires on the falling edge");
        assert_eq!(plc.get("Falls"), 1);
        plc.scan();
        assert!(!plc.get_bool("OsfOut"));
        assert_eq!(plc.get("Falls"), 1);
        plc.set_bool("In", true);
        plc.scan();
        assert_eq!(plc.get("Pulses"), 1);
        assert!(plc.get_bool("OsrOut"));
        plc.scan();
        plc.scan();
        assert_eq!(plc.get("Pulses"), 1);
        assert!(!plc.get_bool("OsrOut"));
    });
}

#[test]
fn latches_nested_branches_and_compares() {
    with_plc("bits_branches.L5X", |plc| {
        plc.set_bool("Set", true);
        plc.scan();
        assert!(plc.get_bool("Latched"));
        plc.set_bool("Set", false);
        plc.scan();
        assert!(plc.get_bool("Latched"), "OTL holds on a false rung");
        plc.set_bool("Reset", true);
        plc.scan();
        assert!(!plc.get_bool("Latched"));

        // (A AND (B OR C)) OR D, then AND NOT C.
        for (a, b, c, d) in [
            (false, false, false, false),
            (true, false, false, false),
            (true, true, false, false),
            (true, false, true, false),
            (false, false, false, true),
            (false, true, true, true),
        ] {
            plc.set_bool("A", a);
            plc.set_bool("B", b);
            plc.set_bool("C", c);
            plc.set_bool("D", d);
            plc.scan();
            let out1 = (a && (b || c)) || d;
            assert_eq!(plc.get_bool("Out1"), out1, "{a} {b} {c} {d}");
            assert_eq!(plc.get_bool("Out2"), out1 && !c, "{a} {b} {c} {d}");
        }

        assert!(plc.get_bool("Bit5"), "Word.5 of 16#20");
        assert!(plc.get_bool("Masked"), "MEQ 16#20 AND 16#F0 = 16#20");
        assert!(!plc.get_bool("Never"), "AFI makes the rung false");
        assert!(
            plc.get_bool("Warm"),
            "21.5 > 20 (DINT immediate vs REAL) and < 25.0"
        );

        for (level, in_range, out_of_band, cmp) in [
            (5, false, true, false),
            (10, true, true, false),
            (15, true, false, true),
            (20, true, true, true),
            (25, false, true, true),
        ] {
            plc.set("Level", level);
            plc.scan();
            assert_eq!(plc.get_bool("InRange"), in_range, "LIM(10,{level},20)");
            assert_eq!(
                plc.get_bool("OutOfBand"),
                out_of_band,
                "LIM(20,{level},10) is circular"
            );
            assert_eq!(plc.get_bool("CmpOk"), cmp, "CMP at {level}");
        }

        plc.set("Flags[7]", 1);
        plc.set("Idx", 7);
        plc.scan();
        assert!(plc.get_bool("IdxBit"));
        plc.set("Idx", 6);
        plc.scan();
        assert!(!plc.get_bool("IdxBit"));
    });
}

#[test]
fn first_scan_flag_is_true_once() {
    with_plc("bits_branches.L5X", |plc| {
        plc.scan();
        assert!(plc.get_bool("FirstScan"));
        plc.scan();
        plc.scan();
        assert!(!plc.get_bool("FirstScan"));
        assert_eq!(plc.get("FsCount"), 1);
    });
}

#[test]
fn indirect_bits_are_written() {
    with_plc("bits_branches.L5X", |plc| {
        plc.set("Idx", 31);
        plc.set_bool("A", true);
        plc.scan();
        assert_eq!(plc.get("Bits32"), i32::MIN as i64, "OTL(Bits32.[31])");
        plc.set_bool("A", false);
        plc.set("Idx", 3);
        plc.set_bool("A", true);
        plc.scan();
        assert_eq!(plc.get("Bits32"), i32::MIN as i64 | 8);
        plc.set_bool("A", false);
        plc.set_bool("B", true);
        plc.scan();
        assert_eq!(plc.get("Bits32"), i32::MIN as i64, "OTU(Bits32.[3])");
    });
}

#[test]
fn seal_in_rung_latches_and_breaks() {
    with_plc("seal_in.L5X", |plc| {
        plc.scan();
        assert!(!plc.get_bool("Motor"));
        plc.set_bool("Start", true);
        plc.scan();
        assert!(plc.get_bool("Motor"));
        assert!(plc.get_bool("MainProgram.RunLamp"));
        plc.set_bool("Start", false);
        plc.scan();
        assert!(plc.get_bool("Motor"), "sealed in through its own contact");
        plc.set_bool("Stop", true);
        plc.scan();
        assert!(!plc.get_bool("Motor"));
        assert!(!plc.get_bool("MainProgram.RunLamp"));
        plc.set_bool("Stop", false);
        plc.scan();
        assert!(!plc.get_bool("Motor"));
    });
}

#[test]
fn neq_leq_geq_equ_nop_sub() {
    with_plc("bits_branches.L5X", |plc| {
        // (Level, NEQ 15, LEQ 15, GEQ 15, EQU 15.0) — EQU compares INT with a
        // REAL immediate in REAL (1756-RM003 "Data conversions").
        for (level, ne, le, ge, eq) in [
            (14, true, true, false, false),
            (15, false, true, true, true),
            (16, true, false, true, false),
        ] {
            plc.set("Level", level);
            plc.scan();
            assert_eq!(plc.get_bool("CmpNe"), ne, "NEQ at {level}");
            assert_eq!(plc.get_bool("CmpLe"), le, "LEQ at {level}");
            assert_eq!(plc.get_bool("CmpGe"), ge, "GEQ at {level}");
            assert_eq!(plc.get_bool("CmpEq"), eq, "EQU at {level}");
            // NOP leaves the rung true; the result of SUB goes to an INT.
            assert!(plc.get_bool("NopOut"));
            assert_eq!(plc.get("Diff"), level - 20);
        }
        plc.set("Level", -32768);
        plc.scan();
        assert_eq!(plc.get("Diff"), -32768 - 20 + 65536, "INT result keeps its low 16 bits");
    });
}
