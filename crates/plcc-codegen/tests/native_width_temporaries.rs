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

#[test]
fn shifts_and_rotates_of_a_narrow_temporary_use_its_type_width() {
    let src = r#"
PROGRAM p
VAR b : BYTE := 16#80; r1 : BYTE; r2 : BYTE; w : WORD := 16#8000; r3 : WORD; r4 : DWORD; END_VAR
r1 := ROL(b + 0, 1);
r2 := ROR(b + 1, 1);
r3 := ROL(w + 0, 1);
r4 := SHL(b + 0, 1);
END_PROGRAM
"#;
    for s in [run(src), run_o3(src)] {
        assert_eq!(s.u64("r1"), 0x01, "rotated within 8 bits, not 32");
        assert_eq!(s.u64("r2"), 0xC0);
        assert_eq!(s.u64("r3"), 0x0001);
        assert_eq!(s.u64("r4"), 0, "SHL of a BYTE drops the top bit");
    }
}

#[test]
fn unary_minus_is_a_native_width_temporary() {
    let src = r#"
PROGRAM p
VAR us : USINT := 200; w : WORD := 1; si : SINT := -128; d1 : DINT; d2 : DINT; d3 : DINT; nw : WORD; ns : SINT; END_VAR
d1 := -us;
d2 := -w;
d3 := -si;
nw := -w;
ns := -si;
END_PROGRAM
"#;
    for s in [run(src), run_o3(src)] {
        assert_eq!(s.i64("d1"), -200);
        assert_eq!(s.i64("d2"), -1);
        assert_eq!(s.i64("d3"), 128);
        assert_eq!(s.u64("nw"), 0xFFFF, "the store still wraps");
        assert_eq!(s.i64("ns"), -128);
    }
}
