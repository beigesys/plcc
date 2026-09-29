// SPDX-License-Identifier: MPL-2.0

//! Method functions are named so they cannot collide with the POU's own
//! generated functions or with another POU's methods. A method `Init` of FB
//! `Fb` was the function `fb_init` — the name of Fb's initializer — and the
//! module failed verification (TcOpen's Tc.Prober tests have one).

mod common;
use common::run;

const SRC: &str = r#"
FUNCTION_BLOCK Fb
VAR
    n : INT := 5;
    inits : INT;
END_VAR
METHOD Init : BOOL
    inits := inits + 1;
    Init := TRUE;
END_METHOD
METHOD Scan : INT
    Scan := n;
END_METHOD
END_FUNCTION_BLOCK

FUNCTION_BLOCK A
METHOD B_C : INT
    B_C := 1;
END_METHOD
END_FUNCTION_BLOCK

FUNCTION_BLOCK A_B
METHOD C : INT
    C := 2;
END_METHOD
END_FUNCTION_BLOCK

PROGRAM p
VAR
    f : Fb;
    a : A;
    ab : A_B;
    ok : BOOL;
    inits : INT;
    scanned : INT;
    sum : INT;
END_VAR
ok := f.Init();
inits := f.inits;
scanned := f.Scan();
sum := a.B_C() * 10 + ab.C();
END_PROGRAM
"#;

#[test]
fn methods_named_like_generated_functions() {
    let s = run(SRC);
    assert!(s.bool("ok"));
    // Initialized to 0 by the FB's initializer, then one explicit call.
    assert_eq!(s.i64("inits"), 1);
    assert_eq!(s.i64("scanned"), 5);
    assert_eq!(s.i64("sum"), 12);
}
