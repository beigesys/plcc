// SPDX-License-Identifier: MPL-2.0

//! TIME / LTIME / DATE / TOD / DT → STRING, in literal syntax. They were
//! "unknown function". Date and time fields are not zero-padded, as in the
//! CODESYS help ("Conversion: DATE, DT, TOD": `DATE_TO_STRING(D#1970-1-1)` =
//! `'D#1970-1-1'`, `TOD_TO_STRING(12:0:1)` = `'TOD#12:0:1'`).

mod common;
use common::run;

#[test]
fn time_and_date_values_to_strings() {
    let src = r#"
PROGRAM p
VAR
  s1 : STRING; s2 : STRING; s3 : STRING; s4 : STRING; s5 : STRING; s6 : STRING;
  s7 : STRING; s8 : STRING; s9 : STRING; msg : STRING;
  s10 : STRING; s11 : STRING; s12 : STRING; s13 : STRING;
END_VAR
s1 := TIME_TO_STRING(T#1500ms);
s2 := TIME_TO_STRING(T#1d2h3m4s5ms);
s3 := TIME_TO_STRING(T#0s);
s4 := TIME_TO_STRING(T#-2s);
s5 := LTIME_TO_STRING(LTIME#1ms2us3ns);
s6 := DATE_TO_STRING(D#2024-03-05);
s7 := TOD_TO_STRING(TOD#09:05:07.25);
s8 := DT_TO_STRING(DT#2024-12-31-23:59:59);
s9 := DATE_TO_STRING(D#1969-12-31);
msg := CONCAT('up ', TIME_TO_STRING(T#90s));
s10 := DATE_TO_STRING(D#1970-01-01);
s11 := TOD_TO_STRING(TOD#12:00:01);
s12 := DT_TO_STRING(DT#1970-01-01-00:00:01);
s13 := TOD_TO_STRING(TOD#02:03:04.005);
END_PROGRAM
"#;
    let s = run(src);
    assert_eq!(s.str("s1"), "T#1s500ms");
    assert_eq!(s.str("s2"), "T#1d2h3m4s5ms");
    assert_eq!(s.str("s3"), "T#0ms");
    assert_eq!(s.str("s4"), "T#-2s");
    assert_eq!(s.str("s5"), "LTIME#1ms2us3ns");
    assert_eq!(s.str("s6"), "D#2024-3-5");
    assert_eq!(s.str("s7"), "TOD#9:5:7.250");
    assert_eq!(s.str("s8"), "DT#2024-12-31-23:59:59");
    assert_eq!(s.str("s9"), "D#1969-12-31");
    assert_eq!(s.str("msg"), "up T#1m30s");
    // The CODESYS help's own examples.
    assert_eq!(s.str("s10"), "D#1970-1-1");
    assert_eq!(s.str("s11"), "TOD#12:0:1");
    assert_eq!(s.str("s12"), "DT#1970-1-1-0:0:1");
    assert_eq!(s.str("s13"), "TOD#2:3:4.005");
}
