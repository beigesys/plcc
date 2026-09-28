// SPDX-License-Identifier: MPL-2.0

//! TIME / LTIME / DATE / TOD / DT → STRING, in literal syntax. They were
//! "unknown function".

mod common;
use common::run;

#[test]
fn time_and_date_values_to_strings() {
    let src = r#"
PROGRAM p
VAR
  s1 : STRING; s2 : STRING; s3 : STRING; s4 : STRING; s5 : STRING; s6 : STRING;
  s7 : STRING; s8 : STRING; s9 : STRING; msg : STRING;
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
END_PROGRAM
"#;
    let s = run(src);
    assert_eq!(s.str("s1"), "T#1s500ms");
    assert_eq!(s.str("s2"), "T#1d2h3m4s5ms");
    assert_eq!(s.str("s3"), "T#0ms");
    assert_eq!(s.str("s4"), "T#-2s");
    assert_eq!(s.str("s5"), "LTIME#1ms2us3ns");
    assert_eq!(s.str("s6"), "D#2024-03-05");
    assert_eq!(s.str("s7"), "TOD#09:05:07.250");
    assert_eq!(s.str("s8"), "DT#2024-12-31-23:59:59");
    assert_eq!(s.str("s9"), "D#1969-12-31");
    assert_eq!(s.str("msg"), "up T#1m30s");
}
