// SPDX-License-Identifier: MPL-2.0

//! Host implementation of the one runtime symbol the compiler imports for time.
//!
//! Compiled ST calls `plcc_monotonic_ns() -> i64` whenever it needs a clock —
//! `MONOTONIC_NS()` in ST source, and the bundled ST standard library's timer
//! function blocks (TON/TOF/TP) underneath. The compiler emits an *external*
//! declaration; the platform supplies the definition.
//!
//! # Porting
//!
//! A bare-metal integrator must export exactly one C symbol:
//!
//! ```c
//! int64_t plcc_monotonic_ns(void);
//! ```
//!
//! Contract:
//!   * nanoseconds since an arbitrary but *fixed* epoch — only differences matter;
//!   * monotonic non-decreasing, never runs backwards, never wraps within a session;
//!   * callable from the scan context, and cheap (timers call it every scan).
//!
//! A SysTick counter, a free-running hardware timer, or `clock_gettime(CLOCK_MONOTONIC)`
//! all satisfy this. This module is the host/simulator implementation of it.

use std::sync::OnceLock;
use std::time::Instant;

/// Fixed process epoch. `Instant` has no public representation, so the epoch is
/// captured once and every reading is an elapsed duration from it.
fn epoch() -> &'static Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now)
}

/// Monotonic nanoseconds since this process's fixed epoch.
///
/// Saturates rather than wrapping; `i64::MAX` nanoseconds is about 292 years of
/// uptime, so saturation is unreachable in practice but is still the safe choice
/// over a panicking or wrapping cast.
pub fn monotonic_ns() -> i64 {
    let nanos = epoch().elapsed().as_nanos();
    i64::try_from(nanos).unwrap_or(i64::MAX)
}

/// The C ABI entry point compiled ST links against.
///
/// Exported under the exact symbol name `plcc_monotonic_ns` so that natively
/// linked ST object files resolve against it without a shim.
#[unsafe(no_mangle)]
pub extern "C" fn plcc_monotonic_ns() -> i64 {
    monotonic_ns()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotonic_ns_never_decreases() {
        let mut prev = monotonic_ns();
        for _ in 0..1000 {
            let now = monotonic_ns();
            assert!(now >= prev, "clock went backwards: {prev} -> {now}");
            prev = now;
        }
    }

    #[test]
    fn monotonic_ns_advances_over_a_sleep() {
        let before = monotonic_ns();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let after = monotonic_ns();
        assert!(
            after - before >= 1_000_000,
            "expected at least 1ms of advance, got {}ns",
            after - before
        );
    }

    #[test]
    fn extern_entry_point_matches_the_safe_one() {
        let a = plcc_monotonic_ns();
        let b = monotonic_ns();
        assert!(b >= a);
    }
}
