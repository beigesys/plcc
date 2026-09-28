// SPDX-License-Identifier: MPL-2.0

//! CODESYS `VAR_INST`: a METHOD variable stored in the instance, so it keeps its
//! value between calls (per instance), unlike an ordinary METHOD local. It did
//! not parse.

mod common;
use common::run;

#[test]
fn var_inst_keeps_its_value_per_instance() {
    let src = r#"
FUNCTION_BLOCK F
METHOD Count : INT
VAR_INST calls : INT := 10; edge : R_TRIG; END_VAR
VAR tmp : INT; END_VAR
tmp := tmp + 1;
calls := calls + tmp;
edge(CLK := calls > 11);
IF edge.Q THEN calls := calls + 100; END_IF;
Count := calls;
END_METHOD
METHOD Other : INT
VAR_INST calls : INT; END_VAR
calls := calls + 5;
Other := calls;
END_METHOD
END_FUNCTION_BLOCK
PROGRAM p
VAR a : F; b : F; r1 : INT; r2 : INT; r3 : INT; r4 : INT; END_VAR
r1 := a.Count();
r1 := a.Count();
r2 := b.Count();
r3 := a.Other();
r4 := a.Count();
END_PROGRAM
"#;
    let s = run(src);
    assert_eq!(s.i64("r1"), 112, "11, then 12 plus the rising edge's 100");
    assert_eq!(s.i64("r2"), 11, "b has its own copy");
    assert_eq!(s.i64("r3"), 5, "another method's VAR_INST of the same name is separate");
    assert_eq!(s.i64("r4"), 113, "the edge stays high: no second +100");
}
