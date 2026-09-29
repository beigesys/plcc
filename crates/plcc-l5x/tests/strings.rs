// SPDX-License-Identifier: MPL-2.0

//! Logix strings: the ASCII string and conversion instructions, string
//! compares, MOV between string types, and string literals in ST.

mod common;
use common::*;

/// The text of a Logix string tag.
fn text(plc: &Plc, tag: &str) -> String {
    let len = plc.get(&format!("{tag}.LEN"));
    (0..len)
        .map(|i| plc.get(&format!("{tag}.DATA[{i}]")) as u8 as char)
        .collect()
}

#[test]
fn string_instructions() {
    with_plc("strings.L5X", |plc| {
        assert_eq!(text(plc, "Hello"), "Hello", "L5K string data");
        assert_eq!(text(plc, "World"), " world", "decorated string data");
        plc.scan();
        assert_eq!(text(plc, "Joined"), "Hello world");
        assert_eq!(
            text(plc, "Short"),
            "Hello worl",
            "MOV truncates to the 10-character type"
        );
        assert!(plc.get_bool("lx__S_MINOR") || plc.get_bool("lx__S_V"));
        assert_eq!(text(plc, "Piece"), "world", "MID(Joined, 5, 7)");
        assert_eq!(text(plc, "Cut"), "world", "DELETE(Joined, 6, 1)");
        assert_eq!(text(plc, "Ins"), "He worldllo", "INSERT(Hello, World, 3)");
        assert_eq!(plc.get("Pos"), 7, "FIND");
        assert_eq!(text(plc, "Up"), "HELLO WORLD");
        assert_eq!(text(plc, "Num"), "-1234");
        assert_eq!(plc.get("Back"), -1234);
        assert!(!plc.get_bool("Same"));
        assert!(
            plc.get_bool("Before"),
            "' world' < 'Hello' by character code"
        );
    });
}

#[test]
fn strings_in_structured_text() {
    with_plc("strings.L5X", |plc| {
        plc.scan();
        assert_eq!(text(plc, "StLit"), "abc");
        assert!(plc.get_bool("StEq"));
        assert_eq!(text(plc, "StCopy"), "abc", "Str10 → STRING copy");
        assert_eq!(text(plc, "StJoin"), "abcabc");
    });
}
