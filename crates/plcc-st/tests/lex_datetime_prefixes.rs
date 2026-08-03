// SPDX-License-Identifier: MPL-2.0

//! Lexer coverage for IEC 61131-3 date/time literal prefixes (Annex A B.1.2.3)
//! and for the CODESYS 2.3 `FUNCTIONBLOCK` POU opener.
//!
//! These tests assert the exact token stream, not just "no error", because the
//! failure mode being guarded against is silent: an unrecognised prefix falls
//! through to `Identifier` and the parser then reports a confusing downstream
//! error (or, worse, accepts something wrong).

use logos::Logos;
use plcc_st::token::Token;
use plcc_st::{Declaration, ExpressionKind, StatementKind, VarBlockKind};

/// Lex `src` into a `Vec<Token>`, panicking on any lexer error so that a
/// tokenisation failure cannot be mistaken for an unexpected-token assertion.
fn lex(src: &str) -> Vec<Token> {
    Token::lexer(src)
        .map(|r| r.unwrap_or_else(|e| panic!("lexer error in {src:?}: {e:?}")))
        .collect()
}

/// Lex `src` and additionally return the matched slice for each token.
fn lex_slices(src: &str) -> Vec<(Token, String)> {
    let mut lexer = Token::lexer(src);
    let mut out = Vec::new();
    while let Some(res) = lexer.next() {
        let tok = res.unwrap_or_else(|e| panic!("lexer error in {src:?}: {e:?}"));
        out.push((tok, lexer.slice().to_string()));
    }
    out
}

// ── Duration literals: T# / TIME# / LT# / LTIME# ──

#[test]
fn duration_literal_all_prefixes() {
    for src in [
        "T#5s",
        "t#5s",
        "TIME#5s",
        "time#5s",
        "Time#5s",
        "LT#5s",
        "LTIME#5s",
        "ltime#5s",
        "T#1d2h3m4s5ms",
        "TIME#1d2h3m4s5ms",
    ] {
        assert_eq!(
            lex_slices(src),
            vec![(Token::TimeLiteral, src.to_string())],
            "expected {src:?} to lex as one TimeLiteral covering the whole input"
        );
    }
}

// ── Date literals: D# / DATE# / LD# / LDATE# ──

#[test]
fn date_literal_all_prefixes() {
    for src in [
        "D#2024-01-01",
        "d#2024-01-01",
        "DATE#2024-01-01",
        "date#2024-01-01",
        "LD#2024-01-01",
        "LDATE#2024-01-01",
        "ldate#2024-01-01",
    ] {
        assert_eq!(
            lex_slices(src),
            vec![(Token::DateLiteral, src.to_string())],
            "expected {src:?} to lex as one DateLiteral covering the whole input"
        );
    }
}

#[test]
fn date_literal_single_digit_month_and_day() {
    // IEC `day_month`/`day_day` are plain integers, so `D#1970-1-1` is legal.
    assert_eq!(
        lex_slices("D#1970-1-1"),
        vec![(Token::DateLiteral, "D#1970-1-1".to_string())]
    );
}

// ── Time-of-day literals: TOD# / TIME_OF_DAY# / LTOD# / LTIME_OF_DAY# ──

#[test]
fn tod_literal_all_prefixes() {
    for src in [
        "TOD#12:00:00",
        "tod#12:00:00",
        "TIME_OF_DAY#12:00:00",
        "time_of_day#12:00:00",
        "LTOD#12:00:00",
        "ltod#12:00:00",
        "LTIME_OF_DAY#12:00:00",
    ] {
        assert_eq!(
            lex_slices(src),
            vec![(Token::TodLiteral, src.to_string())],
            "expected {src:?} to lex as one TodLiteral covering the whole input"
        );
    }
}

#[test]
fn tod_literal_seconds_are_optional() {
    // CODESYS (and OSCAT's DT_TO_STRF.EXP) write `TOD#12:00` with no seconds.
    for src in [
        "TOD#12:00",
        "TIME_OF_DAY#12:00",
        "LTOD#12:00",
        "TOD#12:00:00.125",
        "TIME_OF_DAY#12:00:00.125",
    ] {
        assert_eq!(
            lex_slices(src),
            vec![(Token::TodLiteral, src.to_string())],
            "expected {src:?} to lex as one TodLiteral covering the whole input"
        );
    }
}

// ── Date-and-time literals: DT# / DATE_AND_TIME# / LDT# / LDATE_AND_TIME# ──

#[test]
fn dt_literal_all_prefixes() {
    for src in [
        "DT#2024-01-01-12:00:00",
        "dt#2024-01-01-12:00:00",
        "DATE_AND_TIME#2024-01-01-12:00:00",
        "date_and_time#2024-01-01-12:00:00",
        "LDT#2024-01-01-12:00:00",
        "ldt#2024-01-01-12:00:00",
        "LDATE_AND_TIME#2024-01-01-12:00:00",
        "DT#2024-01-01-12:00:00.5",
    ] {
        assert_eq!(
            lex_slices(src),
            vec![(Token::DtLiteral, src.to_string())],
            "expected {src:?} to lex as one DtLiteral covering the whole input"
        );
    }
}

// ── Keyword / literal disambiguation ──
//
// `TIME`, `TIME_OF_DAY`, `DATE`, `DATE_AND_TIME`, `TOD`, `DT`, `LTOD`, `LDT`,
// `LDATE`, `LTIME` are also type keywords. logos picks the longest match, so a
// bare keyword must stay a keyword and the same word followed by `#...` must
// become a literal. Both directions are asserted here.

#[test]
fn bare_datetime_type_keywords_are_still_keywords() {
    let cases: &[(&str, Token)] = &[
        ("TIME", Token::Time),
        ("LTIME", Token::Ltime),
        ("DATE", Token::Date),
        ("LDATE", Token::Ldate),
        ("TIME_OF_DAY", Token::TimeOfDay),
        ("TOD", Token::Tod),
        ("LTOD", Token::Ltod),
        ("DATE_AND_TIME", Token::DateAndTime),
        ("DT", Token::Dt),
        ("LDT", Token::Ldt),
    ];
    for (word, expected) in cases {
        assert_eq!(
            lex(word),
            vec![expected.clone()],
            "bare {word} must lex as the type keyword"
        );
        // ...and in the position it actually appears in: `x : <TYPE> ;`
        let decl = format!("x : {word} ;");
        assert_eq!(
            lex(&decl),
            vec![
                Token::Identifier,
                Token::Colon,
                expected.clone(),
                Token::Semicolon
            ],
            "declaration `{decl}` must keep {word} as a type keyword"
        );
    }
}

#[test]
fn datetime_type_keywords_parse_in_var_block() {
    let src = "\
FUNCTION_BLOCK dtypes
VAR
  a : TIME;
  b : LTIME;
  c : DATE;
  d : LDATE;
  e : TIME_OF_DAY;
  f : TOD;
  g : LTOD;
  h : DATE_AND_TIME;
  i : DT;
  j : LDT;
END_VAR
END_FUNCTION_BLOCK
";
    let (unit, errors) = plcc_st::parse(src);
    assert!(errors.is_empty(), "unexpected parse errors: {errors:#?}");
    let Declaration::FunctionBlock(fb) = &unit.declarations[0] else {
        panic!("expected a FUNCTION_BLOCK declaration");
    };
    assert_eq!(fb.var_blocks.len(), 1);
    assert_eq!(fb.var_blocks[0].kind, VarBlockKind::Var);
    let names: Vec<&str> = fb.var_blocks[0]
        .declarations
        .iter()
        .map(|d| d.name.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec!["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"]
    );
}

#[test]
fn literal_prefixes_do_not_shadow_shorter_keywords() {
    // The dangerous case: alternation order. `TOD` must not win over
    // `TIME_OF_DAY`, and `T`/`D`/`DT` must not win over their longer siblings.
    assert_eq!(
        lex("TIME_OF_DAY#12:00:00"),
        vec![Token::TodLiteral],
        "TIME_OF_DAY#... must be one TodLiteral, not TimeOfDay + junk"
    );
    assert_eq!(
        lex("DATE_AND_TIME#2024-01-01-12:00:00"),
        vec![Token::DtLiteral],
        "DATE_AND_TIME#... must be one DtLiteral"
    );
    assert_eq!(lex("LTOD#12:00:00"), vec![Token::TodLiteral]);
    assert_eq!(lex("LDT#2024-01-01-12:00:00"), vec![Token::DtLiteral]);
    assert_eq!(lex("LTIME#5s"), vec![Token::TimeLiteral]);
    assert_eq!(lex("LDATE#2024-01-01"), vec![Token::DateLiteral]);
}

#[test]
fn datetime_literals_in_expression_context() {
    let src = "\
PROGRAM p
VAR
  td : TOD;
  s : STRING;
END_VAR
IF td >= TOD#12:00 THEN s := 'PM'; ELSE s := 'AM'; END_IF;
END_PROGRAM
";
    let (unit, errors) = plcc_st::parse(src);
    assert!(errors.is_empty(), "unexpected parse errors: {errors:#?}");
    let Declaration::Program(prog) = &unit.declarations[0] else {
        panic!("expected a PROGRAM declaration");
    };
    let StatementKind::If { condition, .. } = &prog.body[0].kind else {
        panic!("expected an IF statement, got {:#?}", prog.body[0]);
    };
    let ExpressionKind::BinaryOp { right, .. } = &condition.kind else {
        panic!("expected a binary comparison, got {:#?}", condition.kind);
    };
    match &right.kind {
        ExpressionKind::TodLiteral(raw) => assert_eq!(raw, "TOD#12:00"),
        other => panic!("expected TodLiteral(\"TOD#12:00\"), got {other:#?}"),
    }
}

// ── CODESYS `FUNCTIONBLOCK` opener ──

#[test]
fn codesys_functionblock_opener_lexes_as_function_block() {
    for word in ["FUNCTIONBLOCK", "functionblock", "FunctionBlock"] {
        assert_eq!(
            lex(word),
            vec![Token::FunctionBlock],
            "{word} must lex as the FunctionBlock keyword, not an Identifier"
        );
    }
    // The standard spelling must be unaffected.
    for word in ["FUNCTION_BLOCK", "function_block"] {
        assert_eq!(lex(word), vec![Token::FunctionBlock]);
    }
    // There is no `ENDFUNCTIONBLOCK`; only the opener has the variant.
    assert_eq!(lex("END_FUNCTION_BLOCK"), vec![Token::EndFunctionBlock]);
}

#[test]
fn codesys_functionblock_opener_parses_with_standard_closer() {
    // Shape taken from OSCAT SRAMP.EXP: `FUNCTIONBLOCK` open, `END_FUNCTION_BLOCK` close.
    let src = "\
FUNCTIONBLOCK SRAMP
VAR_INPUT
  IN : REAL;
  RUN : BOOL;
END_VAR
VAR_OUTPUT
  OUT : REAL;
END_VAR
OUT := IN;
END_FUNCTION_BLOCK
";
    let (unit, errors) = plcc_st::parse(src);
    assert!(errors.is_empty(), "unexpected parse errors: {errors:#?}");
    assert_eq!(unit.declarations.len(), 1);
    let Declaration::FunctionBlock(fb) = &unit.declarations[0] else {
        panic!(
            "expected a FunctionBlock declaration, got {:#?}",
            unit.declarations[0]
        );
    };
    assert_eq!(fb.name.name, "SRAMP");
    assert_eq!(fb.var_blocks.len(), 2);
    assert_eq!(fb.var_blocks[0].kind, VarBlockKind::VarInput);
    assert_eq!(fb.var_blocks[1].kind, VarBlockKind::VarOutput);
    assert_eq!(fb.body.len(), 1);
}

#[test]
fn standard_function_block_still_parses() {
    let src = "\
FUNCTION_BLOCK std
VAR_INPUT
  IN : REAL;
END_VAR
END_FUNCTION_BLOCK
";
    let (unit, errors) = plcc_st::parse(src);
    assert!(errors.is_empty(), "unexpected parse errors: {errors:#?}");
    let Declaration::FunctionBlock(fb) = &unit.declarations[0] else {
        panic!("expected a FunctionBlock declaration");
    };
    assert_eq!(fb.name.name, "std");
}
