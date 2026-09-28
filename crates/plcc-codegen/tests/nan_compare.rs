// SPDX-License-Identifier: MPL-2.0

//! `<>` on REAL/LREAL is IEEE "unordered or not equal": `NaN <> x` is TRUE for
//! every x, NaN included. It was lowered as ordered-not-equal, so `x <> x` —
//! the usual NaN test — was FALSE for a NaN, and `IF r <> 0.0` skipped a NaN.
//! `=`, `<`, `>`… stay FALSE for a NaN, as in IEEE 60559.

mod common;
use common::{run, run_o3};

#[test]
fn nan_is_not_equal_to_anything() {
    let src = r#"
PROGRAM p
VAR
  z : REAL := 0.0; nan : REAL; lnan : LREAL;
  ne_self : BOOL; ne_zero : BOOL; eq_self : BOOL; lt : BOOL; lne : BOOL; ne_num : BOOL;
END_VAR
nan := z / z;
lnan := REAL_TO_LREAL(nan);
ne_self := nan <> nan;
ne_zero := nan <> 0.0;
eq_self := nan = nan;
lt := nan < 1.0;
lne := lnan <> lnan;
ne_num := z <> 1.0;
END_PROGRAM
"#;
    for s in [run(src), run_o3(src)] {
        assert!(s.bool("ne_self"));
        assert!(s.bool("ne_zero"));
        assert!(!s.bool("eq_self"));
        assert!(!s.bool("lt"));
        assert!(s.bool("lne"));
        assert!(s.bool("ne_num"));
    }
}
