// SPDX-License-Identifier: MPL-2.0

//! CODESYS `REFERENCE TO`, `REF=` and `__ISVALIDREF`.
//!
//! `REFERENCE TO INT` was compiled as `POINTER TO INT`: `r := r + 10` moved the
//! address by 10 bytes and `w := r` stored the address — silently — and `REF=`
//! did not parse.

mod common;
use common::{run, run_o3};

#[test]
fn references_read_write_and_rebind() {
    let src = r#"
FUNCTION_BLOCK Acc
VAR_INPUT target : REFERENCE TO DINT; END_VAR
target := target + 100;
METHOD Bump
VAR_INPUT n : DINT; END_VAR
target := target + n;
END_METHOD
END_FUNCTION_BLOCK
PROGRAM p
VAR
  v : INT := 3; u : INT := 40; w : INT; w2 : INT;
  r : REFERENCE TO INT; r2 : REFERENCE TO INT; none : REFERENCE TO INT;
  ok1 : BOOL; ok2 : BOOL;
  d : DINT := 1; acc : Acc;
END_VAR
r REF= v;
r := r + 10;
w := r;
r REF= u;
r := r * 2;
r2 REF= r;
w2 := r2 + 1;
ok1 := __ISVALIDREF(r);
ok2 := __ISVALIDREF(none);
acc(target := d);
acc.Bump(5);
END_PROGRAM
"#;
    for s in [run(src), run_o3(src)] {
        assert_eq!(s.i64("v"), 13);
        assert_eq!(s.i64("w"), 13);
        assert_eq!(s.i64("u"), 80, "rebinding with REF= redirects later writes");
        assert_eq!(s.i64("w2"), 81, "a REF= to a reference binds its target");
        assert!(s.bool("ok1"));
        assert!(!s.bool("ok2"));
        assert_eq!(s.i64("d"), 106, "FB input reference, used in the body and a method");
    }
}
