// SPDX-License-Identifier: MPL-2.0

//! BSL/BSR, FOR/BRK, SIZE, UID/UIE, and GSV/MSG (accepted with a warning).

mod common;
use common::*;

#[test]
fn bit_shifts_loops_and_size() {
    let diags = diagnostics("shift_loop.L5X");
    assert!(
        diags.iter().any(|d| d.contains("GSV WallClockTime")),
        "{diags:?}"
    );
    assert!(
        diags
            .iter()
            .any(|d| d.contains("MSG: plcc has no CIP messaging")),
        "{diags:?}"
    );
    with_plc("shift_loop.L5X", |plc| {
        plc.scan();
        assert_eq!(plc.get("TrackCtl.LEN"), 40, "Length pseudo-operand");
        assert_eq!(
            plc.get("Total"),
            1 + 2 + 3 + 4,
            "BRK at the first negative entry"
        );
        assert_eq!(plc.get("TableLen"), 8);
        // Shift a 1 in, then zeros: it walks up the 40-bit register, across
        // the DINT boundary.
        for k in 0..36 {
            plc.set_bool("Part", k == 0);
            plc.set_bool("Clock", true);
            plc.scan();
            plc.set_bool("Clock", false);
            plc.scan();
        }
        // After 36 shifts the first bit loaded sits at bit 35 = Track[1].3.
        assert_eq!(plc.get("Track[0]"), 0);
        assert_eq!(plc.get("Track[1]"), 1 << 3);
        assert!(!plc.get_bool("TrackCtl.DN"), "a false rung clears DN");
        // BSR over 4 bits of 2#0110 with zeros shifted in.
        assert_eq!(plc.get("Back"), 0);
        assert!(!plc.get_bool("Msg1.EN"));
        plc.set_bool("Poll", true);
        plc.scan();
        assert!(!plc.get_bool("Msg1.DN"), "MSG never completes");
    });
}

#[test]
fn bsr_unloads_bit_zero() {
    with_plc("shift_loop.L5X", |plc| {
        plc.scan();
        // Back = 2#0110. One BSR shift with Part = 1: 2#1011, UL = 0.
        plc.set_bool("Part", true);
        plc.set_bool("Clock", true);
        plc.scan();
        assert_eq!(plc.get("Back"), 0b1011);
        assert!(!plc.get_bool("BackCtl.UL"));
        assert!(plc.get_bool("BackCtl.DN"));
        assert_eq!(plc.get("BackCtl.POS"), 4);
        plc.scan();
        assert_eq!(
            plc.get("Back"),
            0b1011,
            "one shift per false→true transition"
        );
        plc.set_bool("Clock", false);
        plc.scan();
        plc.set_bool("Part", false);
        plc.set_bool("Clock", true);
        plc.scan();
        assert_eq!(plc.get("Back"), 0b0101);
        assert!(plc.get_bool("BackCtl.UL"));
    });
}
