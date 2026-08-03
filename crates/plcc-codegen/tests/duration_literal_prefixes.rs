// SPDX-License-Identifier: MPL-2.0

//! JIT execution tests for the spelled-out duration literal prefixes
//! (`TIME#`, `LT#`, `LTIME#`) that the lexer accepts per IEC 61131-3 Annex A
//! B.1.2.3.
//!
//! These execute rather than inspect IR: the value of a duration literal is
//! computed by `parse_time_literal_ns`, which strips the prefix by hand. An
//! IR-text assertion would not catch a prefix it fails to strip.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;

/// Compile `source`, JIT the named scan function, run it once, return state bytes.
fn compile_and_run(source: &str, scan_fn_name: &str, state_size: usize) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");

    let context = Context::create();
    let mut compiler = Compiler::new(&context, "test");
    compiler.compile(&unit).expect("codegen failed");

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
    scan(state_ptr);
    state
}

fn read_i64(state: &[u8], offset: usize) -> i64 {
    i64::from_ne_bytes(state[offset..offset + 8].try_into().expect("8 bytes"))
}

/// Every duration prefix must yield the identical nanosecond value.
#[test]
fn duration_literal_prefixes_all_yield_same_nanoseconds() {
    // 1s 500ms = 1_500_000_000 ns
    const EXPECTED_NS: i64 = 1_500_000_000;

    for literal in ["T#1s500ms", "TIME#1s500ms", "LT#1s500ms", "LTIME#1s500ms"] {
        let source = format!(
            "\
PROGRAM DurPfx
VAR
    result : TIME;
END_VAR
    result := {literal};
END_PROGRAM
"
        );
        let state = compile_and_run(&source, "durpfx_scan", 8);
        assert_eq!(
            read_i64(&state, 0),
            EXPECTED_NS,
            "{literal} should evaluate to {EXPECTED_NS} ns"
        );
    }
}

/// The spelled-out prefixes must work through the TIME arithmetic builtins too,
/// which is where a mis-stripped prefix would silently produce 0.
#[test]
fn add_time_with_spelled_out_prefixes() {
    let source = "\
PROGRAM AddPfx
VAR
    a : TIME;
    b : TIME;
    result : TIME;
END_VAR
    a := TIME#100ms;
    b := LTIME#50ms;
    result := ADD_TIME(a, b);
END_PROGRAM
";
    let state = compile_and_run(source, "addpfx_scan", 24);
    assert_eq!(read_i64(&state, 0), 100_000_000, "TIME#100ms");
    assert_eq!(read_i64(&state, 8), 50_000_000, "LTIME#50ms");
    assert_eq!(
        read_i64(&state, 16),
        150_000_000,
        "ADD_TIME(TIME#100ms, LTIME#50ms)"
    );
}

/// Mixed compound units, upper and lower case, across prefixes.
#[test]
fn duration_literal_compound_units() {
    let cases: &[(&str, i64)] = &[
        ("T#1d", 86_400_000_000_000),
        ("TIME#2h", 7_200_000_000_000),
        ("ltime#3m", 180_000_000_000),
        ("lt#4s", 4_000_000_000),
        ("time#5ms", 5_000_000),
        ("T#1h30m", 5_400_000_000_000),
        ("LTIME#1h30m", 5_400_000_000_000),
    ];
    for (literal, expected) in cases {
        let source = format!(
            "\
PROGRAM DurUnits
VAR
    result : TIME;
END_VAR
    result := {literal};
END_PROGRAM
"
        );
        let state = compile_and_run(&source, "durunits_scan", 8);
        assert_eq!(
            read_i64(&state, 0),
            *expected,
            "{literal} should evaluate to {expected} ns"
        );
    }
}
