// SPDX-License-Identifier: MPL-2.0

//! Names from Beckhoff libraries plcc does not provide are reported as a
//! missing library, at their location; Tc2_Standard names resolve to plcc's
//! own standard blocks.

fn errors(src: &str) -> Vec<String> {
    let (unit, parse_errors) = plcc_st::parse(src);
    assert!(parse_errors.is_empty(), "{parse_errors:?}");
    let (_, errors) = plcc_hir::check(&unit);
    errors
        .iter()
        .filter(|e| !e.is_warning())
        .map(|e| e.to_string())
        .collect()
}

#[test]
fn beckhoff_library_names_are_reported_by_library() {
    let errs = errors(
        r#"
PROGRAM p
VAR
    fbTime : Tc2_Utilities.FB_LocalSystemTime;
    axis : AXIS_REF;
    s : STRING;
END_VAR
ADSLOGSTR(msgCtrlMask := ADSLOG_MSGTYPE_ERROR, msgFmtStr := 'x %s', strArg := s);
s := Tc2_Utilities.F_ToUCase(s);
s := Tc2_Other.Whatever(s);
END_PROGRAM
"#,
    );
    let joined = errs.join("\n");
    assert!(
        joined.contains("`FB_LocalSystemTime` is part of the Tc2_Utilities library"),
        "{joined}"
    );
    assert!(
        joined.contains("`AXIS_REF` is part of the Tc2_MC2 library"),
        "{joined}"
    );
    assert!(
        joined.contains("`ADSLOGSTR` is part of the Tc2_System library"),
        "{joined}"
    );
    assert!(
        joined.contains("`F_ToUCase` is part of the Tc2_Utilities library"),
        "{joined}"
    );
    assert!(joined.contains("the Tc2_Other library"), "{joined}");
}

#[test]
fn tc2_standard_names_are_plccs_own() {
    let errs = errors(
        r#"
FUNCTION_BLOCK TON
VAR_INPUT IN : BOOL; PT : TIME; END_VAR
VAR_OUTPUT Q : BOOL; END_VAR
END_FUNCTION_BLOCK
PROGRAM p
VAR t : Tc2_Standard.TON; n : INT; END_VAR
t(IN := TRUE, PT := T#1S);
n := Tc2_Standard.LEN('abc');
END_PROGRAM
"#,
    );
    assert!(errs.is_empty(), "{errs:?}");
}

#[test]
fn dynamic_memory_is_reported() {
    let errs = errors(
        r#"
PROGRAM p
VAR ptr : POINTER TO INT; END_VAR
ptr := __NEW(INT);
__DELETE(ptr);
END_PROGRAM
"#,
    );
    assert_eq!(errs.len(), 2, "{errs:?}");
    assert!(errs[0].contains("`__NEW` (dynamic memory) is not supported"));
}

#[test]
fn unknown_bases_are_reported() {
    let errs = errors(
        r#"
FUNCTION_BLOCK A EXTENDS NoSuchBase IMPLEMENTS I_Missing
END_FUNCTION_BLOCK
FUNCTION_BLOCK TcoContext EXTENDS TcoCore.TcoContext
END_FUNCTION_BLOCK
FUNCTION_BLOCK B EXTENDS A
END_FUNCTION_BLOCK
"#,
    );
    assert_eq!(errs.len(), 3, "{errs:?}");
    assert!(errs[0].contains("'NoSuchBase'"), "{errs:?}");
    assert!(errs[1].contains("'I_Missing'"), "{errs:?}");
    // Not taken as extending itself.
    assert!(errs[2].contains("'TcoCore.TcoContext'"), "{errs:?}");
}
