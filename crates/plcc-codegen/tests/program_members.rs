// SPDX-License-Identifier: MPL-2.0

//! Methods, properties and actions of a PROGRAM (CODESYS/TwinCAT), called
//! from inside it and through its name.

mod common;
use common::run_with;

#[test]
fn program_methods_properties_and_actions() {
    let src = r#"
PROGRAM Stats
VAR_INPUT sample : DINT; END_VAR
VAR n, sum, top : DINT; END_VAR
    n := n + 1;
    Add(sample);
METHOD Add
VAR_INPUT v : DINT; END_VAR
    sum := sum + v;
    IF v > top THEN top := v; END_IF
END_METHOD
PROPERTY Mean : DINT
GET
    IF n > 0 THEN Mean := sum / n; END_IF
END_GET
END_PROPERTY
ACTION Clear:
    n := 0; sum := 0; top := 0;
END_ACTION
END_PROGRAM

PROGRAM p
VAR scans, mean, top, after_clear : DINT; END_VAR
scans := scans + 1;
Stats(sample := scans * 10);
mean := Stats.Mean;
top := Stats.top;
IF scans = 3 THEN
    Stats.Clear();
    after_clear := Stats.Mean;
END_IF
END_PROGRAM
"#;
    let s = run_with(src, 3, 10, false);
    assert_eq!(s.i64("mean"), 20, "(10 + 20 + 30) / 3");
    assert_eq!(s.i64("top"), 30);
    assert_eq!(s.i64("after_clear"), 0);
}
