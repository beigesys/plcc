// SPDX-License-Identifier: MPL-2.0

//! JIT tests for the conversions and string builtins OSCAT leans on:
//! DWORD⇄TIME (milliseconds), REAL⇄DWORD, and IEC `REPLACE(IN1, IN2, L, P)`.
//!
//! TIME is held as i64 nanoseconds; its numeric conversions speak milliseconds.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;

/// Compile, seed the state with `init`, run one scan, return the state bytes.
fn run(source: &str, scan_fn: &str, size: usize, init: impl FnOnce(&mut [u8])) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let context = Context::create();
    let mut compiler = Compiler::new(&context, "test");
    compiler.compile(&unit).expect("codegen failed");
    compiler
        .module()
        .verify()
        .unwrap_or_else(|e| panic!("module failed verification: {e}"));
    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("JIT");
    let mut state = vec![0u8; size];
    init(&mut state);
    let addr = ee.get_function_address(scan_fn).expect("scan fn");
    let scan: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(addr) };
    scan(state.as_mut_ptr());
    state
}

fn i64_at(s: &[u8], o: usize) -> i64 {
    i64::from_ne_bytes(s[o..o + 8].try_into().unwrap())
}
fn u32_at(s: &[u8], o: usize) -> u32 {
    u32::from_ne_bytes(s[o..o + 4].try_into().unwrap())
}
fn i32_at(s: &[u8], o: usize) -> i32 {
    i32::from_ne_bytes(s[o..o + 4].try_into().unwrap())
}
fn f32_at(s: &[u8], o: usize) -> f32 {
    f32::from_ne_bytes(s[o..o + 4].try_into().unwrap())
}
fn f64_at(s: &[u8], o: usize) -> f64 {
    f64::from_ne_bytes(s[o..o + 8].try_into().unwrap())
}
fn str_at(s: &[u8], o: usize, cap: usize) -> String {
    let b = &s[o..o + cap];
    let end = b.iter().position(|&c| c == 0).unwrap_or(cap);
    String::from_utf8_lossy(&b[..end]).into_owned()
}
fn put_str(s: &mut [u8], o: usize, v: &str) {
    s[o..o + v.len()].copy_from_slice(v.as_bytes());
    s[o + v.len()] = 0;
}

const NS_PER_MS: i64 = 1_000_000;

const TIME_SRC: &str = r#"
PROGRAM T
VAR
    t_in : TIME;
    t_out : TIME;
    t_udint : TIME;
    t_dint : TIME;
    wide : LINT;
    t_lit : TIME;
    d_in : DWORD;
    d_out : DWORD;
    di_out : DINT;
    di_in : DINT;
    lit_ms : DWORD;
    r_out : REAL;
END_VAR
    t_out := DWORD_TO_TIME(d_in);
    t_udint := UDINT_TO_TIME(d_in);
    t_dint := DINT_TO_TIME(di_in);
    d_out := TIME_TO_DWORD(t_in);
    wide := TIME_TO_DWORD(t_in);
    di_out := TIME_TO_DINT(t_in);
    r_out := TIME_TO_REAL(t_in);
    t_lit := DWORD_TO_TIME(1500);
    lit_ms := TIME_TO_DWORD(T#2s250ms);
END_PROGRAM
"#;
// Layout: 6 x i64 at 0..48, then i32s at 48, 52, 56, 60, 64, then REAL at 68.
const T_IN: usize = 0;
const T_OUT: usize = 8;
const T_UDINT: usize = 16;
const T_DINT: usize = 24;
const WIDE: usize = 32;
const T_LIT: usize = 40;
const D_IN: usize = 48;
const D_OUT: usize = 52;
const DI_OUT: usize = 56;
const DI_IN: usize = 60;
const LIT_MS: usize = 64;
const R_OUT: usize = 68;

fn run_time(t_in_ns: i64, d_in: u32, di_in: i32) -> Vec<u8> {
    run(TIME_SRC, "t_scan", 80, |s| {
        s[T_IN..T_IN + 8].copy_from_slice(&t_in_ns.to_ne_bytes());
        s[D_IN..D_IN + 4].copy_from_slice(&d_in.to_ne_bytes());
        s[DI_IN..DI_IN + 4].copy_from_slice(&di_in.to_ne_bytes());
    })
}

#[test]
fn dword_to_time_is_milliseconds() {
    let s = run_time(0, 1500, 0);
    assert_eq!(i64_at(&s, T_OUT), 1500 * NS_PER_MS);
    assert_eq!(i64_at(&s, T_UDINT), 1500 * NS_PER_MS);
    assert_eq!(i64_at(&s, T_LIT), 1500 * NS_PER_MS, "literal argument");
}

#[test]
fn dword_to_time_is_unsigned_above_2_31() {
    // 3e9 ms read as a signed DINT would be about -1.29e9 ms.
    let s = run_time(0, 3_000_000_000, 0);
    assert_eq!(i64_at(&s, T_OUT), 3_000_000_000 * NS_PER_MS);
    let s = run_time(0, u32::MAX, 0);
    assert_eq!(i64_at(&s, T_OUT), u32::MAX as i64 * NS_PER_MS);
    assert_eq!(i64_at(&s, T_UDINT), u32::MAX as i64 * NS_PER_MS);
}

#[test]
fn dint_to_time_keeps_sign() {
    let s = run_time(0, 0, -250);
    assert_eq!(i64_at(&s, T_DINT), -250 * NS_PER_MS);
}

#[test]
fn time_to_dword_is_milliseconds_truncated() {
    // 1234 ms and 999_999 ns: the sub-millisecond part is dropped.
    let s = run_time(1234 * NS_PER_MS + 999_999, 0, 0);
    assert_eq!(u32_at(&s, D_OUT), 1234);
    assert_eq!(i32_at(&s, DI_OUT), 1234);
    assert_eq!(i64_at(&s, WIDE), 1234);
    assert_eq!(u32_at(&s, LIT_MS), 2250, "TIME_TO_DWORD(T#2s250ms)");
    assert!((f32_at(&s, R_OUT) - 1234.999_999).abs() < 1e-3);
}

#[test]
fn time_to_dword_above_2_31_widens_unsigned() {
    let s = run_time(3_000_000_000 * NS_PER_MS, 0, 0);
    assert_eq!(u32_at(&s, D_OUT), 3_000_000_000);
    // Stored into a LINT the DWORD must zero-extend, not sign-extend.
    assert_eq!(i64_at(&s, WIDE), 3_000_000_000);
}

#[test]
fn dword_time_round_trip() {
    for ms in [0u32, 1, 1000, 86_400_000, 2_147_483_648, u32::MAX] {
        let s = run_time(0, ms, 0);
        let t = i64_at(&s, T_OUT);
        let s = run_time(t, 0, 0);
        assert_eq!(u32_at(&s, D_OUT), ms, "round trip of {ms} ms");
    }
}

const REAL_SRC: &str = r#"
PROGRAM R
VAR
    lr_in : LREAL;
    lr_out : LREAL;
    t_out : TIME;
    r_in : REAL;
    d_in : DWORD;
    d_out : DWORD;
    ld_out : DWORD;
    r_out : REAL;
    u_out : UDINT;
    w_in : WORD;
    wr_out : REAL;
    wide : LINT;
END_VAR
    d_out := REAL_TO_DWORD(r_in);
    u_out := REAL_TO_UDINT(r_in);
    ld_out := LREAL_TO_DWORD(lr_in);
    r_out := DWORD_TO_REAL(d_in);
    lr_out := DWORD_TO_LREAL(d_in);
    wr_out := WORD_TO_REAL(w_in);
    t_out := REAL_TO_TIME(r_in);
    wide := REAL_TO_DWORD(r_in);
END_PROGRAM
"#;
// Layout: f64 0, f64 8, i64 16, f32 24, i32 28, i32 32, i32 36, f32 40, i32 44,
// i16 48, f32 52, i64 56.
const LR_IN: usize = 0;
const LR_OUT: usize = 8;
const RT_OUT: usize = 16;
const R_IN: usize = 24;
const RD_IN: usize = 28;
const RD_OUT: usize = 32;
const LD_OUT: usize = 36;
const RR_OUT: usize = 40;
const U_OUT: usize = 44;
const W_IN: usize = 48;
const WR_OUT: usize = 52;
const R_WIDE: usize = 56;

fn run_real(r_in: f32, lr_in: f64, d_in: u32, w_in: u16) -> Vec<u8> {
    run(REAL_SRC, "r_scan", 64, |s| {
        s[LR_IN..LR_IN + 8].copy_from_slice(&lr_in.to_ne_bytes());
        s[R_IN..R_IN + 4].copy_from_slice(&r_in.to_ne_bytes());
        s[RD_IN..RD_IN + 4].copy_from_slice(&d_in.to_ne_bytes());
        s[W_IN..W_IN + 2].copy_from_slice(&w_in.to_ne_bytes());
    })
}

#[test]
fn real_to_dword_truncates_like_real_to_dint() {
    let s = run_real(2.7, 9.99, 0, 0);
    assert_eq!(u32_at(&s, RD_OUT), 2);
    assert_eq!(u32_at(&s, U_OUT), 2);
    assert_eq!(u32_at(&s, LD_OUT), 9);
}

#[test]
fn real_to_dword_above_2_31() {
    // 3e9 is exact in f32; an fptosi to i32 would be poison here.
    let s = run_real(3.0e9, 4_000_000_000.9, 0, 0);
    assert_eq!(u32_at(&s, RD_OUT), 3_000_000_000);
    assert_eq!(u32_at(&s, LD_OUT), 4_000_000_000);
    assert_eq!(
        i64_at(&s, R_WIDE),
        3_000_000_000,
        "DWORD result widens unsigned"
    );
}

#[test]
fn dword_to_real_is_unsigned() {
    let s = run_real(0.0, 0.0, 4_000_000_000, 0);
    assert_eq!(f32_at(&s, RR_OUT), 4.0e9);
    assert_eq!(f64_at(&s, LR_OUT), 4.0e9);
    let s = run_real(0.0, 0.0, u32::MAX, 0);
    assert_eq!(f64_at(&s, LR_OUT), 4_294_967_295.0);
    assert!(f32_at(&s, RR_OUT) > 0.0);
}

#[test]
fn word_to_real_is_unsigned() {
    let s = run_real(0.0, 0.0, 0, 65_535);
    assert_eq!(f32_at(&s, WR_OUT), 65_535.0);
}

#[test]
fn real_to_time_is_milliseconds() {
    let s = run_real(1.5, 0.0, 0, 0);
    assert_eq!(i64_at(&s, RT_OUT), 1_500_000);
}

const INT_SRC: &str = r#"
PROGRAM I
VAR
    i_in : INT;
    d_out : DWORD;
    wide : LINT;
    d_in : DWORD;
    b_out : BYTE;
    x : BOOL;
    bd_out : DWORD;
END_VAR
    d_out := INT_TO_DWORD(i_in);
    wide := DWORD_TO_BYTE(d_in);
    b_out := DWORD_TO_BYTE(d_in);
    bd_out := BOOL_TO_DWORD(x);
END_PROGRAM
"#;

#[test]
fn integer_siblings() {
    // i16 0, pad, i32 4, i64 8, i32 16, i8 20, i8 21, pad, i32 24.
    let s = run(INT_SRC, "i_scan", 32, |s| {
        s[0..2].copy_from_slice(&(-2i16).to_ne_bytes());
        s[16..20].copy_from_slice(&0x1234_56F0u32.to_ne_bytes());
        s[21] = 1;
    });
    assert_eq!(u32_at(&s, 4), 0xFFFF_FFFE, "INT_TO_DWORD sign-extends");
    assert_eq!(s[20], 0xF0, "DWORD_TO_BYTE truncates");
    assert_eq!(i64_at(&s, 8), 0xF0, "BYTE result widens unsigned");
    assert_eq!(u32_at(&s, 24), 1);
}

// ===== REPLACE(IN1, IN2, L, P) =====

const REPLACE_SRC: &str = r#"
PROGRAM Rep
VAR
    src : STRING[20];
    a : STRING[20];
    b : STRING[20];
    c : STRING[20];
    s : STRING[20];
    e : STRING[5];
    ins : STRING[4];
    d : STRING[20];
    f : STRING[20];
    l : INT;
    p : INT;
END_VAR
    a := REPLACE(src, 'XY', 2, 1);
    b := REPLACE(src, '!!', 0, 11);
    c := REPLACE(src, 'Z', 99, 8);
    s := REPLACE(s, '', 1, 5);
    e := REPLACE(src, 'abc', 0, 3);
    d := REPLACE(src, ins, 3, 4);
    f := REPLACE(IN1 := src, IN2 := ins, L := l, P := p);
END_PROGRAM
"#;
const SRC: usize = 0;
const A: usize = 21;
const B: usize = 42;
const C: usize = 63;
const S: usize = 84;
const E: usize = 105;
const INS: usize = 111;
const D: usize = 116;
const F: usize = 137;
const L: usize = 158;
const P: usize = 160;

fn run_replace(l: i16, p: i16) -> Vec<u8> {
    run(REPLACE_SRC, "rep_scan", 168, |st| {
        put_str(st, SRC, "HelloWorld");
        put_str(st, S, "HelloWorld");
        put_str(st, INS, "ab");
        st[L..L + 2].copy_from_slice(&l.to_ne_bytes());
        st[P..P + 2].copy_from_slice(&p.to_ne_bytes());
    })
}

#[test]
fn replace_at_position_one() {
    let st = run_replace(0, 1);
    // 'He' (L = 2 from P = 1) becomes 'XY'.
    assert_eq!(str_at(&st, A, 21), "XYlloWorld");
    // L = 0 at P = 1 is a pure insert at the front.
    assert_eq!(str_at(&st, F, 21), "abHelloWorld");
}

#[test]
fn replace_at_end_appends() {
    let st = run_replace(0, 0);
    assert_eq!(str_at(&st, B, 21), "HelloWorld!!", "P = LEN + 1");
    // L running past the end removes only what is there.
    assert_eq!(str_at(&st, C, 21), "HelloWoZ");
}

#[test]
fn replace_in_the_middle_with_a_variable() {
    let st = run_replace(0, 0);
    assert_eq!(str_at(&st, D, 21), "Helaborld");
}

#[test]
fn replace_onto_its_own_input() {
    // `s := REPLACE(s, '', 1, pos)` is how OSCAT's TRIM deletes a character.
    let st = run_replace(0, 0);
    assert_eq!(str_at(&st, S, 21), "HellWorld");
}

#[test]
fn replace_truncates_to_destination_length() {
    let st = run_replace(0, 0);
    assert_eq!(str_at(&st, E, 6), "Heabc");
}

#[test]
fn replace_clamps_out_of_range_position_and_length() {
    // P past the end appends; P below 1 is treated as 1; negative L as 0.
    let st = run_replace(2, 50);
    assert_eq!(str_at(&st, F, 21), "HelloWorldab");
    let st = run_replace(-3, -7);
    assert_eq!(str_at(&st, F, 21), "abHelloWorld");
    let st = run_replace(10, 1);
    assert_eq!(str_at(&st, F, 21), "ab");
}
