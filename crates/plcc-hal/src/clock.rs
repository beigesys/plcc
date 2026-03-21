// SPDX-License-Identifier: MPL-2.0
//! Real-time clock and elapsed time services.

/// Clock trait for PLC timing services.
///
/// All times are in nanoseconds for maximum precision.
/// Implementations must be monotonic for `now_ns()`.
pub trait Clock {
    /// Monotonic time in nanoseconds since an arbitrary epoch.
    /// Must never go backwards.
    fn now_ns(&self) -> u64;

    /// Wall-clock time in nanoseconds since Unix epoch (1970-01-01).
    /// Used for DATE_AND_TIME / RTC function blocks.
    fn wall_clock_ns(&self) -> u64;

    /// Nanoseconds elapsed since the last call to `begin_scan()`.
    /// Returns 0 if `begin_scan()` has not been called yet.
    fn elapsed_since_last_scan_ns(&self) -> u64;

    /// Mark the beginning of a new scan cycle.
    /// Resets the internal scan timer.
    fn begin_scan(&mut self);
}
