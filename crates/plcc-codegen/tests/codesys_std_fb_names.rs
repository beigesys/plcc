// SPDX-License-Identifier: MPL-2.0

//! The CODESYS names of the standard FB inputs — `SR(SET1, RESET)`,
//! `RS(SET, RESET1)`, `CTU(.., RESET, ..)`, `CTD(.., LOAD, ..)`,
//! `CTUD(.., RESET, LOAD, ..)` — are accepted next to the IEC 61131-3 names the
//! bundled blocks use (`S1`/`R`, `S`/`R1`, `R`, `LD`). They were "FB field
//! 'RESET' not found", so no CODESYS program using a counter compiled.

mod common;
use common::run_with;

#[test]
fn codesys_parameter_names_of_standard_fbs() {
    let src = r#"
PROGRAM p
VAR
  n : INT; sr1 : SR; rs1 : RS; cu : CTU; cd : CTD; ud : CTUD;
  srq : BOOL; rsq : BOOL; cucv : INT; cdcv : INT; udcv : INT; cuq : BOOL;
END_VAR
n := n + 1;
sr1(SET1 := n = 1, RESET := n = 3); srq := sr1.Q1;
rs1(SET := TRUE, RESET1 := TRUE); rsq := rs1.Q1;
cu(CU := (n MOD 2) = 0, RESET := n = 5, PV := 2); cucv := cu.CV; cuq := cu.Q;
cd(CD := (n MOD 2) = 0, LOAD := n = 1, PV := 10); cdcv := cd.CV;
ud(CU := (n MOD 2) = 0, CD := FALSE, RESET := FALSE, LOAD := n = 1, PV := 7); udcv := ud.CV;
END_PROGRAM
"#;
    // Scans 1..4: CU/CD edges at n = 2 and 4.
    let s = run_with(src, 4, 10, false);
    assert!(!s.bool("srq"), "SR reset at n = 3");
    assert!(!s.bool("rsq"), "RS is reset-dominant");
    assert_eq!(s.i64("cucv"), 2);
    assert!(s.bool("cuq"));
    assert_eq!(s.i64("cdcv"), 8, "loaded with 10, two edges");
    assert_eq!(s.i64("udcv"), 9, "loaded with 7, two up edges");
    let s = run_with(src, 5, 10, false);
    assert_eq!(s.i64("cucv"), 0, "CTU RESET at n = 5");
}
