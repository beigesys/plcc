// SPDX-License-Identifier: MPL-2.0
//! Hardware Abstraction Layer for plcc PLC runtime.
//! Platform integrators implement these traits for their hardware.

pub mod app;
pub mod clock;
pub mod comms;
pub mod diagnostics;
pub mod fault;
pub mod io_driver;
pub mod platform;
pub mod process_image;
pub mod retain;
pub mod scan;
pub mod task;
pub mod watchdog;

#[cfg(feature = "simulator")]
pub mod simulator;

#[cfg(feature = "runner")]
pub mod jit;

// Re-export the ProcessImageLayout that codegen targets
pub use app::Application;
pub use platform::Platform;
pub use scan::ScanCycle;
pub use process_image::ProcessImageLayout;
