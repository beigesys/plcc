// SPDX-License-Identifier: MPL-2.0

//! UDTs, arrays, aliases, an Add-On Instruction, subroutines, jumps, MCR,
//! TND, COP and FLL.

mod common;
use common::*;

#[test]
fn udt_initial_values_from_l5k_and_decorated_data() {
    with_plc("structure.L5X", |plc| {
        // M1 from L5K: the hidden SINT host 1 carries Run (bit 0).
        assert!(plc.get_bool("M1.Run"));
        assert!(!plc.get_bool("M1.Fault"));
        assert_eq!(plc.get("M1.Speed"), 1500);
        // M2 from decorated data.
        assert!(plc.get_bool("M2.Fault"));
        assert_eq!(plc.get("M2.Speed"), 750);
        assert_eq!(plc.get("M2.Tmr.PRE"), 2500);
        assert_eq!(plc.get("Arr[9]"), 100);
        plc.scan();
        assert_eq!(plc.get("GridSum"), 7, "Grid[0,0] + Grid[1,2]");
        assert_eq!(plc.get_real("M1.Setpoints[1]"), 2.5);
        assert_eq!(plc.get_real("M2.Setpoints[2]"), 9.25);
    });
}

#[test]
fn aliases_resolve_to_their_targets() {
    with_plc("structure.L5X", |plc| {
        plc.scan();
        assert_eq!(plc.get("SpeedCopy"), 1500, "XIC(StartAlias) reads M1.Run");
        assert!(
            plc.get_bool("MainProgram.SawFault"),
            "program alias LocalRun → M2.Fault"
        );
        plc.set_bool("M1.Run", false);
        plc.set("M1.Speed", 1);
        plc.scan();
        assert_eq!(plc.get("SpeedCopy"), 1500);
    });
}

#[test]
fn subroutines_with_parameters_and_ret() {
    with_plc("structure.L5X", |plc| {
        plc.scan();
        plc.scan();
        assert_eq!(plc.get("MainProgram.SubCalls"), 2);
        assert!(
            !plc.get_bool("MainProgram.AfterRet"),
            "RET ends the subroutine"
        );
        assert_eq!(
            plc.get("MainProgram.PSum"),
            5 + 20,
            "JSR inputs copied into the SBR tags"
        );
        assert_eq!(
            plc.get("MainProgram.SqOut"),
            49,
            "RET(SqTmp) copied into the JSR return operand"
        );
    });
}

#[test]
fn add_on_instruction_call() {
    with_plc("structure.L5X", |plc| {
        plc.scan();
        // Rung false: EnableIn FALSE, the logic does not run, EnableOut FALSE.
        assert_eq!(plc.get("ScaleTag.Calls"), 0);
        assert!(!plc.get_bool("AoiOut"));
        plc.set_bool("AoiGate", true);
        plc.scan();
        assert_eq!(plc.get_real("Scaled"), 12.0, "50 % of 4..20");
        assert_eq!(plc.get("RefCount"), 1, "InOut parameter written through");
        assert!(plc.get_bool("AoiOut"), "rung continues with EnableOut");
        plc.scan();
        assert_eq!(plc.get("ScaleTag.Calls"), 2);
        assert_eq!(plc.get("RefCount"), 2);
    });
}

#[test]
fn file_instructions() {
    with_plc("structure.L5X", |plc| {
        plc.scan();
        assert_eq!(plc.get("Arr2[1]"), 0);
        assert_eq!(plc.get("Arr2[2]"), 10);
        assert_eq!(plc.get("Arr2[4]"), 30);
        assert_eq!(plc.get("Arr2[5]"), 0);
        assert_eq!(plc.get("Arr3[0]"), 0);
        assert_eq!(plc.get("Arr3[1]"), 7);
        assert_eq!(plc.get("Arr3[4]"), 7, "FLL stops at the end of the array");
    });
}

#[test]
fn jumps_mcr_and_tnd() {
    with_plc("structure.L5X", |plc| {
        plc.scan();
        assert!(!plc.get_bool("Skipped"), "JMP skips to its LBL");
        assert!(plc.get_bool("Landed"));
        assert_eq!(plc.get("Iter"), 5, "a backward jump loops within the scan");
        // MCR zone disabled (ZoneOn FALSE): the rungs inside run false.
        assert!(!plc.get_bool("InZone"));
        assert!(plc.get_bool("AfterZone"), "the terminating MCR re-enables");
        plc.set_bool("ZoneOn", true);
        plc.scan();
        assert!(plc.get_bool("InZone"));
        assert!(plc.get_bool("AfterTnd"));
        plc.set_bool("DoTnd", true);
        plc.set_bool("ZoneOn", false);
        plc.scan();
        assert!(
            plc.get_bool("AfterTnd"),
            "TND ends the routine: rungs after it keep their state"
        );
        assert!(!plc.get_bool("InZone"));
    });
}
