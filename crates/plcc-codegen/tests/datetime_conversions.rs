// SPDX-License-Identifier: MPL-2.0

//! `<SRC>_TO_<DST>` between every pair of elementary types, `TO_<DST>`, and the
//! DATE / TIME_OF_DAY / DATE_AND_TIME functions.
//!
//! Units follow CODESYS: DATE and DT convert to/from seconds since 1970-01-01, TOD
//! and TIME to/from milliseconds, the L-types to/from nanoseconds. All of them are
//! stored as i64 nanoseconds.

mod common;
use common::{run, run_o3, run_with};

const NS: i64 = 1_000_000_000;

#[test]
fn date_family_to_numbers_uses_codesys_units() {
    let src = r#"
PROGRAM p
VAR
  d : DATE := D#2024-03-15;
  dt1 : DT := DT#2024-03-15-13:45:30;
  tod1 : TOD := TOD#13:45:30.250;
  a : DWORD; b : DWORD; c : DWORD; e : UDINT; f : LREAL; g : ULINT;
END_VAR
a := DATE_TO_DWORD(d);
b := DT_TO_DWORD(dt1);
c := TOD_TO_DWORD(tod1);
e := DATE_TO_UDINT(d);
f := TOD_TO_LREAL(tod1);
g := LTIME_TO_ULINT(LTIME#1ms);
END_PROGRAM
"#;
    let s = run(src);
    assert_eq!(s.u64("a"), 1_710_460_800);
    assert_eq!(s.u64("b"), 1_710_510_330);
    assert_eq!(s.u64("c"), 49_530_250);
    assert_eq!(s.u64("e"), 1_710_460_800);
    assert_eq!(s.f64("f"), 49_530_250.0);
    assert_eq!(s.u64("g"), 1_000_000);
}

#[test]
fn numbers_to_date_family_and_between_date_types() {
    let src = r#"
PROGRAM p
VAR
  dt1 : DT := DT#2024-03-15-13:45:30.5;
  old : DT := DT#1969-12-31-23:00:00;
  a : DATE; b : TOD; c : DATE; d : DT; e : TOD; f : DATE; g : TIME;
END_VAR
a := DWORD_TO_DATE(1710460800);
b := DT_TO_TOD(dt1);
c := DT_TO_DATE(dt1);
d := UDINT_TO_DT(16#FFFFFFFF);
e := DT_TO_TOD(old);
f := DT_TO_DATE(old);
g := DINT_TO_TIME(-1500);
END_PROGRAM
"#;
    let s = run(src);
    assert_eq!(s.i64("a"), 1_710_460_800 * NS);
    assert_eq!(s.i64("b"), 49_530_500_000_000);
    assert_eq!(s.i64("c"), 1_710_460_800 * NS);
    assert_eq!(s.i64("d"), 4_294_967_295 * NS, "UDINT source is unsigned");
    assert_eq!(s.i64("e"), 23 * 3600 * NS, "time of day of a pre-1970 DT");
    assert_eq!(s.i64("f"), -86_400 * NS);
    assert_eq!(s.i64("g"), -1_500_000_000);
}

#[test]
fn conversions_that_were_unknown_functions() {
    let src = r#"
PROGRAM p
VAR
  w : WORD; r : REAL; b : BOOL; i : INT; si : SINT; l : LINT; t : TIME;
  us : USINT; bi : INT; lr : LREAL; dw : DWORD;
END_VAR
w := DWORD_TO_WORD(16#12345678);
r := BYTE_TO_REAL(200);
b := DINT_TO_BOOL(256);
i := TO_INT(-3.5);
si := INT_TO_SINT(200);
l := TIME_TO_LINT(T#1h);
t := LINT_TO_TIME(1500);
us := SINT_TO_USINT(-1);
bi := BOOL_TO_INT(5 > 3);
lr := TO_LREAL(UINT#65535);
dw := UINT_TO_DWORD(65535);
END_PROGRAM
"#;
    for s in [run(src), run_o3(src)] {
        assert_eq!(s.u64("w"), 0x5678);
        assert_eq!(s.f64("r"), 200.0);
        assert!(s.bool("b"), "256 is non-zero, not truncated to 0");
        assert_eq!(s.i64("i"), -4);
        assert_eq!(s.i64("si"), -56);
        assert_eq!(s.i64("l"), 3_600_000);
        assert_eq!(s.i64("t"), 1_500_000_000);
        assert_eq!(s.u64("us"), 255);
        assert_eq!(s.i64("bi"), 1);
        assert_eq!(s.f64("lr"), 65535.0);
        assert_eq!(s.u64("dw"), 65535);
    }
}

#[test]
fn date_and_time_functions() {
    let src = r#"
PROGRAM p
VAR
  dt1 : DT := DT#2024-03-15-13:45:30;
  dt2 : DT := DT#2024-03-14-12:00:00;
  tod1 : TOD := TOD#13:45:30.250;
  d1 : DATE := D#2024-03-15;
  mul : TIME; dv : TIME; sub : TIME;
  cat : DT; cd : DATE; ct : TOD; cdt : DT; add : DT;
  dow : INT; dow2 : INT;
END_VAR
mul := MUL_TIME(T#2s, 2.5);
dv := DIV_TIME(T#10s, 4);
sub := SUB_DT_DT(dt1, dt2);
cat := CONCAT_DATE_TOD(d1, tod1);
cd := CONCAT_DATE(2024, 3, 15);
ct := CONCAT_TOD(13, 45, 30, 250);
cdt := CONCAT_DT(1969, 12, 31, 23, 59, 59, 0);
add := ADD_DT_TIME(dt1, T#1d);
dow := DAY_OF_WEEK(d1);
dow2 := DAY_OF_WEEK(D#1969-12-28);
END_PROGRAM
"#;
    let s = run(src);
    assert_eq!(s.i64("mul"), 5 * NS);
    assert_eq!(s.i64("dv"), 2_500_000_000);
    assert_eq!(s.i64("sub"), 92_730 * NS);
    assert_eq!(s.i64("cat"), 1_710_510_330_250_000_000);
    assert_eq!(s.i64("cd"), 1_710_460_800 * NS);
    assert_eq!(s.i64("ct"), 49_530_250_000_000);
    assert_eq!(s.i64("cdt"), -NS);
    assert_eq!(s.i64("add"), (1_710_510_330 + 86_400) * NS);
    assert_eq!(s.i64("dow"), 5, "2024-03-15 was a Friday");
    assert_eq!(s.i64("dow2"), 0, "1969-12-28 was a Sunday");
}

#[test]
fn date_arithmetic_operators() {
    let src = r#"
PROGRAM p
VAR
  dt1 : DT := DT#2024-03-15-13:45:30;
  dt2 : DT := DT#2024-03-14-12:00:00;
  d1 : DATE := D#2024-03-15;
  d2 : DATE := D#2024-03-01;
  tod1 : TOD := TOD#13:45:30.250;
  a : TIME; b : DT; c : TOD; e : TIME; f : BOOL;
END_VAR
a := dt1 - dt2;
b := dt1 + T#1h;
c := tod1 + T#10m;
e := d1 - d2;
f := dt1 > dt2;
END_PROGRAM
"#;
    let s = run(src);
    assert_eq!(s.i64("a"), 92_730 * NS);
    assert_eq!(s.i64("b"), (1_710_510_330 + 3600) * NS);
    assert_eq!(s.i64("c"), 49_530_250_000_000 + 600 * NS);
    assert_eq!(s.i64("e"), 14 * 86_400 * NS);
    assert!(s.bool("f"));
}

#[test]
fn codesys_time_function_is_milliseconds_since_start() {
    let src = r#"
PROGRAM p
VAR t : TIME; END_VAR
t := TIME();
END_PROGRAM
"#;
    // The clock advances 1234 ms per scan; the third scan reads 2468 ms.
    let s = run_with(src, 3, 1234, false);
    assert_eq!(s.i64("t"), 2_468_000_000);
}
