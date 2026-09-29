// SPDX-License-Identifier: MPL-2.0

//! Diagnostics point into the .L5X file: the rung text, the operand, the
//! routine or tag they are about.

mod common;
use common::*;

fn diags(name: &str) -> (String, Vec<plcc_l5x::L5xError>) {
    let src = read_fixture(name);
    let (_, errs) = plcc_l5x::parse(&src);
    (src, errs)
}

fn at<'a>(src: &'a str, e: &plcc_l5x::L5xError) -> &'a str {
    let o = e.span.offset();
    &src[o..o + e.span.len()]
}

#[test]
fn bad_rungs_are_reported_at_their_text() {
    let (src, errs) = diags("errors/bad_rungs.L5X");
    let find = |needle: &str| {
        errs.iter()
            .find(|e| e.message.contains(needle))
            .unwrap_or_else(|| panic!("no diagnostic containing {needle:?}: {errs:#?}"))
    };
    let e = find("rung 0");
    assert!(e.message.contains("branch"), "{}", e.message);
    let e = find("unknown tag `Missing`");
    assert_eq!(at(&src, e), "Missing");
    let e = find("`FROB` is not supported");
    assert_eq!(at(&src, e), "FROB");
    let e = find("TON needs a TIMER operand");
    assert_eq!(at(&src, e), "N");
    let e = find("`PowerFlex525_Drive`, which plcc cannot model");
    assert!(!e.is_warning());
    assert_eq!(at(&src, e), "Vfd");
    let e = find("`Blocks`: FBD routines are not supported yet");
    assert!(at(&src, e).starts_with("<Routine Name=\"Blocks\""));
    find("`Steps`: SFC routines are not supported yet");
    // The unsupported tag itself is a warning at its declaration.
    let w = errs
        .iter()
        .find(|e| e.is_warning() && e.message.contains("left out"))
        .expect("warning for the tag");
    assert_eq!(at(&src, w), "PowerFlex525_Drive");
}

#[test]
fn not_an_l5x_file() {
    let (_, errs) = plcc_l5x::parse("<project/>");
    assert!(
        errs[0].message.contains("not an L5X export"),
        "{:?}",
        errs[0]
    );
    let (_, errs) = plcc_l5x::parse("<RSLogix5000Content><Controller>");
    assert!(errs[0].message.contains("malformed XML"), "{:?}", errs[0]);
    assert!(plcc_l5x::is_l5x(
        "\u{feff}<?xml version=\"1.0\"?>\n<RSLogix5000Content SchemaRevision=\"1.0\">"
    ));
    assert!(!plcc_l5x::is_l5x("<project/>"));
}

#[test]
fn every_fixture_lowers_type_checks_and_generates_no_warnings_from_plcc() {
    for f in [
        "seal_in.L5X",
        "timers_counters.L5X",
        "bits_branches.L5X",
        "math.L5X",
        "structure.L5X",
        "tasks.L5X",
        "st_routine.L5X",
        "shift_loop.L5X",
        "strings.L5X",
    ] {
        let errs = check_errors(f);
        assert!(errs.is_empty(), "{f}: {errs:#?}");
    }
}
