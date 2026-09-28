// SPDX-License-Identifier: MPL-2.0

//! An assignment or call statement must end with `;`, as in CODESYS. A missing
//! one was accepted silently, so a typo split one statement into two:
//! `a := b c := d;` was two assignments, and `t := T#-1.5s;` (before negative
//! durations lexed) became `t := T#-1.5` followed by a call statement `s;`.

#[test]
fn missing_semicolon_is_an_error() {
    for src in [
        "PROGRAM p VAR a : INT; b : INT; c : INT; END_VAR a := b c := 1; END_PROGRAM",
        "PROGRAM p VAR a : INT; END_VAR a := 1 END_PROGRAM",
        "PROGRAM p VAR a : INT; END_VAR IF TRUE THEN a := 1 END_IF; END_PROGRAM",
    ] {
        let (_unit, errors) = plcc_st::parse(src);
        assert!(!errors.is_empty(), "`{src}` should need a ';'");
    }
}

#[test]
fn semicolon_after_structured_statements_stays_optional() {
    let src = "PROGRAM p VAR a : INT; END_VAR
IF TRUE THEN a := 1; END_IF
WHILE FALSE DO a := 2; END_WHILE
CASE a OF 1: a := 3; END_CASE
FOR a := 1 TO 2 DO ; END_FOR
END_PROGRAM";
    let (_unit, errors) = plcc_st::parse(src);
    assert!(errors.is_empty(), "{errors:?}");
}
