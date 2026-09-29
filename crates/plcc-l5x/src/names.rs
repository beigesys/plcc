// SPDX-License-Identifier: MPL-2.0

//! Logix names → ST identifiers.
//!
//! Logix names are letters, digits and single underscores, and may not contain
//! two underscores in a row (1756-RM014 / Studio 5000 naming rules). Every name
//! plcc synthesizes therefore contains `__` and can never collide with a user
//! name: hidden variables are `lx__…`, and a Logix name that is an ST keyword
//! (a tag called `DINT`, `Time` or `Step`) gets a `__` suffix. Module-defined
//! names (`Local:1:I`, `AB:1769_DI16:I:0`) turn each `:` into `__`.

/// Words the plcc-st lexer reserves (case-insensitive).
const KEYWORDS: &[&str] = &[
    "PROGRAM",
    "END_PROGRAM",
    "FUNCTION",
    "END_FUNCTION",
    "FUNCTION_BLOCK",
    "FUNCTIONBLOCK",
    "END_FUNCTION_BLOCK",
    "CLASS",
    "END_CLASS",
    "INTERFACE",
    "END_INTERFACE",
    "METHOD",
    "END_METHOD",
    "EXTENDS",
    "IMPLEMENTS",
    "OVERRIDE",
    "ABSTRACT",
    "FINAL",
    "PUBLIC",
    "PRIVATE",
    "PROTECTED",
    "INTERNAL",
    "VAR",
    "END_VAR",
    "VAR_INPUT",
    "VAR_OUTPUT",
    "VAR_IN_OUT",
    "VAR_GLOBAL",
    "VAR_EXTERNAL",
    "VAR_TEMP",
    "VAR_INST",
    "VAR_ACCESS",
    "VAR_CONFIG",
    "CONSTANT",
    "RETAIN",
    "NON_RETAIN",
    "AT",
    "R_EDGE",
    "F_EDGE",
    "TYPE",
    "END_TYPE",
    "STRUCT",
    "END_STRUCT",
    "UNION",
    "END_UNION",
    "ARRAY",
    "OF",
    "POINTER",
    "REF_TO",
    "REFERENCE",
    "STRING",
    "WSTRING",
    "BOOL",
    "BYTE",
    "WORD",
    "DWORD",
    "LWORD",
    "SINT",
    "INT",
    "DINT",
    "LINT",
    "USINT",
    "UINT",
    "UDINT",
    "ULINT",
    "REAL",
    "LREAL",
    "CHAR",
    "WCHAR",
    "TIME",
    "LTIME",
    "DATE",
    "TIME_OF_DAY",
    "TOD",
    "DATE_AND_TIME",
    "DT",
    "LDATE",
    "LTOD",
    "LDT",
    "IF",
    "THEN",
    "ELSIF",
    "ELSE",
    "END_IF",
    "CASE",
    "END_CASE",
    "FOR",
    "TO",
    "BY",
    "DO",
    "END_FOR",
    "WHILE",
    "END_WHILE",
    "REPEAT",
    "UNTIL",
    "END_REPEAT",
    "EXIT",
    "CONTINUE",
    "RETURN",
    "CONFIGURATION",
    "END_CONFIGURATION",
    "RESOURCE",
    "END_RESOURCE",
    "TASK",
    "WITH",
    "ON",
    "TRUE",
    "FALSE",
    "AND",
    "OR",
    "XOR",
    "NOT",
    "MOD",
    // Not lexer keywords, but names plcc gives a meaning of its own.
    "THIS",
    "SUPER",
    "PRINT",
    "MONOTONIC_NS",
    "PLCC_MONOTONIC_NS",
    "REF",
    "ADR",
    "SIZEOF",
];

pub(crate) fn is_keyword(name: &str) -> bool {
    KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(name))
}

/// The ST identifier for a Logix name.
pub(crate) fn ident(name: &str) -> String {
    // `:` (module tags) and anything else ST cannot spell (tool-made exports
    // contain names like `New Program`) become `__`.
    let mut s = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            s.push(c);
        } else {
            s.push_str("__");
        }
    }
    if s.starts_with(|c: char| c.is_ascii_digit()) {
        s.insert_str(0, "lx__");
    }
    if is_keyword(&s) {
        s.push_str("__");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_names_get_a_suffix_that_parses() {
        for k in KEYWORDS {
            let id = ident(k);
            let src = format!("PROGRAM p VAR {id} : INT; END_VAR {id} := 1; END_PROGRAM");
            let (_, errs) = plcc_st::parse(&src);
            assert!(errs.is_empty(), "{k} -> {id}: {errs:?}");
        }
        assert_eq!(ident("Local:1:I"), "Local__1__I");
        assert_eq!(ident("Motor"), "Motor");
    }
}
