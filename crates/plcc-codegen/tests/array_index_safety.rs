// SPDX-License-Identifier: MPL-2.0

//! Array subscripts.
//!
//! * An unsigned subscript was sign-extended: `arr[b]` with `b : BYTE := 200`
//!   addressed element -56, 56 elements *before* the array, and a write there
//!   corrupted whatever lay in front of it.
//! * A LINT subscript on a multi-dimensional array mixed i64 and i32 operands.
//! * An out-of-range subscript addressed memory outside the array (an
//!   out-of-bounds `inbounds` GEP, UB to the optimizer). Subscripts are now
//!   clamped into their dimension's bounds, as CODESYS's standard `CheckBounds`
//!   implicit check does.

mod common;
use common::{run, run_o3};

#[test]
fn unsigned_wide_and_out_of_range_subscripts() {
    let src = r#"
PROGRAM p
VAR
  guard1 : DINT := 11;
  arr : ARRAY[0..255] OF INT;
  guard2 : DINT := 22;
  small : ARRAY[1..4] OF INT;
  guard3 : DINT := 33;
  m : ARRAY[1..2, 0..2] OF INT;
  b : BYTE := 200; w : WORD := 65535; li : LINT := 2; big : INT := 100; neg : INT := -5;
  r1 : INT; r2 : INT; r3 : INT; r4 : INT; r5 : INT; r6 : INT;
END_VAR
arr[b] := 7;
r1 := arr[200];
small[big] := 9;
r2 := small[4];
small[neg] := 8;
r3 := small[1];
m[li, li] := 5;
r4 := m[2, 2];
arr[w] := 3;
r5 := arr[255];
r6 := guard1 + guard2 + guard3;
END_PROGRAM
"#;
    for s in [run(src), run_o3(src)] {
        assert_eq!(s.i64("r1"), 7, "BYTE subscript 200 is element 200");
        assert_eq!(s.i64("r2"), 9, "clamped to the last element");
        assert_eq!(s.i64("r3"), 8, "clamped to the first element");
        assert_eq!(s.i64("r4"), 5, "LINT subscripts on a 2-D array");
        assert_eq!(s.i64("r5"), 3, "WORD subscript 65535 clamped");
        assert_eq!(s.i64("r6"), 66, "nothing outside the arrays was written");
    }
}
