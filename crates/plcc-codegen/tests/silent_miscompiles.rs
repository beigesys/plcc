// SPDX-License-Identifier: MPL-2.0

//! Execution tests for defects that compiled cleanly and produced wrong values.
//!
//! Every test compiles ST, verifies the module, JIT-executes `p_init` then `p_scan`,
//! and asserts exact values read from the PROGRAM state struct. Result variables are
//! declared first, all of one width, so their offsets are stable.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;

fn run_scans(source: &str, scans: usize) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let ctx = Context::create();
    let mut compiler = Compiler::new(&ctx, "silent");
    compiler.compile(&unit).expect("codegen failed");
    if let Err(e) = compiler.module().verify() {
        panic!("invalid IR: {e}\n{}", compiler.emit_ir());
    }

    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT");
    let mut state = vec![0u8; 8192];
    let ptr = state.as_mut_ptr();
    if let Ok(addr) = ee.get_function_address("p_init") {
        let f: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(addr) };
        f(ptr);
    }
    let addr = ee.get_function_address("p_scan").expect("p_scan");
    let f: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(addr) };
    for _ in 0..scans {
        f(ptr);
    }
    state
}

fn run(source: &str) -> Vec<u8> {
    run_scans(source, 1)
}

fn dint(state: &[u8], idx: usize) -> i32 {
    let o = idx * 4;
    i32::from_ne_bytes([state[o], state[o + 1], state[o + 2], state[o + 3]])
}

// ---------------------------------------------------------------------------
// RETURN leaves the POU immediately
// ---------------------------------------------------------------------------

#[test]
fn return_in_program_skips_rest_of_scan() {
    let src = r#"
PROGRAM p
VAR
    a : DINT;
    b : DINT;
    n : DINT;
END_VAR
    n := n + 1;
    a := 1;
    IF n > 1 THEN
        RETURN;
    END_IF;
    b := b + 10;
END_PROGRAM
"#;
    let state = run_scans(src, 3);
    assert_eq!(dint(&state, 0), 1);
    assert_eq!(dint(&state, 1), 10, "only the first scan reaches b := b + 10");
    assert_eq!(dint(&state, 2), 3);
}

#[test]
fn return_unconditional_in_program() {
    let src = r#"
PROGRAM p
VAR
    a : DINT;
    b : DINT;
END_VAR
    a := 1;
    RETURN;
    b := 2;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 1);
    assert_eq!(dint(&state, 1), 0, "code after RETURN must not run");
}

#[test]
fn return_in_function_keeps_result_assigned_so_far() {
    let src = r#"
FUNCTION CLAMP10 : DINT
VAR_INPUT
    x : DINT;
END_VAR
    CLAMP10 := x;
    IF x > 10 THEN
        CLAMP10 := 10;
        RETURN;
    END_IF;
    CLAMP10 := CLAMP10 * 2;
END_FUNCTION

PROGRAM p
VAR
    r1 : DINT;
    r2 : DINT;
END_VAR
    r1 := CLAMP10(3);
    r2 := CLAMP10(50);
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 6);
    assert_eq!(dint(&state, 1), 10, "RETURN must return the value assigned so far");
}

#[test]
fn return_from_inside_nested_loops() {
    let src = r#"
FUNCTION SEEK : DINT
VAR_INPUT
    target : DINT;
END_VAR
VAR
    i : DINT;
    j : DINT;
END_VAR
    SEEK := -1;
    FOR i := 0 TO 9 DO
        j := 0;
        WHILE j < 10 DO
            IF i * 10 + j = target THEN
                SEEK := i * 100 + j;
                RETURN;
            END_IF;
            j := j + 1;
        END_WHILE;
    END_FOR;
    SEEK := -2;
END_FUNCTION

PROGRAM p
VAR
    hit : DINT;
    miss : DINT;
END_VAR
    hit := SEEK(47);
    miss := SEEK(500);
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 407);
    assert_eq!(dint(&state, 1), -2);
}

#[test]
fn return_from_case_and_repeat() {
    let src = r#"
FUNCTION PICK : DINT
VAR_INPUT
    sel : DINT;
END_VAR
VAR
    k : DINT;
END_VAR
    CASE sel OF
        1:
            PICK := 100;
            RETURN;
        2:
            REPEAT
                k := k + 1;
                IF k = 3 THEN
                    PICK := 200 + k;
                    RETURN;
                END_IF;
            UNTIL k > 10
            END_REPEAT;
    END_CASE;
    PICK := PICK + 1;
END_FUNCTION

PROGRAM p
VAR
    a : DINT;
    b : DINT;
    c : DINT;
END_VAR
    a := PICK(1);
    b := PICK(2);
    c := PICK(3);
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 100);
    assert_eq!(dint(&state, 1), 203);
    assert_eq!(dint(&state, 2), 1);
}

#[test]
fn return_in_function_block_body() {
    let src = r#"
FUNCTION_BLOCK FB
VAR_INPUT
    en : BOOL;
END_VAR
VAR_OUTPUT
    cnt : DINT;
    after : DINT;
END_VAR
    IF NOT en THEN
        RETURN;
    END_IF;
    cnt := cnt + 1;
    after := 7;
END_FUNCTION_BLOCK

PROGRAM p
VAR
    c : DINT;
    a : DINT;
    f : FB;
    g : FB;
END_VAR
    f(en := TRUE);
    g(en := FALSE);
    c := f.cnt * 100 + g.cnt;
    a := f.after * 100 + g.after;
END_PROGRAM
"#;
    let state = run_scans(src, 2);
    assert_eq!(dint(&state, 0), 200);
    assert_eq!(dint(&state, 1), 700);
}

#[test]
fn return_in_method() {
    let src = r#"
FUNCTION_BLOCK FB
VAR
    hits : DINT;
END_VAR
METHOD Check : DINT
VAR_INPUT
    x : DINT;
END_VAR
    Check := 1;
    IF x < 0 THEN
        RETURN;
    END_IF;
    hits := hits + 1;
    Check := 2;
END_METHOD
END_FUNCTION_BLOCK

PROGRAM p
VAR
    r1 : DINT;
    r2 : DINT;
    f : FB;
END_VAR
    r1 := f.Check(-5);
    r2 := f.Check(5);
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 1);
    assert_eq!(dint(&state, 1), 2);
}

#[test]
fn return_as_last_statement_and_twice() {
    // RETURN right before END_*, and two RETURNs in a row: both must still verify.
    let src = r#"
FUNCTION F : DINT
    F := 4;
    RETURN;
    RETURN;
END_FUNCTION

PROGRAM p
VAR
    r : DINT;
END_VAR
    r := F();
    RETURN;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 4);
}

// ---------------------------------------------------------------------------
// NOT is bitwise on every ANY_BIT/ANY_INT type except BOOL
// ---------------------------------------------------------------------------

fn byte(state: &[u8], idx: usize) -> u8 {
    state[idx]
}

#[test]
fn not_byte_is_bitwise() {
    let src = r#"
PROGRAM p
VAR
    r1 : BYTE;
    r2 : BYTE;
    r3 : BYTE;
    r4 : BYTE;
    b : BYTE := 16#0F;
    z : BYTE;
END_VAR
    r1 := NOT BYTE#16#0F;
    r2 := NOT b;
    r3 := NOT z;
    r4 := NOT (b AND 16#03);
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(byte(&state, 0), 0xF0);
    assert_eq!(byte(&state, 1), 0xF0);
    assert_eq!(byte(&state, 2), 0xFF);
    assert_eq!(byte(&state, 3), 0xFC);
}

#[test]
fn not_sint_is_bitwise() {
    let src = r#"
PROGRAM p
VAR
    r1 : SINT;
    r2 : SINT;
    s : SINT := 5;
END_VAR
    r1 := NOT s;
    r2 := NOT SINT#0;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(state[0] as i8, -6, "NOT SINT#5 = -6 (two's complement)");
    assert_eq!(state[1] as i8, -1);
}

#[test]
fn not_usint_is_bitwise_at_8_bits() {
    let src = r#"
PROGRAM p
VAR
    r1 : DINT;
    r2 : DINT;
    u : USINT := 16#0F;
END_VAR
    r1 := NOT u;
    u := NOT u;
    r2 := u;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 0xF0, "NOT USINT#16#0F = 16#F0");
    assert_eq!(dint(&state, 1), 0xF0);
}

#[test]
fn usint_and_uint_wrap_at_their_iec_width() {
    let src = r#"
PROGRAM p
VAR
    r1 : DINT;
    r2 : DINT;
    u8 : USINT := 200;
    u16 : UINT := 65000;
END_VAR
    u8 := u8 + 100;
    u16 := u16 + 1000;
    r1 := u8;
    r2 := u16;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 44, "USINT 200 + 100 wraps to 44");
    assert_eq!(dint(&state, 1), 464, "UINT 65000 + 1000 wraps to 464");
    // Layout: { i32 r1, i32 r2, i8 u8, pad, i16 u16 }
    assert_eq!(state[8], 44, "USINT occupies one byte");
    assert_eq!(u16::from_ne_bytes([state[10], state[11]]), 464);
}

#[test]
fn not_bool_stays_logical() {
    let src = r#"
PROGRAM p
VAR
    r1 : BOOL;
    r2 : BOOL;
    r3 : BOOL;
    r4 : BOOL;
    t : BOOL := TRUE;
    n : DINT := 3;
END_VAR
    r1 := NOT t;
    r2 := NOT FALSE;
    r3 := NOT (n > 5);
    r4 := NOT NOT t;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(&state[0..4], &[0, 1, 1, 1]);
}

#[test]
fn not_of_byte_function_and_method_results() {
    let src = r#"
FUNCTION MASK : BYTE
VAR_INPUT
    x : BYTE;
END_VAR
    MASK := x AND 16#3C;
END_FUNCTION

FUNCTION IS_BIG : BOOL
VAR_INPUT
    x : BYTE;
END_VAR
    IS_BIG := x > 100;
END_FUNCTION

FUNCTION_BLOCK FB
METHOD Get : BYTE
    Get := 16#81;
END_METHOD
END_FUNCTION_BLOCK

PROGRAM p
VAR
    r1 : BYTE;
    r2 : BOOL;
    r3 : BYTE;
    f : FB;
END_VAR
    r1 := NOT MASK(16#FF);
    r2 := NOT IS_BIG(200);
    r3 := NOT f.Get();
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(byte(&state, 0), 0xC3);
    assert_eq!(byte(&state, 1), 0);
    assert_eq!(byte(&state, 2), 0x7E);
}

#[test]
fn byte_bitwise_ops_compares_and_conversions() {
    let src = r#"
PROGRAM p
VAR
    r_and : BYTE;
    r_or : BYTE;
    r_xor : BYTE;
    gt : BOOL;
    eq : BOOL;
    to_bool : BOOL;
    to_byte : BYTE;
    pad : BYTE;
    wide : WORD;
    a : BYTE := 16#F0;
    b : BYTE := 16#3C;
    flag : BOOL := TRUE;
END_VAR
    r_and := a AND b;
    r_or := a OR b;
    r_xor := a XOR b;
    gt := a > b;
    eq := (a AND 16#80) = 16#80;
    to_bool := BYTE_TO_BOOL(16#10);
    to_byte := BOOL_TO_BYTE(flag) + 1;
    wide := NOT BYTE_TO_WORD(a);
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(byte(&state, 0), 0x30);
    assert_eq!(byte(&state, 1), 0xFC);
    assert_eq!(byte(&state, 2), 0xCC);
    assert_eq!(byte(&state, 3), 1);
    assert_eq!(byte(&state, 4), 1);
    assert_eq!(byte(&state, 5), 1, "BYTE_TO_BOOL(16#10) is TRUE");
    assert_eq!(byte(&state, 6), 2);
    let w = u16::from_ne_bytes([state[8], state[9]]);
    assert_eq!(w, 0xFF0F);
}

// ---------------------------------------------------------------------------
// `**` is EXPT: ANY_REAL result (IEC 61131-3 Table 23/29)
// ---------------------------------------------------------------------------

fn lreal(state: &[u8], idx: usize) -> f64 {
    let o = idx * 8;
    f64::from_ne_bytes(state[o..o + 8].try_into().unwrap())
}

fn real(state: &[u8], idx: usize) -> f32 {
    let o = idx * 4;
    f32::from_ne_bytes(state[o..o + 4].try_into().unwrap())
}

#[test]
fn integer_power_into_integer_variables() {
    let src = r#"
PROGRAM p
VAR
    r1 : DINT;
    r2 : DINT;
    r3 : DINT;
    r4 : DINT;
    r5 : DINT;
    r6 : DINT;
    r7 : DINT;
    i : INT := 3;
    n : DINT := 4;
    d : DINT := 3;
END_VAR
    r1 := 2 ** 10;
    r2 := i ** 2;
    r3 := d ** 19;
    r4 := 10 ** n;
    r5 := (-2) ** 3;
    r6 := 0 ** 0;
    r7 := 7 ** 1 + 1;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 1024);
    assert_eq!(dint(&state, 1), 9);
    assert_eq!(dint(&state, 2), 1_162_261_467, "3**19 exact: DINT base goes via LREAL");
    assert_eq!(dint(&state, 3), 10_000);
    assert_eq!(dint(&state, 4), -8);
    assert_eq!(dint(&state, 5), 1, "0 ** 0 = 1");
    assert_eq!(dint(&state, 6), 8);
}

#[test]
fn power_negative_exponent_is_real() {
    let src = r#"
PROGRAM p
VAR
    a : LREAL;
    b : LREAL;
    c : LREAL;
    k : INT := -2;
END_VAR
    a := 2 ** -1;
    b := 10 ** k;
    c := 0 ** -1;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(lreal(&state, 0), 0.5);
    assert!((lreal(&state, 1) - 0.01).abs() < 1e-7, "10 ** -2 = 0.01");
    assert!(lreal(&state, 2).is_infinite(), "0 ** -1 = +inf");
}

#[test]
fn real_and_mixed_power() {
    let src = r#"
PROGRAM p
VAR
    a : REAL;
    b : REAL;
    c : REAL;
    d : REAL;
    x : REAL := 9.0;
    e : REAL;
    bb : BYTE := 200;
END_VAR
    a := x ** 0.5;
    b := 1.5 ** 2;
    c := x ** 2;
    d := 2 ** 0.5;
    e := bb ** 1;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(real(&state, 0), 3.0);
    assert_eq!(real(&state, 1), 2.25);
    assert_eq!(real(&state, 2), 81.0);
    assert!((real(&state, 3) - std::f32::consts::SQRT_2).abs() < 1e-6);
    assert_eq!(real(&state, 5), 200.0, "BYTE 200 is 200.0, not -56.0");
}

#[test]
fn expt_function_matches_operator() {
    let src = r#"
PROGRAM p
VAR
    r1 : DINT;
    r2 : REAL;
    b : BYTE := 200;
END_VAR
    r1 := EXPT(3, 4);
    r2 := EXPT(b, 1);
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 81);
    assert_eq!(real(&state, 1), 200.0);
}

// ---------------------------------------------------------------------------
// String initial values are applied everywhere a variable comes into being
// ---------------------------------------------------------------------------

/// The NUL-terminated contents of a STRING buffer at `off`.
fn cstr(state: &[u8], off: usize) -> String {
    let end = state[off..].iter().position(|&b| b == 0).unwrap() + off;
    String::from_utf8(state[off..end].to_vec()).unwrap()
}

#[test]
fn program_string_initializer() {
    let src = r#"
PROGRAM p
VAR
    s : STRING[10] := 'abc';
    t : STRING[3] := 'truncated';
    d : STRING := 'default len';
    e : STRING[4];
END_VAR
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(cstr(&state, 0), "abc");
    assert_eq!(cstr(&state, 11), "tru", "truncated to the declared length");
    assert_eq!(cstr(&state, 15), "default len");
    assert_eq!(cstr(&state, 15 + 257), "");
}

#[test]
fn string_literal_assignment() {
    let src = r#"
PROGRAM p
VAR
    s : STRING[10] := 'initial';
    n : DINT;
END_VAR
    n := n + 1;
    IF n = 2 THEN
        s := 'hi';
    END_IF;
END_PROGRAM
"#;
    assert_eq!(cstr(&run_scans(src, 1), 0), "initial");
    assert_eq!(cstr(&run_scans(src, 2), 0), "hi", "old tail is cleared too");
}

#[test]
fn fb_and_function_string_locals() {
    let src = r#"
FUNCTION FIRST_CHAR_CODE : DINT
VAR
    tmp : STRING[5] := 'Z';
    other : STRING[5];
END_VAR
    other := tmp;
    FIRST_CHAR_CODE := LEN(other);
END_FUNCTION

FUNCTION_BLOCK FB
VAR_OUTPUT
    name : STRING[8] := 'motor';
END_VAR
END_FUNCTION_BLOCK

PROGRAM p
VAR
    out : STRING[8];
    n : DINT;
    f : FB;
END_VAR
    f();
    out := f.name;
    n := FIRST_CHAR_CODE();
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(cstr(&state, 0), "motor");
    assert_eq!(dint(&state, 3), 1, "FUNCTION local STRING initialized each call");
}

#[test]
fn global_initializers_that_are_not_plain_literals() {
    // The VAR_GLOBAL constant folder handles only bare literals; anything else
    // (a negative number, an expression) used to leave the global at zero.
    let src = r#"
VAR_GLOBAL
    gn : DINT := 2 * 21;
    gr : REAL := -1.5;
    gi : INT := -7;
END_VAR

PROGRAM p
VAR
    n : DINT;
    r : REAL;
    i : DINT;
END_VAR
    n := gn;
    r := gr;
    i := gi;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 42);
    assert_eq!(real(&state, 1), -1.5);
    assert_eq!(dint(&state, 2), -7);
}

#[test]
fn global_struct_and_array_string_initializers() {
    let src = r#"
TYPE Tag : STRUCT
    label : STRING[6] := 'pump';
    id : DINT := 7;
END_STRUCT
END_TYPE

VAR_GLOBAL
    g : STRING[6] := 'global';
    gw : WSTRING[4] := "wide";
    gn : DINT := 2 * 21;
    gr : REAL := -1.5;
    ga : ARRAY[0..1] OF STRING[3] := ['ab', 'cd'];
END_VAR

PROGRAM p
VAR
    a : STRING[6];
    b : STRING[6];
    c : STRING[3];
    d : STRING[3];
    e : STRING[3];
    pad : BYTE;
    w : WSTRING[4];
    n : DINT;
    r : REAL;
    t : Tag;
    la : ARRAY[1..2] OF STRING[3] := ['xy', 'z'];
    ch : CHAR := 'Q';
END_VAR
    a := g;
    b := t.label;
    c := ga[0];
    d := ga[1];
    e := la[2];
    w := gw;
    n := gn;
    r := gr;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(cstr(&state, 0), "global");
    assert_eq!(cstr(&state, 7), "pump", "STRUCT field default");
    assert_eq!(cstr(&state, 14), "ab", "VAR_GLOBAL ARRAY OF STRING");
    assert_eq!(cstr(&state, 18), "cd");
    assert_eq!(cstr(&state, 22), "z", "PROGRAM ARRAY OF STRING aggregate");
    // w: WSTRING[4] = [5 x i16] at the next 2-aligned offset after pad (26 -> 28)
    let w: Vec<u16> = (0..4)
        .map(|i| u16::from_ne_bytes([state[28 + 2 * i], state[29 + 2 * i]]))
        .collect();
    assert_eq!(String::from_utf16(&w).unwrap(), "wide");
    assert_eq!(dint(&state, 40 / 4), 42, "non-constant global initializer");
    assert_eq!(real(&state, 44 / 4), -1.5, "negative REAL global initializer");
    // t: { [7 x i8], i32 } at 48 -> id at 56; la at 60..68; ch at 68
    assert_eq!(dint(&state, 56 / 4), 7);
    assert_eq!(state[68], b'Q', "CHAR initializer");
}

#[test]
fn string_assignment_between_lengths_stays_in_bounds() {
    let src = r#"
FUNCTION_BLOCK FB
VAR_INPUT
    name : STRING[4];
END_VAR
VAR_OUTPUT
    guard : DINT := 77;
END_VAR
END_FUNCTION_BLOCK

PROGRAM p
VAR
    a : STRING[3];
    n : DINT := 5;
    b : STRING[20] := 'hello world';
    c : STRING[20];
    k : DINT;
    f : FB;
    fname : STRING[4];
END_VAR
    a := b;
    c := a;
    f(name := b);
    fname := f.name;
    k := f.guard;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(cstr(&state, 0), "hel", "truncated and NUL-terminated");
    assert_eq!(dint(&state, 1), 5, "the variable after a STRING[3] is not overwritten");
    assert_eq!(cstr(&state, 8), "hello world");
    assert_eq!(cstr(&state, 29), "hel", "short into long");
    assert_eq!(dint(&state, 52 / 4), 77, "FB input STRING[4] did not overrun");
}

#[test]
fn user_function_arguments_are_evaluated_once() {
    // The builtin dispatcher compiled every argument before finding out the callee
    // was not a builtin, and the user-FUNCTION path then compiled them again: a
    // side-effecting argument ran twice.
    let src = r#"
VAR_GLOBAL
    calls : DINT;
END_VAR

FUNCTION BUMP : DINT
    calls := calls + 1;
    BUMP := calls;
END_FUNCTION

FUNCTION ID : DINT
VAR_INPUT
    x : DINT;
END_VAR
    ID := x;
END_FUNCTION

PROGRAM p
VAR
    r : DINT;
    c : DINT;
END_VAR
    r := ID(BUMP());
    c := calls;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 1);
    assert_eq!(dint(&state, 1), 1, "BUMP ran exactly once");
}

#[test]
fn user_function_wins_over_nonstandard_builtin() {
    // FLOOR is not an IEC standard function; a FUNCTION the program declares with
    // that name is the one that must be called.
    let src = r#"
FUNCTION FLOOR : DINT
VAR_INPUT
    x : REAL;
END_VAR
    FLOOR := 99;
END_FUNCTION

PROGRAM p
VAR
    r : DINT;
END_VAR
    r := FLOOR(2.5);
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(dint(&state, 0), 99);
}

// ---------------------------------------------------------------------------
// Unsigned integers convert to REAL by their own signedness
// ---------------------------------------------------------------------------

#[test]
fn unsigned_to_real_conversions() {
    let src = r#"
PROGRAM p
VAR
    r1 : REAL;
    r2 : REAL;
    r3 : REAL;
    r4 : LREAL;
    b : BYTE := 200;
    w : WORD := 65000;
    u : USINT := 250;
END_VAR
    r1 := b;
    r2 := SQRT(b);
    r3 := MAX(1.5, u);
    r4 := w;
END_PROGRAM
"#;
    let state = run(src);
    assert_eq!(real(&state, 0), 200.0, "REAL := BYTE 200");
    assert!((real(&state, 1) - 200f32.sqrt()).abs() < 1e-4, "SQRT(BYTE 200)");
    assert_eq!(real(&state, 2), 250.0, "MAX(1.5, USINT 250)");
    assert_eq!(lreal(&state, 2), 65000.0, "LREAL := WORD 65000");
}
