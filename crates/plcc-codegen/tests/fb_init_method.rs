// SPDX-License-Identifier: MPL-2.0

//! CODESYS FB_init: run once at instance initialization, after the instance's
//! declared initial values, with the arguments written at the declaration.

mod common;
use common::{compile_error, run_with};

const SRC: &str = r#"
FUNCTION_BLOCK Buffer
VAR
    size : INT := 4;
    retains, copy : BOOL;
    inits : INT;
    owner_id : INT;
END_VAR
METHOD FB_init : BOOL
VAR_INPUT
    bInitRetains : BOOL;
    bInCopyCode : BOOL;
    nSize : INT;
    nOwner : INT;
END_VAR
    IF nSize > 0 THEN
        size := nSize;
    END_IF
    retains := bInitRetains;
    copy := bInCopyCode;
    owner_id := nOwner;
    inits := inits + 1;
    FB_init := TRUE;
END_METHOD
END_FUNCTION_BLOCK

FUNCTION_BLOCK Owner
VAR
    id : INT := 7;
    child : Buffer(nSize := 32, nOwner := 70);
END_VAR
END_FUNCTION_BLOCK

VAR_GLOBAL
    g : Buffer(5, 1);
END_VAR

PROGRAM p
VAR
    plain : Buffer;
    sized : Buffer(nSize := 16, nOwner := 2);
    positional : Buffer(9, 3);
    nested : Owner;
    a, b, c, d, e, f, n : INT;
    r, cc : BOOL;
END_VAR
a := plain.size;
b := sized.size;
c := positional.size;
d := nested.child.size;
e := g.size;
f := nested.child.owner_id;
n := plain.inits + sized.inits + positional.inits + nested.child.inits;
r := sized.retains;
cc := sized.copy;
END_PROGRAM
"#;

#[test]
fn fb_init_runs_once_with_the_declared_arguments() {
    // Two scans: FB_init must not run again on the second.
    let s = run_with(SRC, 2, 10, false);
    assert_eq!(
        s.i64("a"),
        4,
        "no FB_init arguments: the declared initial value stays"
    );
    assert_eq!(s.i64("b"), 16);
    assert_eq!(
        s.i64("c"),
        9,
        "positional arguments follow the two implicit ones"
    );
    assert_eq!(s.i64("d"), 32, "a member of an FB gets its FB_init too");
    assert_eq!(s.i64("e"), 5, "a VAR_GLOBAL instance too");
    assert_eq!(s.i64("f"), 70);
    assert_eq!(s.i64("n"), 4, "once per instance");
    assert!(s.bool("r"), "bInitRetains: plcc starts cold");
    assert!(!s.bool("cc"), "bInCopyCode: never an online change");
}

#[test]
fn arguments_without_fb_init_are_an_error() {
    let err = compile_error(
        r#"
FUNCTION_BLOCK F VAR x : INT; END_VAR END_FUNCTION_BLOCK
PROGRAM p VAR f : F(1); END_VAR END_PROGRAM
"#,
    );
    assert!(err.contains("has no FB_init method"), "{err}");
}

#[test]
fn reference_inputs_bind_to_the_argument() {
    let src = r#"
FUNCTION_BLOCK Counter
VAR_OUTPUT n : INT; END_VAR
    n := n + 1;
END_FUNCTION_BLOCK

FUNCTION_BLOCK UsesCounter
VAR _c : REFERENCE TO Counter; END_VAR
METHOD FB_init : BOOL
VAR_INPUT
    bInitRetains : BOOL;
    bInCopyCode : BOOL;
    c : REFERENCE TO Counter;
END_VAR
    _c REF= c;
END_METHOD
    _c();
END_FUNCTION_BLOCK

PROGRAM p
VAR
    shared : Counter;
    a : UsesCounter(shared);
    b : UsesCounter(c := shared);
    n : INT;
END_VAR
a();
b();
n := shared.n;
END_PROGRAM
"#;
    let s = run_with(src, 1, 10, false);
    assert_eq!(s.i64("n"), 2, "both users drive the one shared instance");
}
