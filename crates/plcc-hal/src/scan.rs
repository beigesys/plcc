// SPDX-License-Identifier: MPL-2.0
//! The IEC 61131-3 scan cycle over a compiled [`Application`] and a [`Platform`].
//!
//! One [`ScanCycle::step`] is: latch the platform's inputs into the program's `%I`
//! area, run every task that is due (highest priority first — IEC priority 0 is the
//! highest), then flush the program's `%Q` area to the platform's outputs.
//!
//! Scheduling is cooperative and non-preemptive: a due task runs to completion
//! before the next one starts, which is what a single-core runtime without an RTOS
//! (an Arduino loop, a bare-metal superloop) does. A platform with preemptive
//! threads can use [`Application::run_task`] directly instead.
//!
//! Task readiness follows IEC 61131-3 §6.8.2:
//! * INTERVAL > 0: periodic, while SINGLE (if any) is FALSE. A task that overruns
//!   skips the missed activations instead of running back-to-back to catch up.
//! * SINGLE: once on each rising edge.
//! * neither: free-running, every step (the background task of unassociated
//!   program instances, or `INTERVAL := T#0s`).

use thiserror::Error;

use crate::app::{AppError, Application, TaskInfo};
use crate::diagnostics::{DiagLevel, DiagnosticSink};
use crate::fault::Fault;
use crate::platform::Platform;
use crate::process_image::ProcessImage;
use crate::retain::{RetainError, RetainStorage};
use crate::task::{TaskConfig, TaskError, TaskHandle, TaskScheduler};
use crate::clock::Clock;

#[derive(Debug, Error)]
pub enum ScanError {
    #[error(transparent)]
    Task(#[from] TaskError),
    #[error(transparent)]
    Retain(#[from] RetainError),
    #[error(transparent)]
    App(#[from] AppError),
    /// Compiled code raised a runtime fault (e.g. an integer division by zero).
    /// Every task is stopped and the outputs are in their safe state (all zero);
    /// the cycle stays stopped until [`ScanCycle::start`] restarts it.
    #[error("PLC stopped: {fault} in task {task}")]
    Fault { task: String, fault: Fault },
}

/// What one [`ScanCycle::step`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StepReport {
    /// Indices of the tasks that ran, in execution order.
    pub ran: Vec<usize>,
    /// Earliest time (same clock as `now_ns`) a periodic task is next due.
    pub next_due_ns: Option<u64>,
    /// Periodic activations skipped because the task was late.
    pub overruns: u64,
}

/// Whether a step started from retained data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartKind {
    /// No (usable) retain data: every variable at its initial value.
    Cold,
    /// RETAIN variables restored from the platform's retain storage.
    Warm,
}

struct TaskState {
    info: TaskInfo,
    handle: TaskHandle,
    next_due: Option<u64>,
    prev_single: bool,
}

/// Drives a compiled [`Application`] on a [`Platform`].
pub struct ScanCycle {
    app: Application,
    tasks: Vec<TaskState>,
    started: bool,
    /// The fault that stopped the cycle: (task name, fault).
    fault: Option<(String, Fault)>,
}

impl ScanCycle {
    /// Register every task of `app` with the platform's scheduler (so the platform
    /// can list, stop and start them) and prepare the cycle.
    pub fn new<P: Platform>(app: Application, platform: &mut P) -> Result<Self, ScanError> {
        let mut tasks = Vec::new();
        for info in app.tasks() {
            let mut cfg = TaskConfig::new(&info.name, info.interval_ns);
            cfg.priority = info.priority;
            let sched = platform.task_scheduler_mut();
            let handle = sched.create_task(cfg)?;
            sched.start_task(handle)?;
            tasks.push(TaskState {
                info,
                handle,
                next_due: None,
                prev_single: false,
            });
        }
        Ok(Self {
            app,
            tasks,
            started: false,
            fault: None,
        })
    }

    pub fn app(&self) -> &Application {
        &self.app
    }

    pub fn app_mut(&mut self) -> &mut Application {
        &mut self.app
    }

    pub fn tasks(&self) -> Vec<&TaskInfo> {
        self.tasks.iter().map(|t| &t.info).collect()
    }

    /// The fault that stopped the cycle, and the task it happened in.
    pub fn fault(&self) -> Option<(&str, &Fault)> {
        self.fault.as_ref().map(|(t, f)| (t.as_str(), f))
    }

    /// `plcc_init()`, then restore RETAIN variables from the platform's retain
    /// storage when it holds data for this program (a warm start).
    ///
    /// After a fault this is the restart (CODESYS: reset, then start): the fault
    /// is cleared, the tasks are started again and every variable is
    /// re-initialized. A fault in an initial value stops the cycle again; the
    /// next [`Self::step`] reports it.
    pub fn start<P: Platform>(&mut self, platform: &mut P) -> StartKind {
        if self.fault.take().is_some() {
            for t in &mut self.tasks {
                let _ = platform.task_scheduler_mut().start_task(t.handle);
                t.next_due = None;
                t.prev_single = false;
            }
        }
        self.started = true;
        if let Err(fault) = self.app.try_init() {
            self.stop_on_fault(platform, "init".into(), fault);
            return StartKind::Cold;
        }
        match platform.retain_storage().restore() {
            Ok(data) if self.app.restore_retain(&data).is_ok() => StartKind::Warm,
            _ => StartKind::Cold,
        }
    }

    /// Persist every RETAIN variable to the platform's retain storage.
    pub fn save_retain<P: Platform>(&self, platform: &mut P) -> Result<(), ScanError> {
        platform
            .retain_storage_mut()
            .save(&self.app.retain_snapshot())?;
        Ok(())
    }

    /// One scan cycle at the platform clock's current time.
    pub fn step<P: Platform>(&mut self, platform: &mut P) -> Result<StepReport, ScanError> {
        let now = platform.clock().now_ns();
        self.step_at(platform, now)
    }

    /// One scan cycle at `now_ns` (monotonic nanoseconds). Tests and simulators
    /// drive time explicitly through this.
    pub fn step_at<P: Platform>(
        &mut self,
        platform: &mut P,
        now_ns: u64,
    ) -> Result<StepReport, ScanError> {
        if !self.started {
            self.start(platform);
        }
        if let Some((task, fault)) = &self.fault {
            return Err(ScanError::Fault {
                task: task.clone(),
                fault: fault.clone(),
            });
        }
        platform.clock_mut().begin_scan();

        // 1. Latch inputs.
        platform.process_image().read_inputs(self.app.inputs_mut());

        // 2. Which tasks are due.
        let mut report = StepReport::default();
        let mut due = Vec::new();
        for (i, t) in self.tasks.iter_mut().enumerate() {
            if !platform.task_scheduler().is_running(t.handle)? {
                continue;
            }
            let single = if t.info.has_single {
                let v = self.app.single(i).unwrap_or(false);
                let rising = v && !t.prev_single;
                t.prev_single = v;
                if rising {
                    due.push(i);
                    continue;
                }
                v
            } else {
                false
            };
            let interval = t.info.interval_ns;
            if interval == 0 {
                if !t.info.has_single {
                    due.push(i);
                }
                continue;
            }
            let next = *t.next_due.get_or_insert(now_ns);
            if now_ns >= next {
                if !single {
                    due.push(i);
                }
                let mut n = next + interval;
                if n <= now_ns {
                    let missed = (now_ns - n) / interval + 1;
                    report.overruns += missed;
                    n += missed * interval;
                }
                t.next_due = Some(n);
            }
        }
        due.sort_by_key(|&i| (self.tasks[i].info.priority, i));

        // 3. Run them. A fault stops the PLC: no further task runs.
        for &i in &due {
            if let Err(fault) = self.app.try_run_task(i) {
                let task = self.tasks[i].info.name.clone();
                self.stop_on_fault(platform, task.clone(), fault.clone());
                return Err(ScanError::Fault { task, fault });
            }
        }
        report.ran = due;
        report.next_due_ns = self.tasks.iter().filter_map(|t| t.next_due).min();

        // 4. Flush outputs.
        platform.process_image_mut().write_outputs(self.app.outputs());
        Ok(report)
    }

    /// What CODESYS does on an exception: stop every task, drive the outputs to
    /// their safe state (all zero) and report. The program's `%Q` area is cleared
    /// too, so a later flush cannot resurrect the old outputs.
    fn stop_on_fault<P: Platform>(&mut self, platform: &mut P, task: String, fault: Fault) {
        for t in &self.tasks {
            let _ = platform.task_scheduler_mut().stop_task(t.handle);
        }
        let image = self.app.image();
        if !image.output_ptr.is_null() && image.output_size > 0 {
            // Safety: the descriptor's %Q area, owned by the loaded module.
            unsafe { core::ptr::write_bytes(image.output_ptr as *mut u8, 0, image.output_size as usize) };
        }
        platform.process_image_mut().write_outputs(self.app.outputs());
        platform.diagnostic_sink_mut().report(
            DiagLevel::Fatal,
            fault.diag_code(),
            &format!("task {task}: {fault}; PLC stopped, outputs cleared"),
        );
        self.fault = Some((task, fault));
    }
}
