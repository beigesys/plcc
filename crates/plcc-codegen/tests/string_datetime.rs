// SPDX-License-Identifier: MPL-2.0

//! JIT execution tests for string functions (CONCAT, LEFT, RIGHT, MID, FIND)
//! and date/time arithmetic functions (ADD_TIME, SUB_TIME, MUL_TIME, DIV_TIME).

use inkwell::context::Context;
use inkwell::OptimizationLevel;
use plcc_codegen::Compiler;

/// Helper: compile source, JIT execute the scan function, return state bytes.
fn compile_and_run(source: &str, scan_fn_name: &str, state_size: usize, num_scans: usize) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");

    let context = Context::create();
    let mut compiler = Compiler::new(&context, "test");
    compiler.compile(&unit).expect("codegen failed");

    let ir = compiler.emit_ir();
    assert!(
        ir.contains(scan_fn_name),
        "scan function not found in IR:\n{ir}"
    );

    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT execution engine");

    let mut state = vec![0u8; state_size];
    let state_ptr = state.as_mut_ptr();

    let fn_ptr = ee
        .get_function_address(scan_fn_name)
        .expect("failed to get function address");

    let scan: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(fn_ptr) };
    for _ in 0..num_scans {
        scan(state_ptr);
    }

    state
}

fn read_i64(state: &[u8], offset: usize) -> i64 {
    i64::from_ne_bytes(state[offset..offset + 8].try_into().unwrap())
}

fn read_i16(state: &[u8], offset: usize) -> i16 {
    i16::from_ne_bytes(state[offset..offset + 1 + 1].try_into().unwrap())
}

fn read_string(state: &[u8], offset: usize, max_len: usize) -> String {
    let bytes = &state[offset..offset + max_len];
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(max_len);
    String::from_utf8_lossy(&bytes[..end]).to_string()
}

// ===== Date/Time Arithmetic Tests =====

#[test]
fn test_add_time() {
    // ADD_TIME(T#100ms, T#50ms) = T#150ms = 150_000_000 ns
    let source = r#"
PROGRAM AddTimeTest
VAR
    t1 : TIME;
    t2 : TIME;
    result : TIME;
END_VAR
    t1 := T#100ms;
    t2 := T#50ms;
    result := ADD_TIME(t1, t2);
END_PROGRAM
"#;
    // State layout: 3 x i64 = 24 bytes
    // t1 at 0, t2 at 8, result at 16
    let state = compile_and_run(source, "addtimetest_scan", 24, 1);
    let result = read_i64(&state, 16);
    assert_eq!(
        result, 150_000_000,
        "ADD_TIME(T#100ms, T#50ms) should be 150_000_000 ns, got {result}"
    );
}

#[test]
fn test_sub_time() {
    // SUB_TIME(T#1s, T#250ms) = T#750ms = 750_000_000 ns
    let source = r#"
PROGRAM SubTimeTest
VAR
    t1 : TIME;
    t2 : TIME;
    result : TIME;
END_VAR
    t1 := T#1s;
    t2 := T#250ms;
    result := SUB_TIME(t1, t2);
END_PROGRAM
"#;
    let state = compile_and_run(source, "subtimetest_scan", 24, 1);
    let result = read_i64(&state, 16);
    assert_eq!(
        result, 750_000_000,
        "SUB_TIME(T#1s, T#250ms) should be 750_000_000 ns, got {result}"
    );
}

#[test]
fn test_mul_time() {
    // MUL_TIME(T#100ms, 3) = T#300ms = 300_000_000 ns
    let source = r#"
PROGRAM MulTimeTest
VAR
    t1 : TIME;
    factor : INT;
    result : TIME;
END_VAR
    t1 := T#100ms;
    factor := 3;
    result := MUL_TIME(t1, factor);
END_PROGRAM
"#;
    // State layout: i64 (t1) at 0, i16 (factor) at 8, padding to 16, i64 (result) at 16
    // Actually struct packing: {i64, i16, i64} — need to check alignment
    // i64 at 0, i16 at 8, then i64 needs 8-byte alignment so padding to 16, i64 at 16
    let state = compile_and_run(source, "multimetest_scan", 24, 1);
    let result = read_i64(&state, 16);
    assert_eq!(
        result, 300_000_000,
        "MUL_TIME(T#100ms, 3) should be 300_000_000 ns, got {result}"
    );
}

#[test]
fn test_div_time() {
    // DIV_TIME(T#1s, 4) = T#250ms = 250_000_000 ns
    let source = r#"
PROGRAM DivTimeTest
VAR
    t1 : TIME;
    divisor : INT;
    result : TIME;
END_VAR
    t1 := T#1s;
    divisor := 4;
    result := DIV_TIME(t1, divisor);
END_PROGRAM
"#;
    // Same layout as MulTimeTest: i64 at 0, i16 at 8, i64 at 16
    let state = compile_and_run(source, "divtimetest_scan", 24, 1);
    let result = read_i64(&state, 16);
    assert_eq!(
        result, 250_000_000,
        "DIV_TIME(T#1s, 4) should be 250_000_000 ns, got {result}"
    );
}

// ===== String Function Tests =====

#[test]
fn test_find_string() {
    // FIND('HELLO WORLD', 'WORLD') should return 7 (1-based)
    let source = r#"
PROGRAM FindTest
VAR
    s1 : STRING[20];
    s2 : STRING[10];
    pos : INT;
END_VAR
    pos := FIND(s1, s2);
END_PROGRAM
"#;
    // State layout: STRING[20] = 21 bytes at 0, STRING[10] = 11 bytes at 22 (aligned to 2 for i16?),
    // Actually strings are [N x i8] arrays, then INT is i16.
    // Let's just check that it compiles and the FIND function is generated.
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");

    let context = Context::create();
    let mut compiler = Compiler::new(&context, "test");
    compiler.compile(&unit).expect("codegen failed");

    let ir = compiler.emit_ir();
    assert!(
        ir.contains("plcc_find"),
        "plcc_find function should be in IR"
    );
    assert!(
        ir.contains("findtest_scan"),
        "scan function should be in IR"
    );
}

#[test]
fn test_concat_compiles() {
    // Test that CONCAT assignment generates correct IR
    let source = r#"
PROGRAM ConcatTest
VAR
    s1 : STRING[20];
    s2 : STRING[20];
    result : STRING[40];
END_VAR
    result := CONCAT(s1, s2);
END_PROGRAM
"#;
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");

    let context = Context::create();
    let mut compiler = Compiler::new(&context, "test");
    compiler.compile(&unit).expect("codegen failed");

    let ir = compiler.emit_ir();
    assert!(
        ir.contains("plcc_concat"),
        "plcc_concat function should be in IR"
    );
}

#[test]
fn test_left_compiles() {
    let source = r#"
PROGRAM LeftTest
VAR
    src : STRING[20];
    result : STRING[20];
    n : INT;
END_VAR
    n := 5;
    result := LEFT(src, n);
END_PROGRAM
"#;
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");

    let context = Context::create();
    let mut compiler = Compiler::new(&context, "test");
    compiler.compile(&unit).expect("codegen failed");

    let ir = compiler.emit_ir();
    assert!(
        ir.contains("plcc_left"),
        "plcc_left function should be in IR"
    );
}

#[test]
fn test_right_compiles() {
    let source = r#"
PROGRAM RightTest
VAR
    src : STRING[20];
    result : STRING[20];
    n : INT;
END_VAR
    n := 5;
    result := RIGHT(src, n);
END_PROGRAM
"#;
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");

    let context = Context::create();
    let mut compiler = Compiler::new(&context, "test");
    compiler.compile(&unit).expect("codegen failed");

    let ir = compiler.emit_ir();
    assert!(
        ir.contains("plcc_right"),
        "plcc_right function should be in IR"
    );
}

#[test]
fn test_mid_compiles() {
    let source = r#"
PROGRAM MidTest
VAR
    src : STRING[20];
    result : STRING[20];
END_VAR
    result := MID(src, 3, 2);
END_PROGRAM
"#;
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");

    let context = Context::create();
    let mut compiler = Compiler::new(&context, "test");
    compiler.compile(&unit).expect("codegen failed");

    let ir = compiler.emit_ir();
    assert!(
        ir.contains("plcc_mid"),
        "plcc_mid function should be in IR"
    );
}

#[test]
fn test_concat_execution() {
    // Test CONCAT with pre-initialized string state
    let source = r#"
PROGRAM ConcatExecTest
VAR
    s1 : STRING[20];
    s2 : STRING[20];
    result : STRING[40];
END_VAR
    result := CONCAT(s1, s2);
END_PROGRAM
"#;
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");

    let context = Context::create();
    let mut compiler = Compiler::new(&context, "test");
    compiler.compile(&unit).expect("codegen failed");

    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT execution engine");

    // State: STRING[20]=21 bytes + STRING[20]=21 bytes + STRING[40]=41 bytes = 83 bytes
    // But struct alignment may add padding. Use generous buffer.
    let mut state = vec![0u8; 128];

    // Write "Hello" into s1 at offset 0
    let s1_bytes = b"Hello\0";
    state[..s1_bytes.len()].copy_from_slice(s1_bytes);

    // Write "World" into s2 at offset 21 (STRING[20] is 21 bytes with null term)
    let s2_bytes = b"World\0";
    state[21..21 + s2_bytes.len()].copy_from_slice(s2_bytes);

    let state_ptr = state.as_mut_ptr();
    let fn_ptr = ee
        .get_function_address("concatexectest_scan")
        .expect("failed to get function address");
    let scan: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(fn_ptr) };
    scan(state_ptr);

    // result should be at offset 42 (21 + 21)
    let result = read_string(&state, 42, 41);
    assert_eq!(
        result, "HelloWorld",
        "CONCAT('Hello', 'World') should be 'HelloWorld', got '{result}'"
    );
}

#[test]
fn test_left_execution() {
    let source = r#"
PROGRAM LeftExecTest
VAR
    src : STRING[20];
    result : STRING[20];
    n : INT;
END_VAR
    n := 5;
    result := LEFT(src, n);
END_PROGRAM
"#;
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");

    let context = Context::create();
    let mut compiler = Compiler::new(&context, "test");
    compiler.compile(&unit).expect("codegen failed");

    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT execution engine");

    // State: STRING[20]=21 bytes + STRING[20]=21 bytes + INT=2 bytes = 44 bytes
    let mut state = vec![0u8; 64];

    // Write "HelloWorld" into src at offset 0
    let src_bytes = b"HelloWorld\0";
    state[..src_bytes.len()].copy_from_slice(src_bytes);

    // n at offset 42 (after two STRING[20]s = 21+21)
    // Actually: src(21) + result(21) + n(2)
    // n at offset 42, set to 5
    state[42] = 5;
    state[43] = 0;

    let state_ptr = state.as_mut_ptr();
    let fn_ptr = ee
        .get_function_address("leftexectest_scan")
        .expect("failed to get function address");
    let scan: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(fn_ptr) };
    scan(state_ptr);

    // result at offset 21
    let result = read_string(&state, 21, 21);
    assert_eq!(
        result, "Hello",
        "LEFT('HelloWorld', 5) should be 'Hello', got '{result}'"
    );
}

#[test]
fn test_right_execution() {
    let source = r#"
PROGRAM RightExecTest
VAR
    src : STRING[20];
    result : STRING[20];
    n : INT;
END_VAR
    n := 5;
    result := RIGHT(src, n);
END_PROGRAM
"#;
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");

    let context = Context::create();
    let mut compiler = Compiler::new(&context, "test");
    compiler.compile(&unit).expect("codegen failed");

    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT execution engine");

    let mut state = vec![0u8; 64];

    // Write "HelloWorld" into src at offset 0
    let src_bytes = b"HelloWorld\0";
    state[..src_bytes.len()].copy_from_slice(src_bytes);

    // n at offset 42
    state[42] = 5;
    state[43] = 0;

    let state_ptr = state.as_mut_ptr();
    let fn_ptr = ee
        .get_function_address("rightexectest_scan")
        .expect("failed to get function address");
    let scan: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(fn_ptr) };
    scan(state_ptr);

    let result = read_string(&state, 21, 21);
    assert_eq!(
        result, "World",
        "RIGHT('HelloWorld', 5) should be 'World', got '{result}'"
    );
}

#[test]
fn test_mid_execution() {
    let source = r#"
PROGRAM MidExecTest
VAR
    src : STRING[20];
    result : STRING[20];
END_VAR
    result := MID(src, 3, 4);
END_PROGRAM
"#;
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");

    let context = Context::create();
    let mut compiler = Compiler::new(&context, "test");
    compiler.compile(&unit).expect("codegen failed");

    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT execution engine");

    let mut state = vec![0u8; 64];

    // Write "HelloWorld" into src at offset 0
    let src_bytes = b"HelloWorld\0";
    state[..src_bytes.len()].copy_from_slice(src_bytes);

    let state_ptr = state.as_mut_ptr();
    let fn_ptr = ee
        .get_function_address("midexectest_scan")
        .expect("failed to get function address");
    let scan: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(fn_ptr) };
    scan(state_ptr);

    // MID('HelloWorld', 3, 4) => 3 chars starting at position 4 = "loW"
    let result = read_string(&state, 21, 21);
    assert_eq!(
        result, "loW",
        "MID('HelloWorld', 3, 4) should be 'loW', got '{result}'"
    );
}

#[test]
fn test_find_execution() {
    let source = r#"
PROGRAM FindExecTest
VAR
    s1 : STRING[20];
    s2 : STRING[10];
    pos : INT;
END_VAR
    pos := FIND(s1, s2);
END_PROGRAM
"#;
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");

    let context = Context::create();
    let mut compiler = Compiler::new(&context, "test");
    compiler.compile(&unit).expect("codegen failed");

    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT execution engine");

    // State: STRING[20]=21 bytes + STRING[10]=11 bytes + INT=2 bytes
    // pos at offset 32 (21+11), but check alignment
    let mut state = vec![0u8; 64];

    // Write "Hello World" into s1 at offset 0
    let s1_bytes = b"Hello World\0";
    state[..s1_bytes.len()].copy_from_slice(s1_bytes);

    // Write "World" into s2 at offset 21
    let s2_bytes = b"World\0";
    state[21..21 + s2_bytes.len()].copy_from_slice(s2_bytes);

    let state_ptr = state.as_mut_ptr();
    let fn_ptr = ee
        .get_function_address("findexectest_scan")
        .expect("failed to get function address");
    let scan: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(fn_ptr) };
    scan(state_ptr);

    // pos at offset 32 (21 + 11)
    let pos = read_i16(&state, 32);
    assert_eq!(
        pos, 7,
        "FIND('Hello World', 'World') should be 7 (1-based), got {pos}"
    );
}
