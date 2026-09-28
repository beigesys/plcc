// SPDX-License-Identifier: MPL-2.0
//! End to end: ST with AT %IX/%QX and a CONFIGURATION, JIT-compiled, run by the
//! generic scan cycle on the Linux simulator — inputs driven and outputs observed
//! only through the simulator's process image.

#![cfg(feature = "runner")]

use plcc_hal::jit::{JitModule, JitOptions};
use plcc_hal::platform::Platform;
use plcc_hal::process_image::ProcessImage;
use plcc_hal::scan::{ScanCycle, StartKind};
use plcc_hal::simulator::LinuxSimulator;
use plcc_hal::task::TaskScheduler;

const MS: u64 = 1_000_000;

fn sim() -> LinuxSimulator {
    LinuxSimulator::builder()
        .input_size(8)
        .output_size(8)
        .marker_size(8)
        .build()
}

fn load(src: &str) -> JitModule {
    JitModule::compile(&[("test.st", src)], &JitOptions::default()).expect("compile")
}

const MOTOR: &str = r#"
PROGRAM Motor
VAR
    start AT %IX0.0 : BOOL;
    stop  AT %IX0.1 : BOOL;
    run   AT %QX0.0 : BOOL;
    hours AT %QW1 : INT;
    on_delay : TON;
    ready AT %QX0.1 : BOOL;
END_VAR
    run := (run OR start) AND NOT stop;
    on_delay(IN := run, PT := T#1h);
    ready := on_delay.Q;
    IF run THEN hours := hours + 1; END_IF;
END_PROGRAM

CONFIGURATION Plant
    RESOURCE Cpu ON Board
        TASK Main (INTERVAL := T#10ms, PRIORITY := 1);
        PROGRAM m WITH Main : Motor;
    END_RESOURCE
END_CONFIGURATION
"#;

#[test]
fn motor_latch_through_the_simulator_image() {
    let module = load(MOTOR);
    let mut plat = sim();
    plat.init().unwrap();
    let mut cycle = ScanCycle::new(module.application().unwrap(), &mut plat).unwrap();
    let tasks = cycle.tasks();
    assert_eq!(tasks[0].name, "Main");
    assert_eq!(tasks[0].programs, [("Cpu.m".to_string(), "Motor".to_string())]);
    assert_eq!(cycle.start(&mut plat), StartKind::Cold);

    let out = |p: &LinuxSimulator, byte| p.process_image().read_output(byte).unwrap();

    // Nothing pressed.
    let r = cycle.step_at(&mut plat, 0).unwrap();
    assert_eq!(r.ran, [0]);
    assert_eq!(out(&plat, 0) & 1, 0);

    // Press start for one cycle: the motor latches on.
    plat.process_image_mut().write_input(0, 0b01);
    cycle.step_at(&mut plat, 10 * MS).unwrap();
    assert_eq!(out(&plat, 0) & 1, 1);
    plat.process_image_mut().write_input(0, 0b00);
    cycle.step_at(&mut plat, 20 * MS).unwrap();
    assert_eq!(out(&plat, 0) & 1, 1, "latched");

    // Not due yet: nothing runs, outputs unchanged.
    let r = cycle.step_at(&mut plat, 25 * MS).unwrap();
    assert!(r.ran.is_empty());
    assert_eq!(r.next_due_ns, Some(30 * MS));

    // Stop.
    plat.process_image_mut().write_input(0, 0b10);
    cycle.step_at(&mut plat, 30 * MS).unwrap();
    assert_eq!(out(&plat, 0) & 1, 0);
    let hours = i16::from_ne_bytes([out(&plat, 2), out(&plat, 3)]);
    assert_eq!(hours, 2, "%QW1 counted the two running scans");
}

const TWO_TASKS: &str = r#"
VAR_GLOBAL
    fast_n AT %QB0 : BYTE;
    slow_n AT %QB1 : BYTE;
    order AT %QB2 : BYTE;
    trigger AT %IX0.0 : BOOL;
    events AT %QB3 : BYTE;
END_VAR
PROGRAM Fast
    fast_n := fast_n + 1;
    order := order * 4 + 1;
END_PROGRAM
PROGRAM Slow
    slow_n := slow_n + 1;
    order := order * 4 + 2;
END_PROGRAM
PROGRAM OnEvent
    events := events + 1;
END_PROGRAM
CONFIGURATION C
    TASK T10 (INTERVAL := T#10ms, PRIORITY := 5);
    TASK T50 (INTERVAL := T#50ms, PRIORITY := 1);
    TASK Ev (SINGLE := trigger, PRIORITY := 0);
    PROGRAM f WITH T10 : Fast;
    PROGRAM s WITH T50 : Slow;
    PROGRAM e WITH Ev : OnEvent;
END_CONFIGURATION
"#;

#[test]
fn periodic_tasks_by_interval_and_priority() {
    let module = load(TWO_TASKS);
    let mut plat = sim();
    let mut cycle = ScanCycle::new(module.application().unwrap(), &mut plat).unwrap();
    cycle.start(&mut plat);
    let out = |p: &LinuxSimulator, b| p.process_image().read_output(b).unwrap();

    // First step at t=0: both due; T50 (priority 1) runs before T10 (priority 5).
    let r = cycle.step_at(&mut plat, 0).unwrap();
    assert_eq!(r.ran, [1, 0]);
    assert_eq!(out(&plat, 2), 2 * 4 + 1, "Slow then Fast");

    for ms in (1..100).map(|t| t as u64) {
        cycle.step_at(&mut plat, ms * MS).unwrap();
    }
    assert_eq!(out(&plat, 0), 10, "T10 ran at 0,10,..,90");
    assert_eq!(out(&plat, 1), 2, "T50 ran at 0 and 50");
    assert_eq!(out(&plat, 3), 0, "no event yet");

    // An overrun skips missed activations instead of bursting.
    let r = cycle.step_at(&mut plat, 135 * MS).unwrap();
    assert_eq!(r.ran, [1, 0]);
    assert!(r.overruns >= 3, "{r:?}");
    assert_eq!(r.next_due_ns, Some(140 * MS));
}

#[test]
fn single_runs_once_per_rising_edge() {
    let module = load(TWO_TASKS);
    let mut plat = sim();
    let mut cycle = ScanCycle::new(module.application().unwrap(), &mut plat).unwrap();
    cycle.start(&mut plat);
    let events = |p: &LinuxSimulator| p.process_image().read_output(3).unwrap();
    // SINGLE reads a global AT %IX0.0; the value is latched before readiness is
    // evaluated, so an input edge triggers the task in the same step.
    plat.process_image_mut().write_input(0, 1);
    let r = cycle.step_at(&mut plat, 1).unwrap();
    assert_eq!(r.ran[0], 2, "event task (priority 0) first: {r:?}");
    for t in 2..5 {
        cycle.step_at(&mut plat, t).unwrap();
    }
    assert_eq!(events(&plat), 1, "held high: still one activation");
    plat.process_image_mut().write_input(0, 0);
    cycle.step_at(&mut plat, 6).unwrap();
    plat.process_image_mut().write_input(0, 1);
    cycle.step_at(&mut plat, 7).unwrap();
    assert_eq!(events(&plat), 2);
}

#[test]
fn a_stopped_task_does_not_run() {
    let module = load(TWO_TASKS);
    let mut plat = sim();
    let mut cycle = ScanCycle::new(module.application().unwrap(), &mut plat).unwrap();
    cycle.start(&mut plat);
    plat.task_scheduler_mut().stop_task(0).unwrap();
    let r = cycle.step_at(&mut plat, 0).unwrap();
    assert_eq!(r.ran, [1]);
}

#[test]
fn default_task_without_configuration() {
    let module = load(
        "PROGRAM Blink VAR q AT %QX0.0 : BOOL; END_VAR q := NOT q; END_PROGRAM",
    );
    let mut plat = sim();
    let mut cycle = ScanCycle::new(module.application().unwrap(), &mut plat).unwrap();
    assert_eq!(cycle.tasks()[0].name, "MainTask");
    assert_eq!(cycle.tasks()[0].interval_ns, 20 * MS);
    for t in 0..5 {
        cycle.step_at(&mut plat, t * 20 * MS).unwrap();
    }
    assert_eq!(plat.process_image().read_output(0), Some(1));
}

const RETAINED: &str = r#"
PROGRAM Main
VAR RETAIN
    starts : DINT;
END_VAR
VAR
    volatile_n : DINT;
    out AT %QD0 : DINT;
    out2 AT %QD1 : DINT;
END_VAR
    starts := starts + 1;
    volatile_n := volatile_n + 1;
    out := starts;
    out2 := volatile_n;
END_PROGRAM
"#;

#[test]
fn retain_survives_a_restart() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("hal_retain");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("retain.bin");
    let _ = std::fs::remove_file(&path);
    let mk = || {
        LinuxSimulator::builder()
            .output_size(8)
            .retain_path(path.clone())
            .build()
    };
    let read = |p: &LinuxSimulator, b: u32| {
        i32::from_ne_bytes(std::array::from_fn(|i| {
            p.process_image().read_output(b + i as u32).unwrap()
        }))
    };

    {
        let module = load(RETAINED);
        let mut plat = mk();
        let mut cycle = ScanCycle::new(module.application().unwrap(), &mut plat).unwrap();
        assert_eq!(cycle.start(&mut plat), StartKind::Cold);
        for t in 0..3 {
            cycle.step_at(&mut plat, t * 20 * MS).unwrap();
        }
        assert_eq!(read(&plat, 0), 3);
        cycle.save_retain(&mut plat).unwrap();
    }
    {
        // "Power cycle": a fresh module and platform.
        let module = load(RETAINED);
        let mut plat = mk();
        let mut cycle = ScanCycle::new(module.application().unwrap(), &mut plat).unwrap();
        assert_eq!(cycle.start(&mut plat), StartKind::Warm);
        cycle.step_at(&mut plat, 0).unwrap();
        assert_eq!(read(&plat, 0), 4, "RETAIN continued");
        assert_eq!(read(&plat, 4), 1, "non-RETAIN restarted");
    }
    {
        // A different RETAIN layout must not be fed stale bytes.
        let other = RETAINED.replace("starts : DINT;", "starts : DINT; extra : INT;");
        let module = load(&other);
        let mut plat = mk();
        let mut cycle = ScanCycle::new(module.application().unwrap(), &mut plat).unwrap();
        assert_eq!(cycle.start(&mut plat), StartKind::Cold);
    }
}
