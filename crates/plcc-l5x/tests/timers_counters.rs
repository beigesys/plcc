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

/// A controller with the given tags and rungs in one program.
fn project(tags: &[(&str, &str)], rungs: &[&str]) -> String {
    let tags: String = tags
        .iter()
        .map(|(n, t)| format!("<Tag Name=\"{n}\" TagType=\"Base\" DataType=\"{t}\"/>\n"))
        .collect();
    let rungs: String = rungs
        .iter()
        .enumerate()
        .map(|(k, r)| format!("<Rung Number=\"{k}\" Type=\"N\"><Text><![CDATA[{r}]]></Text></Rung>\n"))
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<RSLogix5000Content SchemaRevision="1.0" SoftwareRevision="33.01" TargetName="P" TargetType="Controller">
<Controller Use="Target" Name="P" ProcessorType="1769-L33ER" MajorRev="33" MinorRev="11">
<DataTypes/><Modules/><AddOnInstructionDefinitions/>
<Tags>
{tags}</Tags>
<Programs><Program Name="MainProgram" MainRoutineName="MainRoutine"><Tags/><Routines>
<Routine Name="MainRoutine" Type="RLL"><RLLContent>
{rungs}</RLLContent></Routine></Routines></Program></Programs>
<Tasks><Task Name="T" Type="CONTINUOUS" Priority="10"><ScheduledPrograms><ScheduledProgram Name="MainProgram"/></ScheduledPrograms></Task></Tasks>
</Controller>
</RSLogix5000Content>
"#
    )
}

/// The statements of the program's prescan method, printed.
fn prescan(src: &str) -> Vec<String> {
    let unit = lower_src("prescan.L5X", src, &plcc_l5x::Options::default());
    for d in &unit.declarations {
        if let plcc_st::Declaration::FunctionBlock(f) = d {
            for m in &f.methods {
                if m.name.name == "lx__prescan" {
                    return m
                        .body
                        .iter()
                        .map(|s| plcc_st::print_statements(std::slice::from_ref(s), 0).trim().to_string())
                        .collect();
                }
            }
        }
    }
    panic!("no lx__prescan");
}

/// Each prescan action is stated once, in order: the TON's `Accum` operand
/// and its own prescan both clear `.ACC` (the studio's demo project).
#[test]
fn prescan_states_each_assignment_once() {
    let src = project(
        &[
            ("StartPB", "BOOL"),
            ("StopPB", "BOOL"),
            ("Motor", "BOOL"),
            ("Level", "INT"),
            ("High", "BOOL"),
            ("RunTimer", "TIMER"),
        ],
        &[
            "[XIC(StartPB) ,XIC(Motor) ]XIO(StopPB)OTE(Motor);",
            "XIC(Motor)TON(RunTimer,5000,0);",
            "GRT(Level,2000)OTE(High);",
        ],
    );
    assert_eq!(
        prescan(&src),
        [
            "Motor := FALSE;",
            "RunTimer.PRE := 5000;",
            "RunTimer.ACC := 0;",
            "RunTimer.EN := FALSE;",
            "RunTimer.TT := FALSE;",
            "RunTimer.DN := FALSE;",
            "High := FALSE;",
        ]
    );
}

/// A repeat is kept when something in between changed its target: two TOFs
/// on one TIMER each load `.ACC := 0` (their `Accum`) and then `.ACC := .PRE`,
/// so the second `.ACC := 0` stays and prescan still ends with `.ACC = .PRE`.
#[test]
fn prescan_keeps_a_repeat_after_a_change() {
    let src = project(
        &[("A", "BOOL"), ("B", "BOOL"), ("Out", "BOOL"), ("T", "TIMER")],
        &["XIC(A)TOF(T,50,0);", "XIC(B)TOF(T,50,0);", "XIC(T.DN)OTE(Out);", "XIC(A)OTE(Out);"],
    );
    let p = prescan(&src);
    assert_eq!(
        p,
        [
            "T.PRE := 50;",
            "T.ACC := 0;",
            "T.EN := FALSE;",
            "T.TT := FALSE;",
            "T.DN := FALSE;",
            "T.ACC := T.PRE;",
            "T.ACC := 0;",
            "T.ACC := T.PRE;",
            "Out := FALSE;",
        ],
        "{p:#?}"
    );
    with_plc_src("prescan.L5X", &src, &plcc_l5x::Options::default(), |plc| {
        plc.scan();
        assert_eq!(plc.get("T.ACC"), 50, "TOF prescan: .ACC = .PRE");
        assert!(!plc.get_bool("T.DN"));
    });
}
