// SPDX-License-Identifier: MPL-2.0

//! A FOR control variable that is a location rather than a name: an array
//! element or a structure field. IEC 61131-3 asks for a variable; TwinCAT
//! (and TcUnit, which iterates `FOR Offset[2] := 0 TO ...`) also takes these.

mod common;
use common::{run, run_o3};

const SRC: &str = r#"
TYPE ST_P : STRUCT k : INT; END_STRUCT END_TYPE
PROGRAM p
VAR
    idx : ARRAY[1..3] OF DINT;
    pt : ST_P;
    n : DINT;
    last1 : DINT;
    last2 : DINT;
    lastk : INT;
END_VAR
FOR idx[1] := 0 TO 2 DO
    FOR idx[2] := 0 TO 3 DO
        FOR pt.k := 1 TO 5 BY 2 DO
            n := n + 1;
        END_FOR
    END_FOR
END_FOR
last1 := idx[1];
last2 := idx[2];
lastk := pt.k;
END_PROGRAM
"#;

#[test]
fn array_element_and_field_counters() {
    for s in [run(SRC), run_o3(SRC)] {
        assert_eq!(s.i64("n"), 3 * 4 * 3);
        assert_eq!(s.i64("last1"), 3);
        assert_eq!(s.i64("last2"), 4);
        assert_eq!(s.i64("lastk"), 7);
    }
}
