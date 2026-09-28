// SPDX-License-Identifier: MPL-2.0

//! CODESYS bit access `x.3` and the IEC partial access `x.%X3`, read and write, on
//! every integer / bit-string width, through struct fields, array elements and
//! output bindings. It was "does not resolve to a field" (about 18 OSCAT files).

mod common;
use common::{compile_error, run, run_o3};

#[test]
fn read_and_write_bits() {
    let src = r#"
TYPE S : STRUCT w : WORD; END_STRUCT END_TYPE
FUNCTION_BLOCK F
VAR_OUTPUT q : BOOL; END_VAR
q := TRUE;
END_FUNCTION_BLOCK
PROGRAM p
VAR
  b : BYTE := 16#05; w : WORD; d : DINT := -1; lw : LWORD;
  x0 : BOOL; x1 : BOOL; x2 : BOOL; x63 : BOOL; xi : BOOL;
  s : S; arr : ARRAY[0..2] OF BYTE;
  si : SINT := -128; us : USINT;
  f : F;
END_VAR
x0 := b.0;
x1 := b.1;
x2 := b.%X2;
w.15 := TRUE;
w.0 := x2;
d.31 := FALSE;
lw.63 := TRUE;
x63 := lw.63;
s.w.3 := TRUE;
arr[1].7 := TRUE;
b.%X7 := 1 > 0;
xi := si.7;
f(q => us.4);
END_PROGRAM
"#;
    for s in [run(src), run_o3(src)] {
        assert!(s.bool("x0"));
        assert!(!s.bool("x1"));
        assert!(s.bool("x2"));
        assert_eq!(s.u64("w"), 0x8001);
        assert_eq!(s.i64("d"), i32::MAX as i64);
        assert_eq!(s.u64("lw"), 1 << 63);
        assert!(s.bool("x63"));
        assert_eq!(s.u64("b"), 0x85);
        assert!(s.bool("xi"));
        assert_eq!(s.u64("us"), 0x10);
    }
}

#[test]
fn bit_index_beyond_the_width_is_an_error() {
    let src = r#"
PROGRAM p
VAR b : BYTE; x : BOOL; END_VAR
x := b.8;
END_PROGRAM
"#;
    let e = compile_error(src);
    assert!(e.contains("bit 8"), "{e}");
}
