// SPDX-License-Identifier: MPL-2.0

//! Integer operations whose LLVM lowering was undefined behaviour.
//!
//! * `x / 0`, `x MOD 0`, `MIN / -1`: `sdiv`/`srem` raised SIGFPE on x86 and killed
//!   the whole process (the JIT test itself), returned 0 on ARM, and were UB for
//!   the optimizer. Now: division/MOD by zero is 0, `MIN / -1` wraps to MIN.
//! * `SHL`/`SHR` by the width of IN or more: `shl`/`lshr` poison; x86 masked the
//!   count, so `SHL(dint, 32)` returned the input. An N wider than IN was
//!   truncated first, so `SHL(byte, 256)` shifted by 0. Now 0.

mod common;
use common::{run, run_o3};

const DIV: &str = r#"
PROGRAM p
VAR
  z : INT := 0; m1 : INT := -1; mn : INT := -32768; dz : DINT := 0;
  a : INT; b : INT; c : INT; d : INT; e : DINT; f : UDINT; g : DINT; h : LINT;
  lmn : LINT := LINT#-9223372036854775808; lm1 : LINT := -1;
  u : UDINT := 7;
END_VAR
a := 10 / z;
b := 10 MOD z;
c := mn / m1;
d := mn MOD m1;
e := -7 / dz;
f := u / UDINT#0;
g := -7 MOD 3;
h := lmn / lm1;
END_PROGRAM
"#;

#[test]
fn division_by_zero_and_min_over_minus_one_are_defined() {
    for s in [run(DIV), run_o3(DIV)] {
        assert_eq!(s.i64("a"), 0);
        assert_eq!(s.i64("b"), 0);
        assert_eq!(s.i64("c"), -32768, "MIN / -1 wraps");
        assert_eq!(s.i64("d"), 0);
        assert_eq!(s.i64("e"), 0);
        assert_eq!(s.u64("f"), 0);
        assert_eq!(s.i64("g"), -1, "MOD keeps the dividend's sign");
        assert_eq!(s.i64("h"), i64::MIN);
    }
}

const SHIFTS: &str = r#"
PROGRAM p
VAR
  d : DINT := -7; b : BYTE := 16#81; n : INT := 256; big : DINT := 40; neg : INT := -1;
  r1 : DINT; r2 : BYTE; r3 : DINT; r4 : BYTE; r5 : BYTE; r6 : LWORD; r7 : BYTE; r8 : BYTE;
END_VAR
r1 := SHL(d, 32);
r2 := SHL(b, n);
r3 := SHR(d, big);
r4 := SHL(b, neg);
r5 := SHL(b, 7);
r6 := SHL(LWORD#1, 63);
r7 := ROL(b, 9);
r8 := SHR(b, 7);
END_PROGRAM
"#;

#[test]
fn shifting_by_the_width_or_more_is_zero() {
    for s in [run(SHIFTS), run_o3(SHIFTS)] {
        assert_eq!(s.i64("r1"), 0);
        assert_eq!(s.u64("r2"), 0);
        assert_eq!(s.i64("r3"), 0);
        assert_eq!(s.u64("r4"), 0, "a negative count shifts everything out");
        assert_eq!(s.u64("r5"), 0x80);
        assert_eq!(s.u64("r6"), 1 << 63);
        assert_eq!(s.u64("r7"), 0x03, "rotation counts are modulo the width");
        assert_eq!(s.u64("r8"), 1);
    }
}
