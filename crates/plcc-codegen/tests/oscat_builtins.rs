// SPDX-License-Identifier: MPL-2.0

//! The builtins OSCAT reaches for that were missing, and the diagnostic that hid them.
//!
//! Measured in `docs/oscat-conformance.md`: `DWORD_TO_TIME` alone blocked 35 files, and
//! the top three failure buckets were all one misattributed message — "failed to
//! compile argument for MIN", where MIN was fully implemented and the *argument* was
//! the problem.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;

fn jit_run(source: &str, prog: &str) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let context = Context::create();
    let mut compiler = Compiler::new(&context, "builtins");
    compiler.compile(&unit).expect("codegen failed");
    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT");
    let mut state = vec![0u8; 4096];
    let ptr = state.as_mut_ptr();
    if let Ok(a) = ee.get_function_address(&format!("{prog}_init")) {
        let f: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(a) };
        f(ptr);
    }
    let a = ee
        .get_function_address(&format!("{prog}_scan"))
        .expect("scan missing");
    let scan: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(a) };
    scan(ptr);
    state
}

/// Same, but the caller gets at the state buffer before the scan runs — the only way
/// to put content in a STRING today, since string literals have no codegen.
fn jit_run_with_setup(source: &str, prog: &str, setup: impl FnOnce(&mut [u8])) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let context = Context::create();
    let mut compiler = Compiler::new(&context, "builtins");
    compiler.compile(&unit).expect("codegen failed");
    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT");
    let mut state = vec![0u8; 4096];
    let ptr = state.as_mut_ptr();
    if let Ok(a) = ee.get_function_address(&format!("{prog}_init")) {
        let f: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(a) };
        f(ptr);
    }
    setup(&mut state);
    let a = ee
        .get_function_address(&format!("{prog}_scan"))
        .expect("scan missing");
    let scan: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(a) };
    scan(state.as_mut_ptr());
    state
}

fn compile_err(source: &str) -> String {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let context = Context::create();
    let mut compiler = Compiler::new(&context, "builtins_err");
    match compiler.compile(&unit) {
        Ok(()) => panic!("expected a codegen error"),
        Err(e) => e.to_string(),
    }
}

fn read_i32(s: &[u8], off: usize) -> i32 {
    i32::from_ne_bytes([s[off], s[off + 1], s[off + 2], s[off + 3]])
}

fn read_i64(s: &[u8], off: usize) -> i64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&s[off..off + 8]);
    i64::from_ne_bytes(b)
}

fn read_f32(s: &[u8], off: usize) -> f32 {
    f32::from_ne_bytes([s[off], s[off + 1], s[off + 2], s[off + 3]])
}

fn write_str(s: &mut [u8], off: usize, text: &str) {
    s[off..off + text.len()].copy_from_slice(text.as_bytes());
    s[off + text.len()] = 0;
}

fn read_str(s: &[u8], off: usize) -> String {
    let end = s[off..].iter().position(|&b| b == 0).expect("unterminated");
    String::from_utf8_lossy(&s[off..off + end]).into_owned()
}

#[test]
fn dword_to_time_reads_the_word_as_milliseconds() {
    // OSCAT stores durations as raw ms in a DWORD. TIME is nanoseconds internally,
    // so the conversion scales; reinterpreting the word would be 1e6 times too short.
    let source = r#"
PROGRAM D2T
VAR
    et : TIME;
    ms : DWORD := 1500;
END_VAR
    et := DWORD_TO_TIME(ms);
END_PROGRAM
"#;
    let state = jit_run(source, "d2t");
    assert_eq!(read_i64(&state, 0), 1_500_000_000, "1500 ms in nanoseconds");
}

#[test]
fn time_to_dword_yields_milliseconds() {
    let source = r#"
PROGRAM T2D
VAR
    ms : DWORD;
    t : TIME := T#2s500ms;
END_VAR
    ms := TIME_TO_DWORD(t);
END_PROGRAM
"#;
    let state = jit_run(source, "t2d");
    assert_eq!(read_i32(&state, 0), 2500, "T#2s500ms is 2500 ms");
}

#[test]
fn dword_and_time_round_trip() {
    let source = r#"
PROGRAM RT
VAR
    out_ms : DWORD;
    in_ms : DWORD := 86_400_000;
    t : TIME;
END_VAR
    t := DWORD_TO_TIME(in_ms);
    out_ms := TIME_TO_DWORD(t);
END_PROGRAM
"#;
    let state = jit_run(source, "rt");
    assert_eq!(
        read_i32(&state, 0),
        86_400_000,
        "a full day survives the trip"
    );
}

#[test]
fn dword_to_real_carries_the_value_not_the_bit_pattern() {
    let source = r#"
PROGRAM D2R
VAR
    r : REAL;
    src : DWORD := 16;
END_VAR
    r := DWORD_TO_REAL(src);
END_PROGRAM
"#;
    let state = jit_run(source, "d2r");
    assert_eq!(read_f32(&state, 0), 16.0, "a bitcast would give 2.2e-44");
}

#[test]
fn real_to_dword_converts_back() {
    let source = r#"
PROGRAM R2D
VAR
    d : DWORD;
    r : REAL := 32.75;
END_VAR
    d := REAL_TO_DWORD(r);
END_PROGRAM
"#;
    let state = jit_run(source, "r2d");
    assert_eq!(
        read_i32(&state, 0),
        32,
        "truncates toward zero, as its siblings do"
    );
}

#[test]
fn replace_splices_the_second_string_into_the_first() {
    // REPLACE('HELLO WORLD', 'XY', 3, 4): drop three characters from position 4
    // ("LO ") and put 'XY' there.
    let source = r#"
PROGRAM Rep
VAR
    dst : STRING[31];
    a : STRING[31];
    b : STRING[31];
END_VAR
    dst := REPLACE(a, b, 3, 4);
END_PROGRAM
"#;
    let state = jit_run_with_setup(source, "rep", |s| {
        write_str(s, 32, "HELLO WORLD");
        write_str(s, 64, "XY");
    });
    assert_eq!(read_str(&state, 0), "HELXYWORLD");
}

#[test]
fn replace_can_write_over_its_own_input() {
    // `s := REPLACE(s, t, ...)` is the idiomatic form. Building the result directly
    // in the destination would overwrite the tail of `s` before reading it.
    let source = r#"
PROGRAM RepSelf
VAR
    a : STRING[31];
    b : STRING[31];
END_VAR
    a := REPLACE(a, b, 5, 1);
END_PROGRAM
"#;
    let state = jit_run_with_setup(source, "repself", |s| {
        write_str(s, 0, "ABCDEFGH");
        write_str(s, 32, "0123456789");
    });
    assert_eq!(read_str(&state, 0), "0123456789FGH");
}

#[test]
fn replace_past_the_end_appends_rather_than_reading_past_it() {
    let source = r#"
PROGRAM RepEnd
VAR
    dst : STRING[31];
    a : STRING[31];
    b : STRING[31];
END_VAR
    dst := REPLACE(a, b, 99, 40);
END_PROGRAM
"#;
    let state = jit_run_with_setup(source, "repend", |s| {
        write_str(s, 32, "ABC");
        write_str(s, 64, "XY");
    });
    assert_eq!(read_str(&state, 0), "ABCXY");
}

#[test]
fn replace_stops_at_the_destination_capacity() {
    let source = r#"
PROGRAM RepCap
VAR
    dst : STRING[7];
    a : STRING[31];
    b : STRING[31];
END_VAR
    dst := REPLACE(a, b, 1, 2);
END_PROGRAM
"#;
    let state = jit_run_with_setup(source, "repcap", |s| {
        write_str(s, 8, "ABCDEFGHIJ");
        write_str(s, 40, "12345");
    });
    let out = read_str(&state, 0);
    assert_eq!(out, "A12345C", "seven characters and a terminator, no more");
}

#[test]
fn a_failed_argument_names_the_argument_not_the_builtin() {
    // The message that cost an hour: MIN is implemented, and reporting the failure
    // against MIN sent the reader to the wrong file. The undefined name is what
    // should be on screen — in OSCAT it is a VAR_GLOBAL from a sibling file.
    let source = r#"
PROGRAM ArgDiag
VAR
    y : INT;
END_VAR
    y := MIN(y, LANGUAGE_LMAX);
END_PROGRAM
"#;
    let msg = compile_err(source);
    assert!(
        msg.contains("LANGUAGE_LMAX"),
        "the failing argument must be named, got: {msg}"
    );
    assert!(
        !msg.contains("failed to compile argument for"),
        "the old self-blaming wording must be gone, got: {msg}"
    );
}

#[test]
fn a_call_to_something_undefined_says_so() {
    let source = r#"
PROGRAM CallDiag
VAR
    y : DINT;
END_VAR
    y := SHR(T_PLC_MS(), 1);
END_PROGRAM
"#;
    let msg = compile_err(source);
    assert!(
        msg.contains("T_PLC_MS"),
        "the innermost cause must reach the surface, got: {msg}"
    );
}

#[test]
fn a_conversion_states_its_own_result_type() {
    // TIME_TO_DWORD is unsigned, and SHR on an untyped value took the signed path —
    // an arithmetic shift that drags the sign bit down through the result.
    let source = r#"
PROGRAM ShiftSign
VAR
    out : DWORD;
    t : TIME := T#4294967s;
END_VAR
    out := SHR(TIME_TO_DWORD(t), 4);
END_PROGRAM
"#;
    let state = jit_run(source, "shiftsign");
    // 4294967 s = 4_294_967_000 ms, which has the high bit set as a DWORD.
    let expected = (4_294_967_000u32 >> 4) as i32;
    assert_eq!(
        read_i32(&state, 0),
        expected,
        "the shift must be logical, not arithmetic"
    );
}
