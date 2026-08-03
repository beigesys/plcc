// SPDX-License-Identifier: MPL-2.0
//! Task scheduler for cyclic PLC execution.

use thiserror::Error;

/// Configuration for a PLC task.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TaskConfig {
    /// Null-terminated task name.
    pub name: [u8; 64],
    /// Cycle interval in nanoseconds. 0 means free-running.
    pub interval_ns: u64,
    /// Priority (0 = lowest, 255 = highest).
    pub priority: u8,
    /// Watchdog timeout in nanoseconds. 0 means no watchdog.
    pub watchdog_ns: u64,
}

impl TaskConfig {
    /// Create a new TaskConfig with the given name and interval.
    pub fn new(name: &str, interval_ns: u64) -> Self {
        let mut name_buf = [0u8; 64];
        let bytes = name.as_bytes();
        let len = bytes.len().min(63); // leave room for null terminator
        name_buf[..len].copy_from_slice(&bytes[..len]);
        Self {
            name: name_buf,
            interval_ns,
            priority: 0,
            watchdog_ns: 0,
        }
    }

    /// Get the task name as a string slice.
    pub fn name_str(&self) -> &str {
        let end = self
            .name
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.name.len());
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }
}

/// Opaque handle to a scheduled task.
pub type TaskHandle = u32;

/// Errors from the task scheduler.
#[derive(Debug, Error)]
pub enum TaskError {
    #[error("maximum number of tasks reached")]
    TooManyTasks,
    #[error("invalid task handle: {0}")]
    InvalidHandle(TaskHandle),
    #[error("task interval too short: {interval_ns}ns (minimum: {min_ns}ns)")]
    IntervalTooShort { interval_ns: u64, min_ns: u64 },
    #[error("scheduler error: {0}")]
    Other(String),
}

/// Task scheduler trait for cyclic PLC program execution.
pub trait TaskScheduler {
    /// Register a new task with the given configuration.
    fn create_task(&mut self, config: TaskConfig) -> Result<TaskHandle, TaskError>;

    /// Start a previously created task.
    fn start_task(&mut self, handle: TaskHandle) -> Result<(), TaskError>;

    /// Stop a running task.
    fn stop_task(&mut self, handle: TaskHandle) -> Result<(), TaskError>;

    /// Check whether a task is currently running.
    fn is_running(&self, handle: TaskHandle) -> Result<bool, TaskError>;

    /// Remove a task from the scheduler. Must be stopped first.
    fn destroy_task(&mut self, handle: TaskHandle) -> Result<(), TaskError>;

    /// Number of currently registered tasks.
    fn task_count(&self) -> usize;
}
