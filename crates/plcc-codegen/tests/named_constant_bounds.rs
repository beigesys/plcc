// SPDX-License-Identifier: MPL-2.0

//! Array bounds and string lengths given by named constants:
//! `ARRAY[1..N] OF INT`, `STRING(LEN)`. Type resolution only understood literal
//! bounds; `ARRAY[1..N]` silently became ONE element (every access past it went
//! to the wrong memory) and `STRING(LEN)` the default 255 characters.

mod common;
use common::{compile_error, run};

#[test]
fn constant_bounds_and_lengths() {
    let src = r#"
VAR_GLOBAL CONSTANT L : INT := 3; N : INT := L + 1; END_VAR
TYPE Buf : ARRAY[0..N - 1] OF BYTE; END_TYPE
FUNCTION_BLOCK F
VAR CONSTANT M : INT := 5; END_VAR
VAR a : ARRAY[1..M] OF INT; END_VAR
VAR_OUTPUT sz : UDINT; last : INT; END_VAR
a[M] := 77;
last := a[M];
sz := SIZEOF(a);
END_FUNCTION_BLOCK
PROGRAM p
VAR CONSTANT K : INT := 2; END_VAR
VAR
  s : STRING(L); s2 : STRING[L]; arr : ARRAY[1..N] OF INT; arr2 : ARRAY[0..K, 1..N] OF INT;
  b : Buf; f : F;
  n1 : UDINT; n2 : UDINT; n3 : UDINT; n4 : UDINT; n5 : UDINT; fsz : UDINT; flast : INT;
  guard : INT := 11; r : INT;
END_VAR
s := 'abcdef';
s2 := 'abcdef';
arr[4] := 44;
r := arr[4];
n1 := SIZEOF(s);
n2 := SIZEOF(arr);
n3 := SIZEOF(arr2);
n4 := SIZEOF(s2);
n5 := SIZEOF(b);
f();
fsz := f.sz;
flast := f.last;
END_PROGRAM
"#;
    let s = run(src);
    assert_eq!(s.str("s"), "abc");
    assert_eq!(s.str("s2"), "abc");
    assert_eq!(s.i64("r"), 44);
    assert_eq!(s.u64("n1"), 4);
    assert_eq!(s.u64("n2"), 8);
    assert_eq!(s.u64("n3"), 24);
    assert_eq!(s.u64("n4"), 4);
    assert_eq!(s.u64("n5"), 4);
    assert_eq!(s.u64("fsz"), 10);
    assert_eq!(s.i64("flast"), 77);
    assert_eq!(s.i64("guard"), 11);
}

#[test]
fn a_bound_that_is_not_a_constant_is_an_error() {
    let src = r#"
PROGRAM p
VAR n : INT := 3; arr : ARRAY[1..n] OF INT; END_VAR
arr[1] := 1;
END_PROGRAM
"#;
    let e = compile_error(src);
    assert!(e.contains("not a constant"), "{e}");
}
