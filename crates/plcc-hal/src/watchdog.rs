// SPDX-License-Identifier: MPL-2.0
//! Safety watchdog for scan overrun detection.

use thiserror::Error;

/// Errors from the watchdog.
#[derive(Debug, Error)]
pub enum WatchdogError {
    #[error("watchdog not started")]
    NotStarted,
    #[error("watchdog already running")]
    AlreadyRunning,
    #[error("watchdog error: {0}")]
    Other(String),
}

/// Hardware or software watchdog trait.
///
/// The watchdog must be "kicked" (fed) within its timeout period.
/// If `kick()` is not called in time, `has_tripped()` returns true
/// and the platform should take corrective action (e.g., set outputs
/// to safe state).
pub trait Watchdog {
    /// Start the watchdog with the given timeout in nanoseconds.
    fn start(&mut self, timeout_ns: u64) -> Result<(), WatchdogError>;

    /// Feed the watchdog. Must be called within the timeout period.
    fn kick(&mut self) -> Result<(), WatchdogError>;

    /// Stop the watchdog.
    fn stop(&mut self) -> Result<(), WatchdogError>;

    /// Check whether the watchdog has tripped (timed out).
    fn has_tripped(&self) -> bool;
}
