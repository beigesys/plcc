// SPDX-License-Identifier: MPL-2.0

//! TwinCAT projects compiled and run: a function block with a method,
//! properties and an action, an interface property, a GVL, DUTs, Tc2_Standard
//! timers, and the task from the .TcTTO. POUs\Drafts is excluded from the
//! build and holds an unfinished POU.

mod common;
use common::{MS, check_errors, with_plc};

const DEMO: &str = "Demo/Demo.plcproj";

#[test]
fn demo_project_type_checks() {
    let errors = check_errors(DEMO);
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn demo_project_runs() {
    with_plc(DEMO, |plc| {
        // The task comes from PlcTask.TcTTO: 10 ms, priority 20, calling MAIN.
        assert_eq!(plc.tasks().len(), 1);
        assert_eq!(plc.task_name(0), "PlcTask");
        assert_eq!(plc.tasks()[0].interval_ns, 10 * MS);
        assert_eq!(plc.tasks()[0].priority, 20);

        for _ in 0..3 {
            plc.scan();
            plc.advance(10 * MS);
        }
        // GVL_Main.nScans, written through the qualified name.
        assert_eq!(plc.get("nScans"), 3);
        // Property SET (Step := MAX_STEP) then GET, through the instance and
        // through the interface.
        assert_eq!(plc.get("MAIN.nStepSeen"), 7);
        assert_eq!(plc.get("MAIN.nItfStep"), 7);
        // Method Increment, called from the FB body, three times.
        assert_eq!(plc.get("MAIN.fbCounter.nCount"), 21);
        assert!(!plc.get_bool("MAIN.bAtLimit"));
        // DUT struct field and qualified enum.
        assert_eq!(plc.get("MAIN.stSample.nValue"), 3);
        // An array bound written with a GVL-qualified constant.
        assert_eq!(plc.get("MAIN.nLast"), 300);
        assert_eq!(plc.get("MAIN.eMode"), 11, "E_Mode.Done");
        // FB_Counter.HISTORY_LEN, a VAR CONSTANT read through the FB's name,
        // as an array bound and as a value.
        assert_eq!(plc.get("MAIN.nHistoryLen"), 4);
        assert_eq!(plc.get("MAIN.nHistoryLast"), 21);
        // PRG_Stats runs only when MAIN calls it (it is in no task): samples 7,
        // 14, 21 through its method; Mean is its property.
        assert_eq!(plc.get("MAIN.nStatsMax"), 21);
        assert_eq!(plc.get("MAIN.nStatsMean"), 14);

        for _ in 3..14 {
            plc.scan();
            plc.advance(10 * MS);
        }
        assert_eq!(plc.get("MAIN.fbCounter.nCount"), 98);
        assert!(!plc.get_bool("MAIN.bLimitReached"));
        assert_eq!(plc.get("MAIN.fbCounter.nResets"), 0);
        // Scan 15 reaches the limit: the AtLimit property reads TRUE, S= latches,
        // and the Reset action clears the count.
        plc.scan();
        assert!(plc.get_bool("MAIN.bAtLimit"));
        assert!(plc.get_bool("MAIN.bLimitReached"));
        assert_eq!(plc.get("MAIN.fbCounter.nCount"), 0);
        assert_eq!(plc.get("MAIN.fbCounter.nResets"), 1);
        // Tc2_Standard.TON with PT := T#50MS, 10 ms per scan.
        assert!(plc.get_bool("MAIN.bTimerQ"));
        assert!(plc.get_bool("bTimerDone"));
        // Read before PRG_Stats.Clear() (its action) ran in this scan.
        assert_eq!(plc.get("MAIN.nStatsMax"), 98);
        plc.advance(10 * MS);
        plc.scan();
        assert_eq!(plc.get("MAIN.nStatsMax"), 7, "cleared, then one sample of 7");
    });
}
