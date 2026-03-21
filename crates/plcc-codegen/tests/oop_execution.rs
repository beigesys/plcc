// SPDX-License-Identifier: MPL-2.0

//! Tests for OOP features: FB methods, CLASS compilation, method calls.

use inkwell::context::Context;
use inkwell::OptimizationLevel;
use plcc_codegen::Compiler;

/// Helper: compile, JIT execute with init + multiple scans, return state bytes.
fn jit_run(source: &str, scan_fn_name: &str, state_size: usize, num_scans: usize) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");

    let context = Context::create();
    let mut compiler = Compiler::new(&context, "test");
    compiler.compile(&unit).expect("codegen failed");

    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT");

    let mut state = vec![0u8; state_size];
    let state_ptr = state.as_mut_ptr();

    // Call _init() to apply variable initializers
    let init_fn_name = scan_fn_name.replace("_scan", "_init");
    if let Ok(init_ptr) = ee.get_function_address(&init_fn_name) {
        let init: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(init_ptr) };
        init(state_ptr);
    }

    let fn_ptr = ee
        .get_function_address(scan_fn_name)
        .expect("function not found");

    let scan: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(fn_ptr) };
    for _ in 0..num_scans {
        scan(state_ptr);
    }

    state
}

fn read_i16(state: &[u8], offset: usize) -> i16 {
    i16::from_ne_bytes([state[offset], state[offset + 1]])
}

// ---------------------------------------------------------------------------
// Test 1: FB with methods — void method and method with return value
// ---------------------------------------------------------------------------
#[test]
fn fb_with_method_add_and_get() {
    let source = r#"
FUNCTION_BLOCK Accumulator
VAR
    total : INT := 0;
END_VAR
    METHOD Add
    VAR_INPUT
        val : INT;
    END_VAR
        total := total + val;
    END_METHOD

    METHOD GetTotal : INT
        GetTotal := total;
    END_METHOD
END_FUNCTION_BLOCK

PROGRAM MethodTest
VAR
    acc : Accumulator;
    result : INT;
END_VAR
    acc.Add(val := 5);
    acc.Add(val := 3);
    result := acc.GetTotal();
END_PROGRAM
"#;

    // Accumulator struct: { i16 (total) } = 2 bytes
    // Program struct: { Accumulator(2 bytes), i16 (result) } = 4 bytes
    let state = jit_run(source, "methodtest_scan", 64, 1);

    // result is at offset 2 (after the 2-byte Accumulator struct)
    let result = read_i16(&state, 2);
    assert_eq!(result, 8, "after Add(5) + Add(3), GetTotal should be 8, got {result}");
}

// ---------------------------------------------------------------------------
// Test 2: FB method with return value used directly in expression
// ---------------------------------------------------------------------------
#[test]
fn fb_method_return_in_expression() {
    let source = r#"
FUNCTION_BLOCK Calculator
VAR
    value : INT := 0;
END_VAR
    METHOD AddAndReturn : INT
    VAR_INPUT
        x : INT;
    END_VAR
        value := value + x;
        AddAndReturn := value;
    END_METHOD
END_FUNCTION_BLOCK

PROGRAM CalcTest
VAR
    calc : Calculator;
    result : INT;
END_VAR
    result := calc.AddAndReturn(x := 10);
END_PROGRAM
"#;

    let state = jit_run(source, "calctest_scan", 64, 1);
    let result = read_i16(&state, 2);
    assert_eq!(result, 10, "AddAndReturn(10) should return 10, got {result}");
}

// ---------------------------------------------------------------------------
// Test 3: FB method state persists across scans
// ---------------------------------------------------------------------------
#[test]
fn fb_method_across_scans() {
    let source = r#"
FUNCTION_BLOCK Counter
VAR
    count : INT := 0;
END_VAR
    METHOD Increment
        count := count + 1;
    END_METHOD

    METHOD GetCount : INT
        GetCount := count;
    END_METHOD
END_FUNCTION_BLOCK

PROGRAM ScanTest
VAR
    ctr : Counter;
    result : INT;
END_VAR
    ctr.Increment();
    result := ctr.GetCount();
END_PROGRAM
"#;

    // After 3 scans, Increment is called 3 times
    let state = jit_run(source, "scantest_scan", 64, 3);
    let result = read_i16(&state, 2);
    assert_eq!(result, 3, "after 3 scans with Increment(), count should be 3, got {result}");
}

// ---------------------------------------------------------------------------
// Test 4: FB method modifies state used by another method
// ---------------------------------------------------------------------------
#[test]
fn fb_multiple_methods_shared_state() {
    let source = r#"
FUNCTION_BLOCK Adder
VAR
    accumulator : INT := 0;
END_VAR
    METHOD Add : INT
    VAR_INPUT
        val : INT;
    END_VAR
        accumulator := accumulator + val;
        Add := accumulator;
    END_METHOD

    METHOD GetValue : INT
        GetValue := accumulator;
    END_METHOD

    METHOD Reset
        accumulator := 0;
    END_METHOD
END_FUNCTION_BLOCK

PROGRAM AdderTest
VAR
    adder : Adder;
    r1 : INT;
    r2 : INT;
    r3 : INT;
END_VAR
    r1 := adder.Add(val := 10);
    r2 := adder.Add(val := 20);
    r3 := adder.GetValue();
END_PROGRAM
"#;

    let state = jit_run(source, "addertest_scan", 64, 1);
    // Adder struct: { i16 (accumulator) } = 2 bytes
    // Program struct: { Adder(2), r1(i16), r2(i16), r3(i16) }
    let r1 = read_i16(&state, 2);
    let r2 = read_i16(&state, 4);
    let r3 = read_i16(&state, 6);
    assert_eq!(r1, 10, "first Add(10) should return 10, got {r1}");
    assert_eq!(r2, 30, "second Add(20) should return 30, got {r2}");
    assert_eq!(r3, 30, "GetValue after adds should be 30, got {r3}");
}

// ---------------------------------------------------------------------------
// Test 5: FB method with local variables
// ---------------------------------------------------------------------------
#[test]
fn fb_method_with_locals() {
    let source = r#"
FUNCTION_BLOCK Doubler
VAR
    stored : INT := 0;
END_VAR
    METHOD DoubleAndStore : INT
    VAR_INPUT
        input : INT;
    END_VAR
    VAR
        temp : INT;
    END_VAR
        temp := input * 2;
        stored := temp;
        DoubleAndStore := temp;
    END_METHOD
END_FUNCTION_BLOCK

PROGRAM DoubleTest
VAR
    d : Doubler;
    result : INT;
END_VAR
    result := d.DoubleAndStore(input := 7);
END_PROGRAM
"#;

    let state = jit_run(source, "doubletest_scan", 64, 1);
    let result = read_i16(&state, 2);
    assert_eq!(result, 14, "DoubleAndStore(7) should return 14, got {result}");
}

// ---------------------------------------------------------------------------
// Test 6: Multiple FB instances with methods
// ---------------------------------------------------------------------------
#[test]
fn multiple_fb_instances_with_methods() {
    let source = r#"
FUNCTION_BLOCK Accumulator
VAR
    total : INT := 0;
END_VAR
    METHOD Add
    VAR_INPUT
        val : INT;
    END_VAR
        total := total + val;
    END_METHOD

    METHOD GetTotal : INT
        GetTotal := total;
    END_METHOD
END_FUNCTION_BLOCK

PROGRAM MultiTest
VAR
    a : Accumulator;
    b : Accumulator;
    ra : INT;
    rb : INT;
END_VAR
    a.Add(val := 5);
    b.Add(val := 100);
    ra := a.GetTotal();
    rb := b.GetTotal();
END_PROGRAM
"#;

    let state = jit_run(source, "multitest_scan", 64, 1);
    // Program struct: { Accumulator_a(2), Accumulator_b(2), ra(i16), rb(i16) }
    let ra = read_i16(&state, 4);
    let rb = read_i16(&state, 6);
    assert_eq!(ra, 5, "a.GetTotal() should be 5, got {ra}");
    assert_eq!(rb, 100, "b.GetTotal() should be 100, got {rb}");
}

// ---------------------------------------------------------------------------
// Test 7: CLASS with methods (no body, only methods)
// ---------------------------------------------------------------------------
#[test]
fn class_with_methods() {
    let source = r#"
CLASS Adder
VAR
    accumulator : INT := 0;
END_VAR
    METHOD Add : INT
    VAR_INPUT
        val : INT;
    END_VAR
        accumulator := accumulator + val;
        Add := accumulator;
    END_METHOD

    METHOD GetValue : INT
        GetValue := accumulator;
    END_METHOD
END_CLASS

PROGRAM ClassTest
VAR
    adder : Adder;
    r1 : INT;
    r2 : INT;
END_VAR
    r1 := adder.Add(val := 7);
    r2 := adder.GetValue();
END_PROGRAM
"#;

    let state = jit_run(source, "classtest_scan", 64, 1);
    // Adder struct: { i16 (accumulator) } = 2 bytes
    // Program struct: { Adder(2), r1(i16), r2(i16) }
    let r1 = read_i16(&state, 2);
    let r2 = read_i16(&state, 4);
    assert_eq!(r1, 7, "Add(7) should return 7, got {r1}");
    assert_eq!(r2, 7, "GetValue() should return 7, got {r2}");
}
