// SPDX-License-Identifier: MPL-2.0

//! <Tasks> → the plcc task table: periodic tasks keep their rate and priority,
//! a continuous task runs in the free-running background task, an event task
//! is triggered by the EVENT instruction, inhibited tasks, unscheduled and
//! disabled programs do not run.

mod common;
use common::*;

#[test]
fn task_table_and_scheduling() {
    let diags = diagnostics("tasks.L5X");
    assert!(
        diags.iter().any(|d| d.contains("`Parked` is inhibited")),
        "{diags:?}"
    );
    assert!(
        diags
            .iter()
            .any(|d| d.contains("`Unscheduled` is not scheduled")),
        "{diags:?}"
    );
    assert!(
        diags.iter().any(|d| d.contains("`Off` is disabled")),
        "{diags:?}"
    );
    with_plc("tasks.L5X", |plc| {
        let names: Vec<String> = (0..plc.tasks().len()).map(|i| plc.task_name(i)).collect();
        assert!(names.iter().any(|n| n == "FastTask"), "{names:?}");
        let fast = names.iter().position(|n| n == "FastTask").unwrap();
        assert_eq!(plc.tasks()[fast].interval_ns, 10_000_000);
        assert_eq!(plc.tasks()[fast].priority, 5);
        let bg = names
            .iter()
            .position(|n| n == "__background")
            .expect("continuous task");
        assert_eq!(plc.tasks()[bg].interval_ns, 0);

        plc.run_task("FastTask");
        plc.run_task("FastTask");
        assert_eq!(plc.get("FastCount"), 2);
        assert_eq!(plc.get("SlowCount"), 0);
        plc.run_task("__background");
        assert_eq!(plc.get("SlowCount"), 1);
        assert_eq!(plc.get("Never"), 0);

        // EVENT(OnFire) raises the task's trigger; the event task clears it
        // when it runs.
        assert!(!plc.get_bool("lx__event_OnFire"));
        plc.set_bool("Fire", true);
        plc.run_task("__background");
        assert!(plc.get_bool("lx__event_OnFire"));
        plc.run_task("OnFire");
        assert_eq!(plc.get("EventCount"), 1);
        assert!(!plc.get_bool("lx__event_OnFire"));
    });
}
