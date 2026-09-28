// SPDX-License-Identifier: MPL-2.0

//! A STRING declared without a length holds 80 characters, as in CODESYS and
//! TwinCAT. plcc used 256, so a value CODESYS truncates at 80 characters was
//! kept whole, and every default STRING took 257 bytes instead of 81.

mod common;
use common::run;

#[test]
fn default_string_is_80_characters() {
    let src = r#"
PROGRAM p
VAR s : STRING; w : WSTRING; n : INT; sz : UDINT; wsz : UDINT; long : STRING[200]; END_VAR
long := CONCAT('0123456789012345678901234567890123456789', '0123456789012345678901234567890123456789ABCDEFGHIJ');
s := long;
n := LEN(s);
sz := SIZEOF(s);
wsz := SIZEOF(w);
END_PROGRAM
"#;
    let st = run(src);
    assert_eq!(st.i64("n"), 80);
    assert!(st.str("s").ends_with("6789"));
    assert_eq!(st.u64("sz"), 81);
    assert_eq!(st.u64("wsz"), 162);
}
