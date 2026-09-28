// SPDX-License-Identifier: MPL-2.0

//! STRING values anywhere in an expression: literals as operands and arguments,
//! nested string functions, comparisons, string function results passed to
//! FUNCTIONs and FB inputs and stored into struct fields.
//!
//! The string functions used to work only as the whole right-hand side of an
//! assignment to a plain STRING variable with variable arguments; `LEN('abc')`
//! and `FIND(s, 'x')` compiled to a silent 0.

mod common;
use common::{run, run_o3, run_with};

const SRC: &str = r#"
TYPE R : STRUCT name : STRING[10]; END_STRUCT END_TYPE
FUNCTION Greet : STRING
VAR_INPUT who : STRING; END_VAR
Greet := CONCAT('Hi ', who);
END_FUNCTION
FUNCTION_BLOCK FB
VAR_INPUT s : STRING[20]; END_VAR
VAR_OUTPUT n : INT; END_VAR
n := LEN(s);
END_FUNCTION_BLOCK
PROGRAM p
VAR
  a : STRING := 'Hello'; b : STRING := 'World';
  c : STRING; d : STRING[5]; e : STRING; f : STRING;
  n1 : INT; n2 : INT; n3 : INT; n4 : INT;
  eq : BOOL; lt : BOOL; ne : BOOL; gt : BOOL; eq2 : BOOL; lt2 : BOOL;
  r : R; g : STRING; h : STRING; i : STRING; j : STRING;
  fb : FB; k : STRING; rn : STRING;
END_VAR
c := CONCAT(a, ' ', b, '!');
d := CONCAT(a, b);
e := MID(CONCAT(a, b), 3, 4);
f := LEFT('abcdef', 2);
n1 := LEN('abc');
n2 := LEN(CONCAT(a, b));
n3 := FIND(c, 'World');
eq := a = 'Hello';
lt := 'abc' < 'abd';
ne := a <> b;
gt := b > a;
eq2 := LEFT(a, 2) = 'He';
lt2 := 'ab' < 'abc';
r.name := CONCAT(a, b);
rn := r.name;
g := Greet('Bob');
h := Greet(CONCAT('X', 'Y'));
i := INSERT('SUXY', 'TUXY', 2);
j := DELETE('SUXY', 2, 2);
fb(s := CONCAT(a, b));
n4 := fb.n;
k := RIGHT(a, 3);
END_PROGRAM
"#;

#[test]
fn string_expressions() {
    for s in [run(SRC), run_o3(SRC)] {
        assert_eq!(s.str("c"), "Hello World!");
        assert_eq!(s.str("d"), "Hello", "truncated to STRING[5]");
        assert_eq!(s.str("e"), "loW", "MID(STR, LEN, POS) of a nested CONCAT");
        assert_eq!(s.str("f"), "ab");
        assert_eq!(s.i64("n1"), 3);
        assert_eq!(s.i64("n2"), 10);
        assert_eq!(s.i64("n3"), 7);
        assert!(s.bool("eq"));
        assert!(s.bool("lt"));
        assert!(s.bool("ne"));
        assert!(s.bool("gt"));
        assert!(s.bool("eq2"));
        assert!(s.bool("lt2"), "a prefix sorts first");
        assert_eq!(s.str("rn"), "HelloWorld");
        assert_eq!(s.str("g"), "Hi Bob");
        assert_eq!(s.str("h"), "Hi XY");
        assert_eq!(s.str("i"), "SUTUXYXY", "CODESYS INSERT example");
        assert_eq!(s.str("j"), "SY", "CODESYS DELETE example");
        assert_eq!(s.i64("n4"), 10);
        assert_eq!(s.str("k"), "llo");
    }
}

#[test]
fn a_string_function_may_read_its_own_destination() {
    let src = r#"
PROGRAM p
VAR m : STRING; END_VAR
m := CONCAT(m, 'x');
m := CONCAT('<', m);
END_PROGRAM
"#;
    let s = run_with(src, 2, 10, false);
    assert_eq!(s.str("m"), "<<xx");
}
