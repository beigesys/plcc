// SPDX-License-Identifier: MPL-2.0

//! Tests for the PRINT statement/function.
//! Since PRINT calls an extern function (plcc_print), we verify IR output
//! rather than JIT execution (the JIT won't have the extern symbol).

use inkwell::context::Context;
use plcc_codegen::Compiler;

fn compile_to_ir(source: &str) -> String {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let context = Context::create();
    let mut compiler = Compiler::new(&context, "print_test");
    compiler.compile(&unit).expect("codegen failed");
    compiler.emit_ir()
}

#[test]
fn print_string_literal_emits_call() {
    let source = r#"
PROGRAM PrintTest
VAR
    x : INT := 0;
END_VAR
    PRINT('hello world');
    x := x + 1;
END_PROGRAM
"#;
    let ir = compile_to_ir(source);
    // Verify the extern declaration exists
    assert!(
        ir.contains("plcc_print"),
        "IR should declare plcc_print extern function"
    );
    // Verify there's a call to plcc_print
    assert!(
        ir.contains("call void @plcc_print"),
        "IR should contain call to plcc_print"
    );
    // Verify a global string constant was created for the literal
    assert!(
        ir.contains("hello world"),
        "IR should contain the string literal as a global constant"
    );
}

#[test]
fn print_string_variable_emits_call() {
    let source = r#"
PROGRAM PrintVarTest
VAR
    msg : STRING := 'test message';
END_VAR
    PRINT(msg);
END_PROGRAM
"#;
    let ir = compile_to_ir(source);
    assert!(
        ir.contains("call void @plcc_print"),
        "IR should contain call to plcc_print for STRING variable"
    );
}

#[test]
fn print_case_insensitive() {
    // PRINT should work regardless of case
    let source = r#"
PROGRAM PrintCaseTest
VAR
    x : INT := 0;
END_VAR
    print('lowercase');
    Print('mixed case');
    PRINT('uppercase');
    x := 1;
END_PROGRAM
"#;
    let ir = compile_to_ir(source);
    // Count occurrences of call void @plcc_print
    let count = ir.matches("call void @plcc_print").count();
    assert_eq!(
        count, 3,
        "Expected 3 calls to plcc_print (case-insensitive), got {count}"
    );
}
