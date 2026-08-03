// SPDX-License-Identifier: MPL-2.0

//! A variable may be declared with an INTERFACE type.
//!
//! IEC 61131-3:2013 §6.6.4 — an interface-typed variable holds a reference to an
//! object implementing that interface. The registration loop only knew about
//! FUNCTION_BLOCK and CLASS, so `r : ITimer;` resolved to nothing, and once unknown
//! types became a hard error, every program declaring one stopped compiling.
//!
//! The slot is a pointer, not an inline instance — registering it as an FB instance
//! would find no struct layout and silently fall back to an i32 field.

use inkwell::context::Context;
use plcc_codegen::Compiler;

fn emit_ir(source: &str) -> String {
    let ctx = Context::create();
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let mut compiler = Compiler::new(&ctx, "iface");
    compiler.compile(&unit).expect("codegen failed");
    compiler.emit_ir()
}

const IFACE: &str = r#"
INTERFACE ITimer
METHOD Run : BOOL
END_METHOD
END_INTERFACE

PROGRAM Main
VAR
    r : ITimer;
    x : INT;
END_VAR
    x := 5;
END_PROGRAM
"#;

#[test]
fn an_interface_typed_variable_compiles() {
    let ir = emit_ir(IFACE);
    // The state struct is { ptr, i16 }: a reference slot, then x.
    assert!(
        ir.contains("{ ptr, i16 }"),
        "an ITimer variable must lay out as a pointer-sized reference:\n{ir}"
    );
    assert!(
        ir.contains("store i16 5"),
        "the rest of the body must still be compiled:\n{ir}"
    );
}

#[test]
fn an_interface_type_is_case_insensitive_too() {
    let ir = emit_ir(&IFACE.replace("r : ITimer;", "r : itimer;"));
    assert!(
        ir.contains("{ ptr, i16 }"),
        "`r : itimer;` must resolve the same as `r : ITimer;`:\n{ir}"
    );
}

#[test]
fn an_interface_typed_variable_inside_a_function_block_compiles() {
    let source = r#"
INTERFACE ISensor
METHOD Read : INT
END_METHOD
END_INTERFACE

FUNCTION_BLOCK Holder
VAR
    s : ISensor;
    seen : INT;
END_VAR
VAR_INPUT
    v : INT;
END_VAR
    seen := v;
END_FUNCTION_BLOCK

PROGRAM UseHolder
VAR
    h : Holder;
    out : INT;
END_VAR
    h(v := 9);
    out := h.seen;
END_PROGRAM
"#;
    let ir = emit_ir(source);
    assert!(
        ir.contains("call void @holder_scan("),
        "the FB holding an interface reference must still be scanned:\n{ir}"
    );
}

#[test]
fn an_undeclared_interface_like_name_is_still_rejected() {
    // The fix must not turn every unknown name into a silent pointer slot.
    let source = r#"
PROGRAM Bad
VAR
    r : INoSuchInterface;
END_VAR
    ;
END_PROGRAM
"#;
    let ctx = Context::create();
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let mut compiler = Compiler::new(&ctx, "bad");
    let err = compiler
        .compile(&unit)
        .expect_err("an undeclared type must still be an error");
    let msg = err.to_string();
    assert!(
        msg.contains("INoSuchInterface"),
        "the diagnostic must name the type: {msg}"
    );
}
