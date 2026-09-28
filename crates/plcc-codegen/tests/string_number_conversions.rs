// SPDX-License-Identifier: MPL-2.0

//! `<int>_TO_STRING`, `BOOL_TO_STRING` and `STRING_TO_<int>`. They were
//! "unknown function" (INT_TO_STRING is OSCAT's most used conversion).

mod common;
use common::{run, run_o3};

#[test]
fn integers_to_and_from_strings() {
    let src = r#"
PROGRAM p
VAR
  a : STRING; b : STRING; c : STRING; d : STRING; e : STRING; f : STRING; g : STRING;
  i : INT := -1234; dw : DWORD := 16#FFFFFFFF; li : LINT := LINT#-9223372036854775808;
  n1 : INT; n2 : DINT; n3 : INT; n4 : INT; n5 : BOOL; n6 : BOOL; n7 : UDINT;
  msg : STRING;
END_VAR
a := INT_TO_STRING(i);
b := DWORD_TO_STRING(dw);
c := LINT_TO_STRING(li);
d := BOOL_TO_STRING(i < 0);
e := INT_TO_STRING(0);
f := BYTE_TO_STRING(BYTE#200);
g := ULINT_TO_STRING(ULINT#18446744073709551615);
msg := CONCAT('n=', DINT_TO_STRING(42));
n1 := STRING_TO_INT('  -42abc');
n2 := STRING_TO_DINT('+100000');
n3 := STRING_TO_INT('');
n4 := STRING_TO_INT(a);
n5 := STRING_TO_BOOL('TRUE');
n6 := STRING_TO_BOOL('FALSE');
n7 := STRING_TO_UDINT(b);
END_PROGRAM
"#;
    for s in [run(src), run_o3(src)] {
        assert_eq!(s.str("a"), "-1234");
        assert_eq!(s.str("b"), "4294967295");
        assert_eq!(s.str("c"), "-9223372036854775808");
        assert_eq!(s.str("d"), "TRUE");
        assert_eq!(s.str("e"), "0");
        assert_eq!(s.str("f"), "200");
        assert_eq!(s.str("g"), "18446744073709551615");
        assert_eq!(s.str("msg"), "n=42");
        assert_eq!(s.i64("n1"), -42);
        assert_eq!(s.i64("n2"), 100000);
        assert_eq!(s.i64("n3"), 0);
        assert_eq!(s.i64("n4"), -1234);
        assert!(s.bool("n5"));
        assert!(!s.bool("n6"));
        assert_eq!(s.u64("n7"), 4294967295);
    }
}
