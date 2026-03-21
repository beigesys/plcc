// SPDX-License-Identifier: MPL-2.0
//! Diagnostic reporting for runtime errors and warnings.

/// Severity level for diagnostics.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiagLevel {
    Info = 0,
    Warning = 1,
    Error = 2,
    Fatal = 3,
}

/// Standard diagnostic codes for PLC runtime conditions.
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagCode {
    /// No error.
    None = 0,
    /// Division by zero in arithmetic expression.
    DivisionByZero = 1,
    /// Array index outside declared bounds.
    ArrayIndexOutOfBounds = 2,
    /// Scan cycle exceeded its configured deadline.
    ScanOverrun = 3,
    /// Numeric overflow in integer arithmetic.
    NumericOverflow = 4,
    /// Stack overflow (deeply nested calls).
    StackOverflow = 5,
    /// Watchdog timeout.
    WatchdogTripped = 6,
    /// Retain storage failure.
    RetainError = 7,
    /// Fieldbus communication failure.
    FieldbusError = 8,
    /// Null pointer or invalid memory access.
    InvalidAccess = 9,
    /// String operation exceeded STRING max length.
    StringOverflow = 10,
    /// User-defined diagnostic (application specific).
    UserDefined = 1000,
}

/// Trait for collecting runtime diagnostics.
pub trait DiagnosticSink {
    /// Report a diagnostic with the given level, code, and human-readable message.
    fn report(&mut self, level: DiagLevel, code: DiagCode, message: &str);

    /// Number of errors (level >= Error) reported since last clear.
    fn error_count(&self) -> usize;

    /// Clear all recorded diagnostics.
    fn clear_errors(&mut self);
}
