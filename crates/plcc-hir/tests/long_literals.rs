// SPDX-License-Identifier: MPL-2.0

//! `LTIME#`, `LDATE#`, `LTOD#`, `LDT#` literals have the 64-bit types. They were
//! typed TIME/DATE/TOD/DT, so `lt : LTIME := LTIME#1ns` was a type error.

use plcc_hir::check;
use plcc_st::parse;

#[test]
fn long_date_and_time_literals_type_check() {
    let src = "PROGRAM p
VAR lt : LTIME; ld : LDATE; lo : LTOD; lx : LDT; t : TIME; END_VAR
lt := LTIME#1ns;
lt := LT#5ms;
ld := LDATE#2024-01-02;
lo := LTOD#12:00:00;
lx := LDT#2024-01-02-03:04:05;
t := T#1s;
END_PROGRAM";
    let (unit, errors) = parse(src);
    assert!(errors.is_empty(), "{errors:?}");
    let diags = check(&unit).1;
    assert!(diags.is_empty(), "{diags:?}");
}
