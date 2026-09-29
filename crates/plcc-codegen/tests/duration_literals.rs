// SPDX-License-Identifier: MPL-2.0

//! Duration literals: every unit, fractions, negative values, the `L` forms.
//!
//! * `T#-1.5s` did not lex as one literal, and — since a missing `;` was
//!   accepted — parsed as `t := T#-1.5` followed by a statement `s;`.
//! * An unknown unit (`T#5x`) was a silent `T#0s`.
//! * Fractions truncated: `T#1.1s` was 1099999999 ns... or 1100000000, by luck.
//! * `LTIME#1ns` was typed TIME, so `lt : LTIME := LTIME#1ns` failed the check.

mod common;
use common::{compile_error, run};

#[test]
fn duration_literals() {
    let src = r#"
PROGRAM p
VAR
  t1 : TIME; t2 : TIME; t3 : TIME; t4 : LTIME; t5 : TIME; t6 : TIME; t7 : TIME; t8 : TIME;
  t9 : TIME;
END_VAR
t1 := T#1d2h3m4s5ms;
t2 := T#-1.5s;
t3 := TIME#100us;
t4 := LTIME#1ns;
t5 := T#1.5h;
t6 := T#61m;
t7 := T#0.3ms;
t8 := T#1_000ms;
t9 := T#1.1s;
END_PROGRAM
"#;
    let s = run(src);
    let ms = 1_000_000i64;
    assert_eq!(s.i64("t1"), (((24 + 2) * 60 + 3) * 60 + 4) * 1000 * ms + 5 * ms);
    assert_eq!(s.i64("t2"), -1500 * ms);
    assert_eq!(s.i64("t3"), 100_000);
    assert_eq!(s.i64("t4"), 1);
    assert_eq!(s.i64("t5"), 90 * 60 * 1000 * ms);
    assert_eq!(s.i64("t6"), 61 * 60 * 1000 * ms);
    assert_eq!(s.i64("t7"), 300_000);
    assert_eq!(s.i64("t8"), 1000 * ms);
    assert_eq!(s.i64("t9"), 1100 * ms);
}

#[test]
fn unknown_duration_unit_is_an_error() {
    let src = r#"
PROGRAM p
VAR t : TIME; END_VAR
t := T#5x;
END_PROGRAM
"#;
    let e = compile_error(src);
    assert!(e.contains("not a duration unit"), "{e}");
}

#[test]
fn a_duration_beyond_64_bit_nanoseconds_is_an_error() {
    // i64 nanoseconds end at about 106751 days; this used to saturate silently.
    for src in [
        "PROGRAM p VAR t : TIME; END_VAR t := T#200000d; END_PROGRAM",
        // Past the unsigned LTIME range too.
        "PROGRAM p VAR t : LTIME; END_VAR t := LTIME#300000d; END_PROGRAM",
    ] {
        let e = compile_error(src);
        assert!(e.contains("does not fit"), "{e}");
    }
}

#[test]
fn the_largest_ltime_keeps_its_bit_pattern() {
    // CODESYS/TwinCAT LTIME is unsigned: its largest value is 2^64 - 1 ns.
    // TcUnit and TcOpen's tests write it; plcc holds it as the same 64 bits.
    let s = run("PROGRAM p VAR t : LTIME; u : LTIME; same : BOOL; END_VAR
        t := LTIME#213503D23H34M33S709MS551US615NS;
        u := LTIME#213503D23H33M33S709MS551US615NS;
        same := t = u;
        END_PROGRAM");
    assert_eq!(s.u64("t"), u64::MAX);
    assert_eq!(s.u64("u"), u64::MAX - 60_000_000_000);
    assert!(!s.bool("same"));
}
