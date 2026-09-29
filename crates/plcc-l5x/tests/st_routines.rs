// SPDX-License-Identifier: MPL-2.0

//! Structured Text routines: constructs, TONR, JSR with a parameter into a
//! ladder routine, an AOI with ST logic, aliases, keyword-named tags, SIZE,
//! and the Logix numeric rules for `/`, MOD and REAL → integer.

mod common;
use common::*;

#[test]
fn st_routine_runs_with_logix_semantics() {
    with_plc("st_routine.L5X", |plc| {
        plc.scan();
        assert_eq!(plc.get("State"), 0);
        assert_eq!(plc.get("Sum"), 15);
        assert_eq!(plc.get("Loops"), 23);
        assert_eq!(plc.get("Clamped"), 100, "Clamp_AOI with ST logic");
        assert_eq!(
            plc.get("Doubled"),
            200,
            "JSR(Doubler, 1, Clamped) → SBR(Arg)"
        );
        assert_eq!(
            plc.get_real("Doubled2"),
            200.0,
            "RET(Doubled) → JSR return operand (REAL)"
        );
        assert_eq!(plc.get("Elements"), 5);
        // Integer divide by zero: Source A; MOD by zero: 0 (1756-RM003 DIV/MOD).
        assert_eq!(plc.get("Quot"), 150);
        assert_eq!(plc.get("Rem"), 0);
        // REAL → DINT rounds half to even.
        assert_eq!(plc.get("Rounded"), 2);
        // INT := 16#7FFF + 1 is computed in DINT and truncated.
        assert_eq!(plc.get("Word__"), -32768);
        assert_eq!(plc.get("NonRet"), 0, "a [:=] target is reset by the prescan");
        // Indirect bits as assignment targets: bit 4 set, bit 0 cleared, bit
        // 15 (the sign of an INT) set.
        assert_eq!(plc.get("StBits"), 0x10 | -0x8000);
        assert_eq!(plc.get("Copied[0]"), 2, "COP in ST");
        assert_eq!(plc.get("Copied[2]"), 4);
        assert!(!plc.get_bool("LampOn"));

        plc.set_bool("Go", true);
        plc.scan();
        assert_eq!(plc.get("State"), 1, "GoAlias → Go");
        assert!(plc.get_bool("LampOn"), "AOI alias parameter Red → Cmd.0");
        assert_eq!(plc.get("Lamp1.Cmd"), 1);
        plc.scan();
        plc.scan();
        assert_eq!(plc.get("State"), 3);
        plc.scan();
        assert_eq!(plc.get("State"), 10);
        // TONR: first enabled scan starts timing; 100 ms later it is done.
        plc.scans(1, 60);
        assert_eq!(
            plc.get("Time__"),
            60,
            "the tag named `Time` (keyword: `Time__`)"
        );
        assert!(!plc.get_bool("Done"));
        plc.scans(1, 50);
        assert!(plc.get_bool("Done"));
        plc.set_bool("Go", false);
        plc.scan();
        assert!(!plc.get_bool("Done"));
        assert_eq!(plc.get("Time__"), 0);
    });
}
