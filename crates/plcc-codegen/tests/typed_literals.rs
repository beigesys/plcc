// SPDX-License-Identifier: MPL-2.0

//! Typed literals (`DWORD#1`, `BYTE#255`, `INT#10`, `REAL#1`) anywhere a literal
//! can appear: operands, builtin and FUNCTION arguments, conditions, FOR bounds,
//! initializers. They produced no value at all outside a handful of constant
//! folds, which failed 12 OSCAT files (`BIT_TOGGLE_*`, `BIT_LOAD_*2`, `BAND_B`,
//! `INT_TO_BCDC`, ...). Each is built at its named type's width and signedness.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;

fn run(source: &str, scan_fn: &str, size: usize) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let context = Context::create();
    let mut compiler = Compiler::new(&context, "typed_literals");
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
    // Run the init function first when there is one, so declared initializers apply.
    if let Ok(init) = ee.get_function_address(&scan_fn.replace("_scan", "_init")) {
        let init: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(init) };
        init(state.as_mut_ptr());
    }
    let addr = ee.get_function_address(scan_fn).expect("scan fn");
    let scan: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(addr) };
    scan(state.as_mut_ptr());
    state
}

fn u32_at(s: &[u8], o: usize) -> u32 {
    u32::from_ne_bytes(s[o..o + 4].try_into().unwrap())
}
fn i64_at(s: &[u8], o: usize) -> i64 {
    i64::from_ne_bytes(s[o..o + 8].try_into().unwrap())
}
fn i16_at(s: &[u8], o: usize) -> i16 {
    i16::from_ne_bytes(s[o..o + 2].try_into().unwrap())
}
fn u16_at(s: &[u8], o: usize) -> u16 {
    u16::from_ne_bytes(s[o..o + 2].try_into().unwrap())
}
fn f32_at(s: &[u8], o: usize) -> f32 {
    f32::from_ne_bytes(s[o..o + 4].try_into().unwrap())
}
fn f64_at(s: &[u8], o: usize) -> f64 {
    f64::from_ne_bytes(s[o..o + 8].try_into().unwrap())
}

const SRC: &str = r#"
FUNCTION TWICE : DINT
VAR_INPUT
    v : DINT;
END_VAR
    TWICE := v * 2;
END_FUNCTION

PROGRAM T
VAR
    d : DWORD;
    l : LINT;
    b : BYTE;
    x : BOOL;
    i : INT;
    r : REAL;
    lr : LREAL;
    gt : BOOL;
    w : WORD;
    mx : BYTE;
    cnt : INT;
    k : INT;
    hit : BOOL;
    tw : DINT;
    init_w : WORD := WORD#65535;
    neg : LINT;
    sn : SINT;
    ni : DINT;
END_VAR
    d := DWORD#1 + 5;
    l := DWORD#4294967295;
    b := BYTE#255;
    x := BOOL#1;
    i := INT#10 * 3;
    r := REAL#1 / 4.0;
    lr := LREAL#2.5;
    gt := BYTE#200 > BYTE#100;
    w := SHL(WORD#1, 15);
    mx := MAX(BYTE#200, 100);
    FOR k := INT#1 TO INT#3 DO
        cnt := cnt + INT#1;
    END_FOR;
    IF DWORD#6 = d THEN
        hit := TRUE;
    END_IF;
    tw := TWICE(DINT#21);
    neg := BYTE#255 + SINT#127;
    sn := SINT#-1;
    ni := sn + INT#-300 + REAL_TO_DINT(REAL#+2.0);
END_PROGRAM
"#;

#[test]
fn typed_literals_in_every_value_position() {
    // Layout (natural alignment): d@0 l@8 b@16 x@17 i@18 r@20 lr@24 gt@32 w@34
    // mx@36 cnt@38 k@40 hit@42 tw@44 init_w@48 neg@56 sn@64 ni@68.
    let s = run(SRC, "t_scan", 72);
    assert_eq!(u32_at(&s, 0), 6, "DWORD#1 + 5");
    assert_eq!(
        i64_at(&s, 8),
        4_294_967_295,
        "DWORD#4294967295 widens unsigned, not to -1"
    );
    assert_eq!(s[16], 255, "BYTE#255");
    assert_eq!(s[17], 1, "BOOL#1");
    assert_eq!(i16_at(&s, 18), 30, "INT#10 * 3");
    assert_eq!(f32_at(&s, 20), 0.25, "REAL#1 / 4.0");
    assert_eq!(f64_at(&s, 24), 2.5, "LREAL#2.5");
    assert_eq!(s[32], 1, "BYTE#200 > BYTE#100 compares unsigned");
    assert_eq!(u16_at(&s, 34), 32768, "SHL(WORD#1, 15)");
    assert_eq!(s[36], 200, "MAX(BYTE#200, 100)");
    assert_eq!(i16_at(&s, 38), 3, "FOR k := INT#1 TO INT#3");
    assert_eq!(s[42], 1, "IF DWORD#6 = d");
    assert_eq!(u32_at(&s, 44), 42, "TWICE(DINT#21)");
    assert_eq!(u16_at(&s, 48), 65535, "init_w : WORD := WORD#65535");
    assert_eq!(i64_at(&s, 56), 382, "BYTE#255 + SINT#127 = 382 (mixed signedness promotes)");
    assert_eq!(s[64] as i8, -1, "SINT#-1 (signed typed literal)");
    assert_eq!(u32_at(&s, 68) as i32, -299, "sn + INT#-300 + REAL#+2.0");
}

const GLOBAL_SRC: &str = r#"
VAR_GLOBAL
    g : DWORD := DWORD#7;
    gb : BYTE := BYTE#16#F0;
END_VAR

PROGRAM G
VAR
    out : DWORD;
    ob : BYTE;
END_VAR
    out := g;
    ob := gb;
END_PROGRAM
"#;

#[test]
fn typed_literal_global_initializer() {
    let s = run(GLOBAL_SRC, "g_scan", 8);
    assert_eq!(u32_at(&s, 0), 7, "g : DWORD := DWORD#7");
    assert_eq!(s[4], 0xF0, "gb : BYTE := BYTE#16#F0");
}

#[test]
fn real_value_for_an_integer_type_is_reported() {
    let (unit, errors) = plcc_st::parse(
        "PROGRAM P\nVAR\n    i : INT;\nEND_VAR\n    i := INT#2.5 + 1;\nEND_PROGRAM\n",
    );
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let context = Context::create();
    let mut compiler = Compiler::new(&context, "typed_literals");
    let msg = compiler
        .compile(&unit)
        .expect_err("INT#2.5 must not compile")
        .to_string();
    assert!(msg.contains("INT#2.5"), "names the literal; got: {msg}");
}
