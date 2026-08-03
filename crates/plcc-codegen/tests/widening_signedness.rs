// SPDX-License-Identifier: MPL-2.0

//! Widening an ANY_BIT / ANY_UNSIGNED value into a wider signed slot.
//!
//! Storing an assignment at the destination's width needs an extension, and the
//! extension's signedness comes from the **source**. A BYTE holding 16#FF is 255;
//! `acc := raw;` into a DINT must produce 255. Taking the sign from the DINT
//! destination produced -1 for every BYTE/WORD/DWORD/LWORD and every
//! USINT/UINT/UDINT/ULINT value assigned into a wider signed variable, and the same
//! for FB inputs.
//!
//! These read the actual state bytes back after a scan — no IR string matching.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;

/// Run `<pou>_init` then one `<pou>_scan` over a zeroed state block.
fn jit_scan(source: &str, pou: &str) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");

    let ctx = Context::create();
    let mut compiler = Compiler::new(&ctx, "widen");
    compiler.compile(&unit).expect("codegen failed");

    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT");

    let mut state = vec![0u8; 4096];
    let ptr = state.as_mut_ptr();
    if let Ok(addr) = ee.get_function_address(&format!("{pou}_init")) {
        let f: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(addr) };
        f(ptr);
    }
    let addr = ee
        .get_function_address(&format!("{pou}_scan"))
        .expect("scan function");
    let f: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(addr) };
    f(ptr);
    state
}

fn read_i32(state: &[u8], offset: usize) -> i32 {
    i32::from_ne_bytes(state[offset..offset + 4].try_into().expect("4 bytes"))
}

fn read_i64(state: &[u8], offset: usize) -> i64 {
    i64::from_ne_bytes(state[offset..offset + 8].try_into().expect("8 bytes"))
}

const ANY_BIT: &str = r#"
PROGRAM WIDEN
VAR
    raw_b   : BYTE  := 16#FF;
    raw_w   : WORD  := 16#FFFF;
    raw_d   : DWORD := 16#FFFFFFFF;
    acc_b   : DINT;
    acc_w   : DINT;
    acc_d   : LINT;
    total   : LINT;
END_VAR
    acc_b := raw_b;
    acc_w := raw_w;
    acc_d := raw_d;
    total := acc_b + acc_w;
END_PROGRAM
"#;

#[test]
fn any_bit_widening_into_signed_accumulators() {
    // { i8 raw_b, pad, i16 raw_w, i32 raw_d, i32 acc_b, i32 acc_w, i64 acc_d, i64 total }
    let state = jit_scan(ANY_BIT, "widen");
    assert_eq!(read_i32(&state, 8), 255, "BYTE 16#FF widened into DINT");
    assert_eq!(
        read_i32(&state, 12),
        65535,
        "WORD 16#FFFF widened into DINT"
    );
    assert_eq!(
        read_i64(&state, 16),
        4294967295,
        "DWORD 16#FFFFFFFF widened into LINT"
    );
    assert_eq!(read_i64(&state, 24), 65790, "255 + 65535");
}

const ANY_UNSIGNED: &str = r#"
PROGRAM UWIDEN
VAR
    u8v  : USINT := 255;
    u16v : UINT  := 65535;
    u32v : UDINT := 16#FFFFFFFF;
    a    : DINT;
    b    : DINT;
    c    : LINT;
END_VAR
    a := u8v;
    b := u16v;
    c := u32v;
END_PROGRAM
"#;

#[test]
fn any_unsigned_widening_into_signed_accumulators() {
    // This backend stores USINT in i16 and UINT in i32 — one size up — so only the
    // USINT->DINT and UDINT->LINT steps are actual widenings.
    // { i16 u8v, i32 u16v, i32 u32v, i32 a, i32 b, i64 c }
    let state = jit_scan(ANY_UNSIGNED, "uwiden");
    assert_eq!(read_i32(&state, 12), 255, "USINT 255 widened into DINT");
    assert_eq!(read_i32(&state, 16), 65535, "UINT 65535 into DINT");
    assert_eq!(
        read_i64(&state, 24),
        4294967295,
        "UDINT 16#FFFFFFFF widened into LINT"
    );
}

const SIGNED_SOURCE: &str = r#"
PROGRAM SWIDEN
VAR
    s8v  : SINT := -1;
    s16v : INT  := -2;
    s32v : DINT := -3;
    a    : DINT;
    b    : DINT;
    c    : LINT;
END_VAR
    a := s8v;
    b := s16v;
    c := s32v;
END_PROGRAM
"#;

#[test]
fn signed_sources_still_sign_extend() {
    // The fix must not flip the other way: a negative SINT stays negative.
    // { i8 s8v, i16 s16v, i32 s32v, i32 a, i32 b, i64 c }
    let state = jit_scan(SIGNED_SOURCE, "swiden");
    assert_eq!(read_i32(&state, 8), -1, "SINT -1 widened into DINT");
    assert_eq!(read_i32(&state, 12), -2, "INT -2 widened into DINT");
    assert_eq!(read_i64(&state, 16), -3, "DINT -3 widened into LINT");
}

const FB_INPUT: &str = r#"
FUNCTION_BLOCK Widener
VAR_INPUT
    inp : DINT;
    big : LINT;
END_VAR
VAR_OUTPUT
    outp : DINT;
    bigout : LINT;
END_VAR
    outp := inp;
    bigout := big;
END_FUNCTION_BLOCK

PROGRAM FBW
VAR
    raw_b : BYTE  := 16#FF;
    raw_d : DWORD := 16#FFFFFFFF;
    w : Widener;
    seen : DINT;
    seenbig : LINT;
END_VAR
    w(inp := raw_b, big := raw_d);
    seen := w.outp;
    seenbig := w.bigout;
END_PROGRAM
"#;

#[test]
fn fb_inputs_widen_by_the_arguments_signedness() {
    // { i8 raw_b, i32 raw_d, { i32 inp, i64 big, i32 outp, i64 bigout } w,
    //   i32 seen, i64 seenbig }
    let state = jit_scan(FB_INPUT, "fbw");
    assert_eq!(
        read_i32(&state, 8),
        255,
        "a BYTE 16#FF bound to a DINT input must arrive as 255"
    );
    assert_eq!(
        read_i64(&state, 16),
        4294967295,
        "a DWORD 16#FFFFFFFF bound to a LINT input must arrive as 4294967295"
    );
    assert_eq!(read_i32(&state, 40), 255, "the FB echoed 255 back out");
    assert_eq!(read_i64(&state, 48), 4294967295, "and 4294967295");
}

#[test]
fn comparison_results_are_not_sign_extended() {
    // A comparison yields BOOL. If widening took its sign from an ANY_BIT operand, or
    // sign-extended an i1, `flag` would come back as something other than 1.
    let source = r#"
PROGRAM CMPW
VAR
    raw : BYTE := 16#0F;
    lo  : BYTE := 16#01;
    flag : BOOL;
    n : DINT;
END_VAR
    flag := raw > lo;
    IF flag THEN
        n := 1;
    ELSE
        n := 0;
    END_IF;
END_PROGRAM
"#;
    // { i8 raw, i8 lo, i8 flag, pad, i32 n }
    let state = jit_scan(source, "cmpw");
    assert_eq!(
        state[2], 1,
        "BOOL TRUE must be stored as 1, not sign-extended from i1 to 16#FF"
    );
    assert_eq!(read_i32(&state, 4), 1, "the IF must take the TRUE branch");
}
