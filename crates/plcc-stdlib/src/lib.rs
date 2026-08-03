// SPDX-License-Identifier: MPL-2.0

//! The IEC 61131-3 standard function blocks, written as ST source and bundled
//! into the compiler.
//!
//! # Why ST and not Rust
//!
//! `plcc-runtime` has Rust implementations of these blocks, but nothing can call
//! them from compiled ST without an ABI bridge, a linkable runtime archive, and a
//! second FB implementation model that LLVM cannot see through. Compiling the
//! standard blocks from ST alongside the user program gives:
//!
//! * one FB implementation model — the standard blocks are ordinary
//!   FUNCTION_BLOCKs with ordinary state structs;
//! * full inlining and constant propagation across the user/stdlib boundary;
//! * nothing extra to link on bare metal — the only imported symbol the timers
//!   need is `plcc_monotonic_ns` (see `docs/runtime-symbols.md`).
//!
//! # Contents
//!
//! Per IEC 61131-3 section 2.5.2: `SR`, `RS`, `R_TRIG`, `F_TRIG`, `CTU`, `CTD`,
//! `CTUD`, `TON`, `TOF`, `TP`.
//!
//! `RTC` is **not** included: it needs a wall-clock source, and the only clock the
//! runtime contract defines is monotonic (deliberately — a wall clock can step
//! backwards). Adding `RTC` means adding a second runtime symbol.
//!
//! The sources are embedded with `include_str!`, so nothing is read from disk at
//! compile time or run time.

/// One bundled ST source unit: a stable name and its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StdlibUnit {
    /// Short name, used in diagnostics as the pseudo-filename.
    pub name: &'static str,
    /// The ST source text.
    pub source: &'static str,
}

/// Bistable elements: `SR`, `RS`.
pub const BISTABLE: StdlibUnit = StdlibUnit {
    name: "<plcc-stdlib>/bistable.st",
    source: include_str!("../st/bistable.st"),
};

/// Edge detection: `R_TRIG`, `F_TRIG`.
pub const EDGE: StdlibUnit = StdlibUnit {
    name: "<plcc-stdlib>/edge.st",
    source: include_str!("../st/edge.st"),
};

/// Counters: `CTU`, `CTD`, `CTUD`.
pub const COUNTERS: StdlibUnit = StdlibUnit {
    name: "<plcc-stdlib>/counters.st",
    source: include_str!("../st/counters.st"),
};

/// Timers: `TON`, `TOF`, `TP`.
pub const TIMERS: StdlibUnit = StdlibUnit {
    name: "<plcc-stdlib>/timers.st",
    source: include_str!("../st/timers.st"),
};

/// Every bundled unit, in a stable order.
pub const UNITS: &[StdlibUnit] = &[BISTABLE, EDGE, COUNTERS, TIMERS];

/// Names of every POU the bundled standard library defines, uppercased.
///
/// Kept in sync with the `.st` sources by `bundled_pou_names_match_sources`.
pub const POU_NAMES: &[&str] = &[
    "SR", "RS", "R_TRIG", "F_TRIG", "CTU", "CTD", "CTUD", "TON", "TOF", "TP",
];

/// The whole bundled standard library as a single ST source string.
///
/// Convenient for tests and for callers that just want to prepend it to a
/// program. The CLI parses the units individually so diagnostics can name the
/// unit they came from.
pub fn combined_source() -> String {
    UNITS
        .iter()
        .map(|u| u.source)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `POU_NAMES` is used to decide which prelude declarations a user POU
    /// supersedes, so it must actually match what the sources declare.
    #[test]
    fn bundled_pou_names_match_sources() {
        let mut declared: Vec<String> = Vec::new();
        for unit in UNITS {
            for line in unit.source.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("FUNCTION_BLOCK ") {
                    declared.push(rest.trim().to_uppercase());
                }
            }
        }
        declared.sort();
        let mut expected: Vec<String> = POU_NAMES.iter().map(|n| n.to_string()).collect();
        expected.sort();
        assert_eq!(
            declared, expected,
            "POU_NAMES is out of sync with the bundled .st sources"
        );
    }

    #[test]
    fn every_unit_has_an_spdx_header() {
        for unit in UNITS {
            assert!(
                unit.source
                    .starts_with("(* SPDX-License-Identifier: MPL-2.0 *)"),
                "{} is missing its SPDX header",
                unit.name
            );
        }
    }

    #[test]
    fn combined_source_contains_every_block() {
        let all = combined_source();
        for name in POU_NAMES {
            assert!(
                all.contains(&format!("FUNCTION_BLOCK {name}")),
                "combined_source() is missing FUNCTION_BLOCK {name}"
            );
        }
    }
}
