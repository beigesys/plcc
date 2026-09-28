// SPDX-License-Identifier: MPL-2.0

//! Elements of an `ARRAY OF <FUNCTION_BLOCK>` get their declared initial values.
//! They did not: the element-type probe treated the FB as the caller's own
//! variable and skipped its `_init`, so `fbs : ARRAY[1..3] OF Counter` ran every
//! element with `inc := 1` read as 0 (and a TON array with PT defaults as T#0s).

mod common;
use common::run;

#[test]
fn fb_array_elements_are_initialized_everywhere() {
    let src = r#"
FUNCTION_BLOCK Counter
VAR_INPUT inc : INT := 1; END_VAR
VAR_OUTPUT total : INT; END_VAR
VAR base : INT := 100; END_VAR
total := base + total + inc;
base := 0;
END_FUNCTION_BLOCK
TYPE Holder : STRUCT cs : ARRAY[0..1] OF Counter; END_STRUCT END_TYPE
FUNCTION_BLOCK Bank
VAR cs : ARRAY[1..2] OF Counter; END_VAR
VAR_OUTPUT s : INT; END_VAR
cs[2](); s := cs[2].total;
END_FUNCTION_BLOCK
VAR_GLOBAL gcs : ARRAY[1..2] OF Counter; END_VAR
PROGRAM p
VAR
  fbs : ARRAY[1..3] OF Counter;
  grid : ARRAY[1..2, 1..2] OF Counter;
  h : Holder;
  b : Bank;
  r1 : INT; r2 : INT; r3 : INT; r4 : INT; r5 : INT;
END_VAR
fbs[3]();
r1 := fbs[3].total;
grid[2, 2]();
r2 := grid[2, 2].total;
h.cs[1]();
r3 := h.cs[1].total;
b();
r4 := b.s;
gcs[2]();
r5 := gcs[2].total;
END_PROGRAM
"#;
    let s = run(src);
    for v in ["r1", "r2", "r3", "r4", "r5"] {
        assert_eq!(s.i64(v), 101, "{v}");
    }
}
