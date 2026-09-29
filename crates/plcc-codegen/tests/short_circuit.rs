// SPDX-License-Identifier: MPL-2.0

//! CODESYS `AND_THEN` / `OR_ELSE` evaluate their right operand only when the
//! left one does not decide the result.

mod common;
use common::{run, run_o3};

const SRC: &str = r#"
FUNCTION Bump : BOOL
VAR_IN_OUT n : INT; END_VAR
n := n + 1;
Bump := TRUE;
END_FUNCTION

PROGRAM p
VAR
    calls_and, calls_or, calls_plain : INT;
    a1, a2, o1, o2, safe : BOOL;
    ptr : POINTER TO INT;
END_VAR
a1 := FALSE AND_THEN Bump(calls_and);
a2 := TRUE AND_THEN Bump(calls_and);
o1 := TRUE OR_ELSE Bump(calls_or);
o2 := FALSE OR_ELSE Bump(calls_or);
a1 := a1 AND Bump(calls_plain);
(* the classic guard: never dereferences the null pointer *)
safe := ptr <> 0 AND_THEN ptr^ > 0;
safe := safe OR_ELSE (ptr = 0);
END_PROGRAM
"#;

#[test]
fn right_operand_runs_only_when_needed() {
    for s in [run(SRC), run_o3(SRC)] {
        assert!(!s.bool("a1"));
        assert!(s.bool("a2"));
        assert!(s.bool("o1"));
        assert!(s.bool("o2"));
        assert_eq!(
            s.i64("calls_and"),
            1,
            "AND_THEN skipped the call after FALSE"
        );
        assert_eq!(s.i64("calls_or"), 1, "OR_ELSE skipped the call after TRUE");
        assert_eq!(
            s.i64("calls_plain"),
            1,
            "plain AND still evaluates both sides"
        );
        assert!(s.bool("safe"));
    }
}
