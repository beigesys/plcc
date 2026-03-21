// SPDX-License-Identifier: MPL-2.0
//! Platform supertrait bundling all HAL components.

use thiserror::Error;

use crate::clock::Clock;
use crate::comms::VariableAccess;
use crate::diagnostics::DiagnosticSink;
use crate::process_image::ProcessImage;
use crate::retain::RetainStorage;
use crate::task::TaskScheduler;
use crate::watchdog::Watchdog;

/// Errors from platform lifecycle operations.
#[derive(Debug, Error)]
pub enum PlatformError {
    #[error("platform initialization failed: {0}")]
    InitFailed(String),
    #[error("platform shutdown failed: {0}")]
    ShutdownFailed(String),
    #[error("platform error: {0}")]
    Other(String),
}

/// Platform trait that bundles all HAL components.
///
/// Platform integrators implement this trait to provide a complete
/// runtime environment for compiled PLC programs.
pub trait Platform {
    type Clock: Clock;
    type ProcessImage: ProcessImage;
    type TaskScheduler: TaskScheduler;
    type RetainStorage: RetainStorage;
    type Watchdog: Watchdog;
    type DiagnosticSink: DiagnosticSink;
    type VariableAccess: VariableAccess;

    /// Initialize the platform. Called once at startup before any scan cycles.
    fn init(&mut self) -> Result<(), PlatformError>;

    /// Shut down the platform. Called once at exit.
    fn shutdown(&mut self) -> Result<(), PlatformError>;

    /// Access the clock.
    fn clock(&self) -> &Self::Clock;

    /// Mutable access to the clock.
    fn clock_mut(&mut self) -> &mut Self::Clock;

    /// Access the process image.
    fn process_image(&self) -> &Self::ProcessImage;

    /// Mutable access to the process image.
    fn process_image_mut(&mut self) -> &mut Self::ProcessImage;

    /// Access the task scheduler.
    fn task_scheduler(&self) -> &Self::TaskScheduler;

    /// Mutable access to the task scheduler.
    fn task_scheduler_mut(&mut self) -> &mut Self::TaskScheduler;

    /// Access retain storage.
    fn retain_storage(&self) -> &Self::RetainStorage;

    /// Mutable access to retain storage.
    fn retain_storage_mut(&mut self) -> &mut Self::RetainStorage;

    /// Access the watchdog.
    fn watchdog(&self) -> &Self::Watchdog;

    /// Mutable access to the watchdog.
    fn watchdog_mut(&mut self) -> &mut Self::Watchdog;

    /// Access the diagnostic sink.
    fn diagnostic_sink(&self) -> &Self::DiagnosticSink;

    /// Mutable access to the diagnostic sink.
    fn diagnostic_sink_mut(&mut self) -> &mut Self::DiagnosticSink;

    /// Access variable access services.
    fn variable_access(&self) -> &Self::VariableAccess;

    /// Mutable access to variable access services.
    fn variable_access_mut(&mut self) -> &mut Self::VariableAccess;
}
