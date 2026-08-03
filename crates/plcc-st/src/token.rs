// SPDX-License-Identifier: MPL-2.0

use logos::Logos;

fn parse_integer(lex: &logos::Lexer<Token>) -> Option<i128> {
    let s: String = lex.slice().chars().filter(|c| *c != '_').collect();
    // Handle base prefixes: 2#, 8#, 16#
    if let Some(rest) = s.strip_prefix("2#") {
        i128::from_str_radix(rest, 2).ok()
    } else if let Some(rest) = s.strip_prefix("8#") {
        i128::from_str_radix(rest, 8).ok()
    } else if let Some(rest) = s.strip_prefix("16#") {
        i128::from_str_radix(rest, 16).ok()
    } else {
        s.parse().ok()
    }
}

fn parse_real(lex: &logos::Lexer<Token>) -> Option<f64> {
    let s: String = lex.slice().chars().filter(|c| *c != '_').collect();
    s.parse().ok()
}

#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip r"[ \t\r\n]+")]
#[logos(skip(r"//[^\n]*", allow_greedy = true))]
#[logos(skip(r"\(\*([^*]|\*[^)])*\*\)", allow_greedy = true))]
pub enum Token {
    // ── Keywords: POUs ──
    #[token("PROGRAM", ignore(case))]
    Program,
    #[token("END_PROGRAM", ignore(case))]
    EndProgram,
    #[token("FUNCTION", ignore(case))]
    Function,
    #[token("END_FUNCTION", ignore(case))]
    EndFunction,
    #[token("FUNCTION_BLOCK", ignore(case))]
    // `FUNCTIONBLOCK` (no underscore) is a CODESYS 2.3 extension, NOT valid
    // IEC 61131-3 — Annex A B.1.5.2 spells the keyword `FUNCTION_BLOCK`.
    // Accepted for compatibility with legacy CODESYS exports; OSCAT Basic ships
    // seven of them (SRAMP, TMAX, TMIN, TOF_1, TP_1, TP_1D, SEQUENCE_64).
    // Only the opener has this variant — those same files close with the
    // standard `END_FUNCTION_BLOCK`, so `EndFunctionBlock` needs no alias.
    #[token("FUNCTIONBLOCK", ignore(case))]
    FunctionBlock,
    #[token("END_FUNCTION_BLOCK", ignore(case))]
    EndFunctionBlock,
    #[token("CLASS", ignore(case))]
    Class,
    #[token("END_CLASS", ignore(case))]
    EndClass,
    #[token("INTERFACE", ignore(case))]
    Interface,
    #[token("END_INTERFACE", ignore(case))]
    EndInterface,
    #[token("METHOD", ignore(case))]
    Method,
    #[token("END_METHOD", ignore(case))]
    EndMethod,
    #[token("EXTENDS", ignore(case))]
    Extends,
    #[token("IMPLEMENTS", ignore(case))]
    Implements,
    #[token("OVERRIDE", ignore(case))]
    Override,
    #[token("ABSTRACT", ignore(case))]
    Abstract,
    #[token("FINAL", ignore(case))]
    Final,
    #[token("PUBLIC", ignore(case))]
    Public,
    #[token("PRIVATE", ignore(case))]
    Private,
    #[token("PROTECTED", ignore(case))]
    Protected,
    #[token("INTERNAL", ignore(case))]
    Internal,

    // ── Keywords: Variable blocks ──
    #[token("VAR", ignore(case))]
    Var,
    #[token("END_VAR", ignore(case))]
    EndVar,
    #[token("VAR_INPUT", ignore(case))]
    VarInput,
    #[token("VAR_OUTPUT", ignore(case))]
    VarOutput,
    #[token("VAR_IN_OUT", ignore(case))]
    VarInOut,
    #[token("VAR_GLOBAL", ignore(case))]
    VarGlobal,
    #[token("VAR_EXTERNAL", ignore(case))]
    VarExternal,
    #[token("VAR_TEMP", ignore(case))]
    VarTemp,
    #[token("VAR_ACCESS", ignore(case))]
    VarAccess,
    #[token("VAR_CONFIG", ignore(case))]
    VarConfig,
    #[token("CONSTANT", ignore(case))]
    Constant,
    #[token("RETAIN", ignore(case))]
    Retain,
    #[token("NON_RETAIN", ignore(case))]
    NonRetain,
    #[token("AT", ignore(case))]
    At,
    #[token("R_EDGE", ignore(case))]
    REdge,
    #[token("F_EDGE", ignore(case))]
    FEdge,

    // ── Keywords: Types ──
    #[token("TYPE", ignore(case))]
    Type,
    #[token("END_TYPE", ignore(case))]
    EndType,
    #[token("STRUCT", ignore(case))]
    Struct,
    #[token("END_STRUCT", ignore(case))]
    EndStruct,
    #[token("UNION", ignore(case))]
    Union,
    #[token("END_UNION", ignore(case))]
    EndUnion,
    #[token("ARRAY", ignore(case))]
    Array,
    #[token("OF", ignore(case))]
    Of,
    #[token("POINTER", ignore(case))]
    Pointer,
    #[token("REF_TO", ignore(case))]
    RefTo,
    #[token("REFERENCE", ignore(case))]
    Reference,
    #[token("STRING", ignore(case))]
    StringType,
    #[token("WSTRING", ignore(case))]
    WstringType,

    // ── Elementary types ──
    #[token("BOOL", ignore(case))]
    Bool,
    #[token("BYTE", ignore(case))]
    Byte,
    #[token("WORD", ignore(case))]
    Word,
    #[token("DWORD", ignore(case))]
    Dword,
    #[token("LWORD", ignore(case))]
    Lword,
    #[token("SINT", ignore(case))]
    Sint,
    #[token("INT", ignore(case))]
    Int,
    #[token("DINT", ignore(case))]
    Dint,
    #[token("LINT", ignore(case))]
    Lint,
    #[token("USINT", ignore(case))]
    Usint,
    #[token("UINT", ignore(case))]
    Uint,
    #[token("UDINT", ignore(case))]
    Udint,
    #[token("ULINT", ignore(case))]
    Ulint,
    #[token("REAL", ignore(case))]
    Real,
    #[token("LREAL", ignore(case))]
    Lreal,
    #[token("CHAR", ignore(case))]
    Char,
    #[token("WCHAR", ignore(case))]
    Wchar,
    #[token("TIME", ignore(case))]
    Time,
    #[token("LTIME", ignore(case))]
    Ltime,
    #[token("DATE", ignore(case))]
    Date,
    #[token("TIME_OF_DAY", ignore(case))]
    TimeOfDay,
    #[token("TOD", ignore(case))]
    Tod,
    #[token("DATE_AND_TIME", ignore(case))]
    DateAndTime,
    #[token("DT", ignore(case))]
    Dt,
    #[token("LDATE", ignore(case))]
    Ldate,
    #[token("LTOD", ignore(case))]
    Ltod,
    #[token("LDT", ignore(case))]
    Ldt,

    // ── Statements ──
    #[token("IF", ignore(case))]
    If,
    #[token("THEN", ignore(case))]
    Then,
    #[token("ELSIF", ignore(case))]
    Elsif,
    #[token("ELSE", ignore(case))]
    Else,
    #[token("END_IF", ignore(case))]
    EndIf,
    #[token("CASE", ignore(case))]
    Case,
    #[token("END_CASE", ignore(case))]
    EndCase,
    #[token("FOR", ignore(case))]
    For,
    #[token("TO", ignore(case))]
    To,
    #[token("BY", ignore(case))]
    By,
    #[token("DO", ignore(case))]
    Do,
    #[token("END_FOR", ignore(case))]
    EndFor,
    #[token("WHILE", ignore(case))]
    While,
    #[token("END_WHILE", ignore(case))]
    EndWhile,
    #[token("REPEAT", ignore(case))]
    Repeat,
    #[token("UNTIL", ignore(case))]
    Until,
    #[token("END_REPEAT", ignore(case))]
    EndRepeat,
    #[token("EXIT", ignore(case))]
    Exit,
    #[token("CONTINUE", ignore(case))]
    Continue,
    #[token("RETURN", ignore(case))]
    Return,

    // ── Configuration ──
    #[token("CONFIGURATION", ignore(case))]
    Configuration,
    #[token("END_CONFIGURATION", ignore(case))]
    EndConfiguration,
    #[token("RESOURCE", ignore(case))]
    Resource,
    #[token("END_RESOURCE", ignore(case))]
    EndResource,
    #[token("TASK", ignore(case))]
    Task,
    #[token("WITH", ignore(case))]
    With,
    #[token("ON", ignore(case))]
    On,

    // ── Boolean literals ──
    #[token("TRUE", ignore(case))]
    True,
    #[token("FALSE", ignore(case))]
    False,

    // ── Operators ──
    #[token(":=")]
    Assign,
    #[token("=>")]
    OutputAssign,
    #[token("..")]
    DotDot,
    #[token(".")]
    Dot,
    #[token(",")]
    Comma,
    #[token(";")]
    Semicolon,
    #[token(":")]
    Colon,
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("**")]
    Power,
    #[token("=")]
    Equal,
    #[token("<>")]
    NotEqual,
    #[token("<")]
    Less,
    #[token("<=")]
    LessEqual,
    #[token(">")]
    Greater,
    #[token(">=")]
    GreaterEqual,
    #[token("&")]
    Ampersand,
    #[token("#")]
    Hash,
    #[token("^")]
    Caret,

    // ── Logical keywords ──
    #[token("AND", ignore(case))]
    And,
    #[token("OR", ignore(case))]
    Or,
    #[token("XOR", ignore(case))]
    Xor,
    #[token("NOT", ignore(case))]
    Not,
    #[token("MOD", ignore(case))]
    Mod,

    // ── Literals ──
    #[regex(
        r"(2#[01][01_]*|8#[0-7][0-7_]*|16#[0-9a-fA-F][0-9a-fA-F_]*|[0-9][0-9_]*)",
        parse_integer
    )]
    IntegerLiteral(i128),

    #[regex(
        r"[0-9][0-9_]*\.[0-9][0-9_]*([eE][+-]?[0-9][0-9_]*)?|[0-9][0-9_]*[eE][+-]?[0-9][0-9_]*",
        parse_real
    )]
    RealLiteral(f64),

    #[regex(r"'([^'\\]|\\.)*'")]
    StringLiteral,

    #[regex(r#""([^"\\]|\\.)*""#)]
    WstringLiteral,

    // ── Time / date literals ──
    //
    // Prefixes follow IEC 61131-3 3rd ed. Annex A B.1.2.3, which allows both the
    // abbreviated and the spelled-out keyword, each with an `L` (64-bit) form:
    //   duration      ::= ('T'   | 'LT'   | 'TIME'          | 'LTIME'         ) '#' ...
    //   date          ::= ('D'   | 'LD'   | 'DATE'          | 'LDATE'         ) '#' ...
    //   time_of_day   ::= ('TOD' | 'LTOD' | 'TIME_OF_DAY'   | 'LTIME_OF_DAY'  ) '#' ...
    //   date_and_time ::= ('DT'  | 'LDT'  | 'DATE_AND_TIME' | 'LDATE_AND_TIME') '#' ...
    //
    // Two things matter for correctness here:
    //  * Longest alternatives are listed first. logos resolves overlaps by
    //    longest match so ordering is belt-and-braces, but it keeps the intent
    //    obvious: `TOD` must never win over `TIME_OF_DAY`.
    //  * Every prefix word is also a type keyword (`x : TIME_OF_DAY;`). Those
    //    stay keywords because the literal patterns all require a following
    //    `#`, making the literal strictly longer whenever it applies.
    //
    // Time-of-day seconds are optional: CODESYS writes `TOD#12:00`, and the
    // standard's `day_second` is likewise not required to be present. Hour /
    // minute / second / month / day accept one or two digits (they are plain
    // integers in the grammar, not fixed-width fields).
    #[regex(r"(LTIME|TIME|LT|T)#[0-9a-zA-Z_.]+", ignore(case))]
    TimeLiteral,

    #[regex(r"(LDATE|DATE|LD|D)#[0-9]{4}-[0-9]{1,2}-[0-9]{1,2}", ignore(case))]
    DateLiteral,

    #[regex(
        r"(LTIME_OF_DAY|TIME_OF_DAY|LTOD|TOD)#[0-9]{1,2}:[0-9]{1,2}(:[0-9]{1,2}(\.[0-9]+)?)?",
        ignore(case)
    )]
    TodLiteral,

    #[regex(
        r"(LDATE_AND_TIME|DATE_AND_TIME|LDT|DT)#[0-9]{4}-[0-9]{1,2}-[0-9]{1,2}-[0-9]{1,2}:[0-9]{1,2}(:[0-9]{1,2}(\.[0-9]+)?)?",
        ignore(case)
    )]
    DtLiteral,

    // ── Direct representation ──
    #[regex(r"%[IiQqMm][XxBbWwDdLl]?[0-9]+(\.[0-9]+)*")]
    DirectVariable,

    // ── Pragmas ──
    #[regex(r"\{[^}]*\}")]
    Pragma,

    // ── Identifier (must come after all keywords) ──
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*")]
    Identifier,
}

impl std::fmt::Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Token::Identifier => write!(f, "identifier"),
            Token::IntegerLiteral(v) => write!(f, "integer({v})"),
            Token::RealLiteral(v) => write!(f, "real({v})"),
            Token::StringLiteral => write!(f, "string literal"),
            Token::WstringLiteral => write!(f, "wstring literal"),
            Token::Semicolon => write!(f, "';'"),
            Token::Colon => write!(f, "':'"),
            Token::Assign => write!(f, "':='"),
            Token::LParen => write!(f, "'('"),
            Token::RParen => write!(f, "')'"),
            Token::LBracket => write!(f, "'['"),
            Token::RBracket => write!(f, "']'"),
            Token::Comma => write!(f, "','"),
            Token::Dot => write!(f, "'.'"),
            Token::DotDot => write!(f, "'..'"),
            _ => write!(f, "{:?}", self),
        }
    }
}
