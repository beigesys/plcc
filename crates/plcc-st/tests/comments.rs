// SPDX-License-Identifier: MPL-2.0

//! Block comments: nesting (IEC 61131-3 3rd ed. Table 5, CODESYS, OSCAT's
//! `NESTEDCOMMENTS := 'Yes'`), `/* */`, and a `(*` comment ending in `**)`, which
//! the old regex could not match at all (`(* banner **)` was a lex error).

fn parses(src: &str) {
    let (_unit, errors) = plcc_st::parse(src);
    assert!(errors.is_empty(), "{src}: {errors:?}");
}

#[test]
fn block_comments() {
    parses("PROGRAM p VAR x : INT; END_VAR (* banner **) x := 1; END_PROGRAM");
    parses("PROGRAM p VAR x : INT; END_VAR (***********) x := 1; END_PROGRAM");
    parses("PROGRAM p VAR x : INT; (* outer (* inner *) still outer *) END_VAR x := 1; END_PROGRAM");
    parses("PROGRAM p VAR x : INT; END_VAR /* c-style * / */ x := 1; END_PROGRAM");
    parses("PROGRAM p VAR x : INT; END_VAR x := 1; // line (* not a block\nEND_PROGRAM");
}

#[test]
fn unterminated_comment_is_an_error() {
    let (_unit, errors) = plcc_st::parse("PROGRAM p (* never closed END_PROGRAM");
    assert!(!errors.is_empty());
}
