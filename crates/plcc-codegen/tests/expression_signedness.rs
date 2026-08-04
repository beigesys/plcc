// SPDX-License-Identifier: MPL-2.0

//! Signedness across the general expression path.
//!
//! Assignment, FB inputs, method arguments and FOR bounds already widened by the
//! *source* type's signedness. The binary-operator path, the shift/rotate builtins
//! and the FOR predicate did not, so every ANY_BIT and ANY_UNSIGNED value above its
//! type's signed range produced a silently wrong answer:
//!
//!   * `a : BYTE := 200; b : BYTE := 100; a > b` was false (SGT on i8: -56 > 100).
//!   * `a : BYTE := 200; b : BYTE := 2; a / b` was 228 (sdiv on i8).
//!   * `a : BYTE := 250; b : SINT := 10; a + b` was 4 (sext of 250 to -6).
//!   * `SHR(a, 1)` with `a : BYTE := 254` was 32767 (sext to i16 then lshr).
//!
//! Every assertion here reads the state bytes back after a real JIT scan.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;

/// Run `<pou>_init` then one `<pou>_scan` over a zeroed state block.
fn jit_scan(source: &str, pou: &str) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");

    let ctx = Context::create();
    let mut compiler = Compiler::new(&ctx, "exprsign");
    compiler.compile(&unit).expect("codegen failed");

    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT");

    let mut state = vec![0u8; 8192];
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

/// Read `n` consecutive LINT results starting at `base`.
fn read_lints(state: &[u8], base: usize, n: usize) -> Vec<i64> {
    (0..n)
        .map(|i| {
            let o = base + i * 8;
            i64::from_ne_bytes(state[o..o + 8].try_into().expect("8 bytes"))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// S1 — unsigned comparisons must use unsigned predicates
// ---------------------------------------------------------------------------

/// Every operand pair here is same-typed and unsigned, with the left value above
/// the type's signed range. A signed predicate reads it as negative.
const CMP: &str = r#"
PROGRAM CMP
VAR
    r : ARRAY[0..15] OF LINT;
    ab : BYTE  := 200;      bb : BYTE  := 100;
    aw : WORD  := 16#FFFF;  bw : WORD  := 1;
    ad : DWORD := 16#FFFFFFFF; bd : DWORD := 1;
    al : LWORD := 16#FFFFFFFFFFFFFFFF; bl : LWORD := 1;
    au : USINT := 200;      bu : USINT := 100;
    ai : UINT  := 65535;    bi : UINT  := 1;
    ax : UDINT := 4000000000; bx : UDINT := 1;
    an : ULINT := 16#FFFFFFFFFFFFFFFF; bn : ULINT := 1;
    si : SINT := -5;        sj : SINT := 3;
END_VAR
    IF ab > bb THEN r[0] := 1; END_IF;
    IF aw > bw THEN r[1] := 1; END_IF;
    IF ad > bd THEN r[2] := 1; END_IF;
    IF al > bl THEN r[3] := 1; END_IF;
    IF au > bu THEN r[4] := 1; END_IF;
    IF ai > bi THEN r[5] := 1; END_IF;
    IF ax > bx THEN r[6] := 1; END_IF;
    IF an > bn THEN r[7] := 1; END_IF;
    IF ab >= bb THEN r[8] := 1; END_IF;
    IF bb < ab THEN r[9] := 1; END_IF;
    IF bb <= ab THEN r[10] := 1; END_IF;
    (* signed types keep signed predicates *)
    IF si < sj THEN r[11] := 1; END_IF;
    IF sj > si THEN r[12] := 1; END_IF;
    (* mixed signed/unsigned: evaluated in a common signed type wide enough
       for both ranges, so 250 > -5 and -5 < 250 *)
    IF ab > si THEN r[13] := 1; END_IF;
    IF si < ab THEN r[14] := 1; END_IF;
    (* an unsigned value against a plain literal *)
    IF ab > 100 THEN r[15] := 1; END_IF;
END_PROGRAM
"#;

#[test]
fn unsigned_comparisons_use_unsigned_predicates() {
    let state = jit_scan(CMP, "cmp");
    let r = read_lints(&state, 0, 16);
    let names = [
        "BYTE 200 > 100",
        "WORD 16#FFFF > 1",
        "DWORD 16#FFFFFFFF > 1",
        "LWORD 16#FF..FF > 1",
        "USINT 200 > 100",
        "UINT 65535 > 1",
        "UDINT 4000000000 > 1",
        "ULINT 16#FF..FF > 1",
        "BYTE 200 >= 100",
        "BYTE 100 < 200",
        "BYTE 100 <= 200",
        "SINT -5 < 3",
        "SINT 3 > -5",
        "BYTE 200 > SINT -5",
        "SINT -5 < BYTE 200",
        "BYTE 200 > literal 100",
    ];
    for (i, name) in names.iter().enumerate() {
        assert_eq!(r[i], 1, "{name} should be TRUE, got r[{i}]={}", r[i]);
    }
}

// ---------------------------------------------------------------------------
// S2 — unsigned division and MOD
// ---------------------------------------------------------------------------

const DIV: &str = r#"
PROGRAM DIVP
VAR
    r : ARRAY[0..11] OF LINT;
    ab : BYTE  := 200;
    aw : WORD  := 16#FFF0;
    ad : DWORD := 16#FFFFFFF0;
    al : LWORD := 16#FFFFFFFFFFFFFFF0;
    au : USINT := 200;
    ai : UINT  := 65520;
    ax : UDINT := 4000000000;
    an : ULINT := 16#FFFFFFFFFFFFFFF0;
    two_b : BYTE := 2;   two_w : WORD := 2;
    two_d : DWORD := 2;  two_l : LWORD := 2;
    two_u : USINT := 2;  two_i : UINT := 2;
    two_x : UDINT := 2;  two_n : ULINT := 2;
    sa : SINT := -100;   sb : SINT := 3;
END_VAR
    r[0] := ab / two_b;
    r[1] := aw / two_w;
    r[2] := ad / two_d;
    r[3] := ax / two_x;
    r[4] := au / two_u;
    r[5] := ai / two_i;
    r[6] := ab MOD two_b;
    r[7] := aw MOD 7;
    r[8] := ad MOD two_d;
    r[9] := ai MOD two_i;
    (* signed division keeps signed semantics *)
    r[10] := sa / sb;
    r[11] := sa MOD sb;
END_PROGRAM
"#;

#[test]
fn unsigned_division_and_mod() {
    let state = jit_scan(DIV, "divp");
    let r = read_lints(&state, 0, 12);
    assert_eq!(r[0], 100, "BYTE 200 / 2");
    assert_eq!(r[1], 32760, "WORD 16#FFF0 / 2");
    assert_eq!(r[2], 2147483640, "DWORD 16#FFFFFFF0 / 2");
    assert_eq!(r[3], 2000000000, "UDINT 4000000000 / 2");
    assert_eq!(r[4], 100, "USINT 200 / 2");
    assert_eq!(r[5], 32760, "UINT 65520 / 2");
    assert_eq!(r[6], 0, "BYTE 200 MOD 2");
    assert_eq!(r[7], 65520 % 7, "WORD 16#FFF0 MOD 7");
    assert_eq!(r[8], 0, "DWORD 16#FFFFFFF0 MOD 2");
    assert_eq!(r[9], 0, "UINT 65520 MOD 2");
    assert_eq!(r[10], -33, "SINT -100 / 3 stays signed");
    assert_eq!(r[11], -1, "SINT -100 MOD 3 stays signed");
}

// ---------------------------------------------------------------------------
// S3 — mixed-width arithmetic widens each operand by its own signedness
// ---------------------------------------------------------------------------

const MIXED: &str = r#"
PROGRAM MIXED
VAR
    r : ARRAY[0..9] OF LINT;
    ab : BYTE  := 250;
    aw : WORD  := 65000;
    ad : DWORD := 4000000000;
    au : USINT := 250;
    ai : UINT  := 65000;
    ax : UDINT := 4000000000;
    sb : SINT := 10;
    si : INT  := 10;
    sd : DINT := 10;
END_VAR
    r[0] := ab + sb;
    r[1] := aw + si;
    r[2] := ad + sd;
    r[3] := au + sb;
    r[4] := ai + si;
    r[5] := ax + sd;
    r[6] := ab * sb;
    r[7] := aw - si;
    r[8] := ab + 10;
    r[9] := aw + 10;
END_PROGRAM
"#;

#[test]
fn mixed_width_arithmetic_widens_by_own_signedness() {
    let state = jit_scan(MIXED, "mixed");
    let r = read_lints(&state, 0, 10);
    assert_eq!(r[0], 260, "BYTE 250 + SINT 10");
    assert_eq!(r[1], 65010, "WORD 65000 + INT 10");
    assert_eq!(r[2], 4000000010, "DWORD 4000000000 + DINT 10");
    assert_eq!(r[3], 260, "USINT 250 + SINT 10");
    assert_eq!(r[4], 65010, "UINT 65000 + INT 10");
    assert_eq!(r[5], 4000000010, "UDINT 4000000000 + DINT 10");
    assert_eq!(r[6], 2500, "BYTE 250 * SINT 10");
    assert_eq!(r[7], 64990, "WORD 65000 - INT 10");
    assert_eq!(r[8], 260, "BYTE 250 + literal 10");
    assert_eq!(r[9], 65010, "WORD 65000 + literal 10");
}

// ---------------------------------------------------------------------------
// S4 — shifts and rotates
// ---------------------------------------------------------------------------

const SHIFT: &str = r#"
PROGRAM SHIFTP
VAR
    r : ARRAY[0..11] OF LINT;
    ab : BYTE  := 254;
    aw : WORD  := 16#FF00;
    ad : DWORD := 16#FF000000;
    al : LWORD := 16#FF00000000000000;
    au : USINT := 254;
    ai : UINT  := 16#FF00;
    hb : BYTE := 16#81;
    hw : WORD := 16#8001;
END_VAR
    r[0] := SHR(ab, 1);
    r[1] := SHR(aw, 4);
    r[2] := SHR(ad, 8);
    r[3] := SHR(au, 1);
    r[4] := SHR(ai, 4);
    r[5] := SHL(ab, 1);
    r[6] := SHL(hb, 1);
    r[7] := ROL(hb, 1);
    r[8] := ROR(hb, 1);
    r[9] := ROL(hw, 1);
    r[10] := ROR(hw, 1);
    r[11] := SHR(hb, 4);
END_PROGRAM
"#;

#[test]
fn shifts_and_rotates_are_logical_on_any_bit() {
    let state = jit_scan(SHIFT, "shiftp");
    let r = read_lints(&state, 0, 12);
    assert_eq!(r[0], 127, "SHR(BYTE 254, 1)");
    assert_eq!(r[1], 0x0FF0, "SHR(WORD 16#FF00, 4)");
    assert_eq!(r[2], 0x00FF0000, "SHR(DWORD 16#FF000000, 8)");
    assert_eq!(r[3], 127, "SHR(USINT 254, 1)");
    assert_eq!(r[4], 0x0FF0, "SHR(UINT 16#FF00, 4)");
    assert_eq!(r[5], 0xFC, "SHL(BYTE 254, 1) truncated to BYTE");
    assert_eq!(r[6], 0x02, "SHL(BYTE 16#81, 1) truncated to BYTE");
    assert_eq!(r[7], 0x03, "ROL(BYTE 16#81, 1)");
    assert_eq!(r[8], 0xC0, "ROR(BYTE 16#81, 1)");
    assert_eq!(r[9], 0x0003, "ROL(WORD 16#8001, 1)");
    assert_eq!(r[10], 0xC000, "ROR(WORD 16#8001, 1)");
    assert_eq!(r[11], 0x08, "SHR(BYTE 16#81, 4)");
}
// ---------------------------------------------------------------------------
// FOR predicate signedness, and a step held in a variable
// ---------------------------------------------------------------------------

const FORP: &str = r#"
PROGRAM FORP
VAR
    r : ARRAY[0..6] OF LINT;
    n  : LINT;
    i  : BYTE;
    lim : BYTE := 200;
    k  : USINT;
    ulim : USINT := 200;
    j : INT;
    down : INT := -1;
    down50 : INT := -50;
    up2 : INT := 2;
END_VAR
    (* the control variable crosses its type's signed range: SLE reads 200 as
       -56, so 100 <= 200 came out false and the loop never ran *)
    n := 0;
    FOR i := 100 TO lim DO
        n := n + 1;
    END_FOR;
    r[0] := n;

    n := 0;
    FOR k := 100 TO ulim DO
        n := n + 1;
    END_FOR;
    r[1] := n;

    (* a negative step held in a variable: nothing syntactic says it is negative *)
    n := 0;
    FOR j := 5 TO 1 BY down DO
        n := n + 1;
    END_FOR;
    r[2] := n;

    (* a positive step held in a variable still ascends *)
    n := 0;
    FOR j := 1 TO 10 BY up2 DO
        n := n + 1;
    END_FOR;
    r[3] := n;

    (* both at once: unsigned control variable, negative variable step *)
    n := 0;
    FOR i := 200 TO 100 BY down50 DO
        n := n + 1;
    END_FOR;
    r[4] := n;

    (* the plain cases must not regress *)
    n := 0;
    FOR j := 1 TO 3 DO
        n := n + 1;
    END_FOR;
    r[5] := n;

    n := 0;
    FOR j := 5 TO 1 BY -1 DO
        n := n + 1;
    END_FOR;
    r[6] := n;
END_PROGRAM
"#;

#[test]
fn for_loop_predicate_and_variable_step() {
    let state = jit_scan(FORP, "forp");
    let r = read_lints(&state, 0, 7);
    assert_eq!(r[0], 101, "FOR i:BYTE := 100 TO 200");
    assert_eq!(r[1], 101, "FOR k:USINT := 100 TO 200");
    assert_eq!(r[2], 5, "FOR j := 5 TO 1 BY down, down:INT := -1");
    assert_eq!(r[3], 5, "FOR j := 1 TO 10 BY up2, up2:INT := 2");
    assert_eq!(r[4], 3, "FOR i:BYTE := 200 TO 100 BY down50");
    assert_eq!(r[5], 3, "FOR j := 1 TO 3");
    assert_eq!(r[6], 5, "FOR j := 5 TO 1 BY -1");
}

// ---------------------------------------------------------------------------
// MIN / MAX / LIMIT / ABS — the same signed-predicate mistake, in the builtins
// ---------------------------------------------------------------------------

const SEL: &str = r#"
PROGRAM SELP
VAR
    r : ARRAY[0..11] OF LINT;
    ab : BYTE  := 200;  bb : BYTE  := 100;
    aw : WORD  := 65000; bw : WORD := 1;
    ad : DWORD := 4000000000; bd : DWORD := 1;
    au : USINT := 200;  bu : USINT := 100;
    ai : UINT  := 65000; bi : UINT := 1;
    ax : UDINT := 4000000000; bx : UDINT := 1;
    sa : SINT := -5;    sb : SINT := 3;
END_VAR
    r[0] := MAX(ab, bb);
    r[1] := MIN(ab, bb);
    r[2] := MAX(aw, bw);
    r[3] := MAX(ad, bd);
    r[4] := MAX(au, bu);
    r[5] := MAX(ai, bi);
    r[6] := MAX(ax, bx);
    r[7] := LIMIT(bb, ab, ab);
    r[8] := ABS(ab);
    (* signed keeps signed *)
    r[9] := MAX(sa, sb);
    r[10] := MIN(sa, sb);
    r[11] := ABS(sa);
END_PROGRAM
"#;

#[test]
fn min_max_limit_abs_respect_operand_signedness() {
    let state = jit_scan(SEL, "selp");
    let r = read_lints(&state, 0, 12);
    assert_eq!(r[0], 200, "MAX(BYTE 200, 100)");
    assert_eq!(r[1], 100, "MIN(BYTE 200, 100)");
    assert_eq!(r[2], 65000, "MAX(WORD 65000, 1)");
    assert_eq!(r[3], 4000000000, "MAX(DWORD 4000000000, 1)");
    assert_eq!(r[4], 200, "MAX(USINT 200, 100)");
    assert_eq!(r[5], 65000, "MAX(UINT 65000, 1)");
    assert_eq!(r[6], 4000000000, "MAX(UDINT 4000000000, 1)");
    assert_eq!(r[7], 200, "LIMIT(BYTE 100, 200, 200)");
    assert_eq!(r[8], 200, "ABS(BYTE 200) is 200, not 56");
    assert_eq!(r[9], 3, "MAX(SINT -5, 3)");
    assert_eq!(r[10], -5, "MIN(SINT -5, 3)");
    assert_eq!(r[11], 5, "ABS(SINT -5)");
}
