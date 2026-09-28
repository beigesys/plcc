// SPDX-License-Identifier: MPL-2.0

//! Integer temporaries are computed at least 32 bits wide, as CODESYS does ("the
//! compiler ... computes temporary results always with the native size that is
//! defined by the target device"), and only truncated when stored.
//!
//! plcc computed at the widest operand's width, so the answer depended on the
//! width of an untyped literal: `us * 2` was 510 on a USINT (the INT literal
//! widened it) but `ui * 2` wrapped to 65534 on a UINT, and `b + b` on a BYTE
//! stored into a DINT was 254.

mod common;
use common::{run, run_o3};

#[test]
fn narrow_arithmetic_is_not_truncated_before_the_store() {
    let src = r#"
PROGRAM p
VAR
  w : WORD := 65535; b : BYTE := 255; ui : UINT := 65535; us : USINT := 255;
  i : INT := 32767; si : SINT := -128;
  dw : DWORD; d1 : DINT; d2 : DINT; d3 : DINT; d4 : DINT; d5 : DINT;
  wrapped : INT; wb : BYTE; q : DINT;
END_VAR
dw := w + 1;
d1 := b + b;
d2 := ui * 2;
d3 := us * 2;
d4 := i + 1;
d5 := si - 1;
wrapped := i + 1;
wb := b + 1;
q := (b + 1) / 2;
END_PROGRAM
"#;
    for s in [run(src), run_o3(src)] {
        assert_eq!(s.u64("dw"), 65536, "the CODESYS help's own example");
        assert_eq!(s.i64("d1"), 510);
        assert_eq!(s.i64("d2"), 131070);
        assert_eq!(s.i64("d3"), 510);
        assert_eq!(s.i64("d4"), 32768);
        assert_eq!(s.i64("d5"), -129);
        assert_eq!(s.i64("wrapped"), -32768, "the store still truncates");
        assert_eq!(s.u64("wb"), 0);
        assert_eq!(s.i64("q"), 128);
    }
}
