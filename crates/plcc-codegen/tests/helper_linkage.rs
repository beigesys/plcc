// SPDX-License-Identifier: MPL-2.0

//! The string helpers codegen synthesizes (`plcc_strlen`, `plcc_mid`, ...) are
//! private to each module. With external linkage, two separately compiled plcc
//! objects that both used `LEN` could not be linked into one firmware image
//! (duplicate symbol `plcc_strlen`).

use inkwell::context::Context;
use plcc_codegen::Compiler;

#[test]
fn string_helpers_have_internal_linkage() {
    let src = r#"
PROGRAM p
VAR s : STRING := 'abc'; t : STRING; n : INT; END_VAR
n := LEN(s) + FIND(s, 'b');
t := LEFT(s, 1);
t := RIGHT(s, 1);
t := MID(s, 1, 2);
t := REPLACE(s, 'x', 1, 1);
t := CONCAT(s, t);
IF s = t THEN n := 0; END_IF;
END_PROGRAM
"#;
    let (unit, errors) = plcc_st::parse(src);
    assert!(errors.is_empty(), "{errors:?}");
    let ctx = Context::create();
    let mut c = Compiler::new(&ctx, "linkage");
    c.compile(&unit).expect("codegen");
    let ir = c.emit_ir();
    for helper in [
        "plcc_strlen",
        "plcc_find",
        "plcc_left",
        "plcc_right",
        "plcc_mid",
        "plcc_replace",
        "plcc_strlcpy",
        "plcc_strlcat",
        "plcc_strcmp",
    ] {
        let def = ir
            .lines()
            .find(|l| l.starts_with("define") && l.contains(&format!("@{helper}(")))
            .unwrap_or_else(|| panic!("{helper} not defined"));
        assert!(def.contains("internal"), "{helper}: {def}");
    }
}
