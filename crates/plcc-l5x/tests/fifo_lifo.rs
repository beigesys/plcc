// SPDX-License-Identifier: MPL-2.0

//! FFL/FFU and LFL/LFU (1756-RM003 "Array (File)/Shift Instructions"), and
//! SWPB, on L5X fixtures run scan by scan.

mod common;
use common::*;

/// One false-to-true transition of `bit` (the stack instructions act on the
/// rung's rising edge only).
fn pulse(plc: &Plc, bit: &str) {
    plc.set_bool(bit, true);
    plc.scan();
    plc.set_bool(bit, false);
    plc.scan();
}

#[test]
fn fifo_loads_and_unloads_in_order() {
    with_plc("fifo_lifo.L5X", |plc| {
        plc.scan();
        assert!(plc.get_bool("FifoCtl.EM"), "empty after the prescan");
        assert_eq!(plc.get("FifoCtl.LEN"), 4, "Length pseudo-operand");
        for v in [10, 20, 30] {
            plc.set("In", v);
            pulse(plc, "Load");
        }
        assert_eq!(plc.get("FifoCtl.POS"), 3);
        assert!(!plc.get_bool("FifoCtl.EM"));
        assert!(!plc.get_bool("FifoCtl.DN"));
        // A rung held true loads once.
        plc.set("In", 40);
        plc.set_bool("Load", true);
        plc.scan();
        plc.scan();
        plc.scan();
        plc.set_bool("Load", false);
        plc.scan();
        assert_eq!(plc.get("FifoCtl.POS"), 4);
        assert!(plc.get_bool("FifoCtl.DN"), "full");
        // Full: the next load is inhibited.
        plc.set("In", 50);
        pulse(plc, "Load");
        assert_eq!(plc.get("FifoCtl.POS"), 4);
        let fifo: Vec<i64> = (0..4).map(|i| plc.get(&format!("Fifo[{i}]"))).collect();
        assert_eq!(fifo, vec![10, 20, 30, 40]);

        pulse(plc, "Unload");
        assert_eq!(plc.get("Out"), 10, "first in, first out");
        assert_eq!(plc.get("FifoCtl.POS"), 3);
        assert!(!plc.get_bool("FifoCtl.DN"));
        for want in [20, 30, 40] {
            pulse(plc, "Unload");
            assert_eq!(plc.get("Out"), want);
        }
        assert_eq!(plc.get("FifoCtl.POS"), 0);
        assert!(plc.get_bool("FifoCtl.EM"));
        // Empty: FFU returns 0.
        pulse(plc, "Unload");
        assert_eq!(plc.get("Out"), 0);
        assert_eq!(plc.get("FifoCtl.POS"), 0);
    });
}

#[test]
fn lifo_unloads_last_in_first_and_clears_the_slot() {
    with_plc("fifo_lifo.L5X", |plc| {
        plc.scan();
        for v in [1, 2, 3, 4] {
            plc.set("In", v);
            pulse(plc, "Load");
        }
        // Length 3 starting at Lifo[1]: the fourth load is refused.
        let lifo: Vec<i64> = (0..5).map(|i| plc.get(&format!("Lifo[{i}]"))).collect();
        assert_eq!(lifo, vec![0, 1, 2, 3, 0]);
        assert!(plc.get_bool("LifoCtl.DN"));
        pulse(plc, "Unload");
        assert_eq!(plc.get("LOut"), 3, "last in, first out");
        assert_eq!(plc.get("Lifo[3]"), 0, "LFU stores 0 in the unloaded slot");
        pulse(plc, "Unload");
        assert_eq!(plc.get("LOut"), 2);
        pulse(plc, "Unload");
        assert_eq!(plc.get("LOut"), 1);
        assert!(plc.get_bool("LifoCtl.EM"));
        pulse(plc, "Unload");
        assert_eq!(plc.get("LOut"), 0, "empty LIFO returns 0");
    });
}

#[test]
fn fifo_of_a_structure() {
    with_plc("fifo_lifo.L5X", |plc| {
        plc.scan();
        for (id, w) in [(7, 1.5), (8, 2.5)] {
            plc.set("NewPart.Id", id);
            plc.set_real("NewPart.Weight", w);
            pulse(plc, "Load");
        }
        pulse(plc, "Unload");
        assert_eq!(plc.get("GotPart.Id"), 7);
        assert_eq!(plc.get_real("GotPart.Weight"), 1.5);
        pulse(plc, "Unload");
        assert_eq!(plc.get("GotPart.Id"), 8, "shifted down");
        assert_eq!(plc.get_real("GotPart.Weight"), 2.5);
    });
}

#[test]
fn swap_bytes() {
    with_plc("fifo_lifo.L5X", |plc| {
        plc.scan();
        assert_eq!(plc.get("WordSw"), 0x3412);
        assert_eq!(plc.get("WordDint"), 0x3412);
        assert_eq!(plc.get("Rev"), 0x44332211);
        assert_eq!(plc.get("Wrd"), 0x33441122);
        assert_eq!(plc.get("HiLo"), 0x22114433);
        assert_eq!(plc.get("StRev"), 0x44332211, "SWPB in Structured Text");
    });
}
