// SPDX-License-Identifier: MPL-2.0
//! Runtime faults raised by compiled code through `plcc_fault(code, where)`.
//!
//! The hook must not return. On a host, [`unwinding_fault_handler`] satisfies that
//! by unwinding out of the compiled code back to [`Application::try_run_task`]
//! (the JIT maps `plcc_fault` onto it), which turns the fault into a value; the
//! [`ScanCycle`](crate::scan::ScanCycle) then stops every task, puts the outputs in
//! their safe state and reports the fault through the platform's
//! [`DiagnosticSink`](crate::diagnostics::DiagnosticSink).
//!
//! A bare-metal runtime does not unwind: its own `plcc_fault` clears `%Q`, stops
//! the tasks and parks or resets (see docs/runtime-symbols.md).
//!
//! [`Application::try_run_task`]: crate::app::Application::try_run_task

use core::ffi::{CStr, c_char};

use crate::diagnostics::DiagCode;
pub use plcc_runtime::fault::FaultCode;

/// One `plcc_fault` call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fault {
    /// The raw code (a [`FaultCode`], or a runtime's own code).
    pub code: u32,
    /// The site string: `"file:line:col: POU"`, or just the POU.
    pub site: String,
}

impl Fault {
    /// The plcc fault code, if `code` is one.
    pub fn kind(&self) -> Option<FaultCode> {
        FaultCode::from_code(self.code)
    }

    /// The diagnostic code a [`DiagnosticSink`](crate::diagnostics::DiagnosticSink)
    /// receives for this fault.
    pub fn diag_code(&self) -> DiagCode {
        match self.kind() {
            Some(FaultCode::DivByZero) => DiagCode::DivisionByZero,
            Some(FaultCode::ArrayBounds) => DiagCode::ArrayIndexOutOfBounds,
            Some(FaultCode::NullReference) => DiagCode::InvalidAccess,
            None => DiagCode::UserDefined,
        }
    }
}

impl core::fmt::Display for Fault {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let what = self.kind().map(FaultCode::describe).unwrap_or("runtime fault");
        write!(f, "{what} (fault {}) at {}", self.code, self.site)
    }
}

/// A `plcc_fault` for code that has unwind tables (a host JIT or a host object):
/// it unwinds to the nearest [`Application::try_run_task`] /
/// [`Application::try_init`] with the [`Fault`] as payload. It never returns.
///
/// `resume_unwind` does not run the panic hook, so nothing is printed.
///
/// # Safety (for the caller of the compiled code)
/// Only call compiled code that may reach this through a `catch_unwind` — the
/// `try_*` methods of [`Application`] do.
///
/// [`Application`]: crate::app::Application
/// [`Application::try_run_task`]: crate::app::Application::try_run_task
/// [`Application::try_init`]: crate::app::Application::try_init
pub extern "C-unwind" fn unwinding_fault_handler(code: u32, site: *const c_char) {
    let site = if site.is_null() {
        String::new()
    } else {
        // Safety: compiled code passes a NUL-terminated constant string.
        unsafe { CStr::from_ptr(site) }.to_string_lossy().into_owned()
    };
    std::panic::resume_unwind(Box::new(Fault { code, site }));
}

/// Run `f`, turning a fault unwound by [`unwinding_fault_handler`] into `Err`.
/// Any other panic keeps unwinding.
pub(crate) fn catch_fault(f: impl FnOnce()) -> Result<(), Fault> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(()) => Ok(()),
        Err(payload) => match payload.downcast::<Fault>() {
            Ok(fault) => Err(*fault),
            Err(other) => std::panic::resume_unwind(other),
        },
    }
}
