// SPDX-License-Identifier: MPL-2.0

//! An integer literal where a TIME is expected is milliseconds, as in CODESYS
//! (TIME is a millisecond count there; the type checker warns about the
//! implicit conversion). plcc stores TIME as nanoseconds and took the literal as
//! raw nanoseconds: `ton(PT := 20)` was a 20 ns timer, `t := 5000` 5 µs.

mod common;
use common::run_with;

#[test]
fn integer_literals_on_time_are_milliseconds() {
    let src = r#"
VAR_GLOBAL gt : TIME := 250; END_VAR
FUNCTION Twice : TIME
VAR_INPUT t : TIME; END_VAR
Twice := t * 2;
END_FUNCTION
PROGRAM p
VAR
  t : TIME; t2 : TIME := 1000; tn : TON; q2 : BOOL; q3 : BOOL; et : TIME;
  sum : TIME; tw : TIME; bigger : BOOL; g : TIME; lt : LTIME := 7; d : DATE := 86400;
  scans : INT;
END_VAR
scans := scans + 1;
t := 5000;
tn(IN := TRUE, PT := 25);
IF scans = 2 THEN q2 := tn.Q; END_IF;
IF scans = 4 THEN q3 := tn.Q; END_IF;
et := tn.ET;
sum := t + 500;
tw := Twice(100);
bigger := t > 4999;
g := gt;
END_PROGRAM
"#;
    // The clock advances 10 ms per scan; the TON starts at 0 ms.
    let s = run_with(src, 4, 10, false);
    let ms = 1_000_000i64;
    assert_eq!(s.i64("t"), 5000 * ms);
    assert_eq!(s.i64("t2"), 1000 * ms);
    assert!(!s.bool("q2"), "10 ms < PT 25 ms");
    assert!(s.bool("q3"), "30 ms >= PT 25 ms");
    assert_eq!(s.i64("et"), 25 * ms);
    assert_eq!(s.i64("sum"), 5500 * ms);
    assert_eq!(s.i64("tw"), 200 * ms);
    assert!(s.bool("bigger"));
    assert_eq!(s.i64("g"), 250 * ms);
    assert_eq!(s.i64("lt"), 7, "LTIME's numeric unit is the nanosecond");
    assert_eq!(s.i64("d"), 86400 * 1_000_000_000, "DATE's is the second");
}
