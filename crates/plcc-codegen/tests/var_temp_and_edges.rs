// SPDX-License-Identifier: MPL-2.0

//! * VAR_TEMP is initialized on every call (IEC 61131-3 §6.5.2). In an FB or a
//!   PROGRAM it lived in the state struct and kept its value between calls, so
//!   `tmp : INT := 5; tmp := tmp + 1;` counted 6, 7, 8…
//! * `IN : BOOL R_EDGE` / `F_EDGE` (IEC 61131-3 Table 40): inside the body the
//!   input reads as its rising / falling edge. The qualifier was parsed and
//!   ignored, so the input read TRUE on every call the signal was high.

mod common;
use common::run_with;

#[test]
fn var_temp_is_reinitialized_and_edge_inputs_detect_edges() {
    let src = r#"
FUNCTION_BLOCK F
VAR_INPUT clk : BOOL R_EDGE; fclk : BOOL F_EDGE; END_VAR
VAR_OUTPUT n : INT; nf : INT; t : INT; raw : BOOL; END_VAR
VAR_TEMP tmp : INT := 5; arr : ARRAY[0..1] OF INT; END_VAR
IF clk THEN n := n + 1; END_IF;
IF fclk THEN nf := nf + 1; END_IF;
tmp := tmp + 1;
arr[1] := arr[1] + 1;
t := tmp + arr[1];
END_FUNCTION_BLOCK
PROGRAM p
VAR k : INT; f : F; n : INT; nf : INT; t : INT; pt : INT; outer : BOOL; END_VAR
VAR_TEMP ptmp : INT; END_VAR
k := k + 1;
f(clk := k >= 2 AND k <= 3, fclk := k >= 2 AND k <= 3);
f(clk := k >= 2 AND k <= 3, fclk := k >= 2 AND k <= 3);
n := f.n;
nf := f.nf;
t := f.t;
outer := f.clk;
ptmp := ptmp + 1;
pt := ptmp;
END_PROGRAM
"#;
    // Scans 1..5: the input is high on scans 2 and 3, each scan calls twice.
    let s = run_with(src, 5, 10, false);
    assert_eq!(s.i64("n"), 1, "one rising edge");
    assert_eq!(s.i64("nf"), 1, "one falling edge");
    assert_eq!(s.i64("t"), 7, "VAR_TEMP starts from its initial value every call");
    assert_eq!(s.i64("pt"), 1, "a PROGRAM's VAR_TEMP too");
    assert!(!s.bool("outer"));
}
