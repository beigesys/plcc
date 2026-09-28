// SPDX-License-Identifier: MPL-2.0

//! FOR loops whose end value is at (or whose last step passes) the limit of the
//! control variable's type. The increment wrapped (`255 + 1` → 0 on a BYTE), the
//! bound test passed again, and the loop never ended — a PLC task that hangs.
//! CODESYS documents this as an endless loop; plcc ends the loop, and the control
//! variable holds the wrapped value CODESYS would leave in it.

mod common;
use common::{run, run_o3};

const SRC: &str = r#"
PROGRAM p
VAR
  i : INT; c1 : DINT; c2 : DINT; c3 : DINT; c4 : DINT; c5 : DINT; c6 : DINT;
  b : BYTE; si : SINT; st : INT := -2; ui : UINT; li : LINT; c7 : DINT; c8 : DINT;
  last1 : INT; last2 : INT; lastb : BYTE; lastsi : SINT;
END_VAR
FOR i := 10 TO 1 BY -3 DO c1 := c1 + 1; END_FOR;
last1 := i;
FOR b := 250 TO 255 DO c2 := c2 + 1; END_FOR;
lastb := b;
FOR si := 120 TO 127 BY 2 DO c3 := c3 + 1; END_FOR;
lastsi := si;
FOR i := 5 TO 1 BY st DO c4 := c4 + 1; END_FOR;
FOR ui := 65530 TO 65535 DO c5 := c5 + 1; END_FOR;
FOR i := 32760 TO 32767 DO c6 := c6 + 1; END_FOR;
last2 := i;
FOR si := -126 TO -128 BY -1 DO c7 := c7 + 1; END_FOR;
FOR li := LINT#9223372036854775806 TO LINT#9223372036854775807 DO c8 := c8 + 1; END_FOR;
END_PROGRAM
"#;

#[test]
fn loops_at_type_limits_terminate() {
    for s in [run(SRC), run_o3(SRC)] {
        assert_eq!(s.i64("c1"), 4);
        assert_eq!(s.i64("last1"), -2);
        assert_eq!(s.i64("c2"), 6);
        assert_eq!(s.u64("lastb"), 0, "wrapped, as in CODESYS");
        assert_eq!(s.i64("c3"), 4);
        assert_eq!(s.i64("lastsi"), -128);
        assert_eq!(s.i64("c4"), 3);
        assert_eq!(s.i64("c5"), 6);
        assert_eq!(s.i64("c6"), 8);
        assert_eq!(s.i64("last2"), -32768);
        assert_eq!(s.i64("c7"), 3);
        assert_eq!(s.i64("c8"), 2);
    }
}
