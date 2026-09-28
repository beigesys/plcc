// SPDX-License-Identifier: MPL-2.0

//! CASE branch labels other than `<int>:` / `<ident>:`, and error recovery inside a
//! CASE branch. A qualified-enumerator label (`Mode#Idle:`) used to send
//! `parse_case_branch_body` into an endless loop that pushed empty statements until
//! the process ran out of memory (the 38 GiB `plcc_st` test binary in CLAUDE.md).

use plcc_st::ast::*;

fn case_labels(src: &str) -> Vec<usize> {
    let (unit, errors) = plcc_st::parse(src);
    assert!(errors.is_empty(), "{errors:?}");
    let Declaration::Program(p) = &unit.declarations[0] else {
        panic!("expected a PROGRAM");
    };
    let StatementKind::Case { branches, .. } = &p.body[0].kind else {
        panic!("expected CASE");
    };
    branches.iter().map(|b| b.body.len()).collect()
}

#[test]
fn qualified_negative_and_typed_labels_start_branches() {
    let src = "PROGRAM p VAR x : INT; y : INT; END_VAR
CASE x OF
  Mode#Idle: y := 1;
  -1, -3..-2: y := 2;
  INT#5: y := 3;
  Mode.Run: y := 4;
END_CASE;
END_PROGRAM";
    assert_eq!(case_labels(src), vec![1, 1, 1, 1]);
}

#[test]
fn stray_colon_in_a_case_branch_terminates_with_an_error() {
    let src = "PROGRAM p VAR x : INT; y : INT; END_VAR
CASE x OF
  1: y := 1; : ;
END_CASE;
END_PROGRAM";
    let (_unit, errors) = plcc_st::parse(src);
    assert!(!errors.is_empty());
}

/// Tokens that neither start a label nor a statement, in a CASE branch list and a
/// TYPE block: each used to loop forever, allocating until the process died.
#[test]
fn recovery_loops_always_make_progress() {
    for src in [
        "PROGRAM p VAR x : INT; END_VAR CASE x OF VAR END_CASE; END_PROGRAM",
        "TYPE ) END_TYPE",
        "TYPE T : UNION ) END_UNION; END_TYPE",
    ] {
        let (_unit, errors) = plcc_st::parse(src);
        assert!(!errors.is_empty(), "`{src}` should report an error");
    }
}
