// SPDX-License-Identifier: MPL-2.0

//! `POINTER TO`, `ADR()` and dereference (`^`) as OSCAT uses them: `pt^[i]` on a
//! pointer to an array, `pt^` as a value, `pt^ := x` as an assignment target,
//! `ADR(x)` of a variable/element/STRING, and CODESYS byte-addressed pointer
//! arithmetic (`pt := pt + 1`). None of these lowered before; 50 OSCAT files
//! failed on them.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;

fn run(source: &str, scan_fn: &str, size: usize, init: impl FnOnce(&mut [u8])) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let context = Context::create();
    let mut compiler = Compiler::new(&context, "pointer_dereference");
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
    if let Ok(init_fn) = ee.get_function_address(&scan_fn.replace("_scan", "_init")) {
        let init_fn: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(init_fn) };
        init_fn(state.as_mut_ptr());
    }
    init(&mut state);
    let addr = ee.get_function_address(scan_fn).expect("scan fn");
    let scan: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(addr) };
    scan(state.as_mut_ptr());
    state
}

fn expect_error(source: &str) -> String {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let context = Context::create();
    let mut compiler = Compiler::new(&context, "pointer_dereference");
    match compiler.compile(&unit) {
        Ok(()) => panic!("expected a codegen error, but compilation succeeded"),
        Err(e) => e.to_string(),
    }
}

fn i32_at(s: &[u8], o: usize) -> i32 {
    i32::from_ne_bytes(s[o..o + 4].try_into().unwrap())
}
fn u32_at(s: &[u8], o: usize) -> u32 {
    u32::from_ne_bytes(s[o..o + 4].try_into().unwrap())
}

const SRC: &str = r#"
PROGRAM P
VAR
    buf : ARRAY[0..7] OF BYTE;
    pb : POINTER TO BYTE;
    pa : POINTER TO ARRAY[0..7] OF BYTE;
    pd : POINTER TO DWORD;
    i : INT;
    sum : DINT;
    first : BYTE;
    third : BYTE;
    d : DWORD;
    n : UDINT;
    x : DWORD;
    addr_ok : BOOL;
END_VAR
    pb := ADR(buf);
    first := pb^;
    pb := pb + 2;
    third := pb^;
    pb^ := 99;
    pa := ADR(buf);
    FOR i := 0 TO 7 DO
        sum := sum + pa^[i];
    END_FOR;
    pa^[7] := pa^[0] + 1;
    pd := ADR(x);
    pd^ := 16#DEADBEEF;
    d := pd^ + 1;
    n := SIZEOF(buf);
    addr_ok := ADR(buf[3]) = ADR(buf) + 3;
END_PROGRAM
"#;

#[test]
fn adr_deref_index_and_store_through_pointers() {
    // buf@0..8 pb@8 pa@16 pd@24 i@32 sum@36 first@40 third@41 d@44 n@48 x@52
    // addr_ok@56
    let s = run(SRC, "p_scan", 64, |s| {
        s[0..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    });
    assert_eq!(s[40], 1, "first := pb^ after pb := ADR(buf)");
    assert_eq!(s[41], 3, "pb := pb + 2 steps two bytes");
    assert_eq!(s[2], 99, "pb^ := 99 writes buf[2]");
    assert_eq!(i32_at(&s, 36), 1 + 2 + 99 + 4 + 5 + 6 + 7 + 8, "sum of pa^[i]");
    assert_eq!(s[7], 2, "pa^[7] := pa^[0] + 1");
    assert_eq!(u32_at(&s, 52), 0xDEAD_BEEF, "pd^ := ... writes x");
    assert_eq!(u32_at(&s, 44), 0xDEAD_BEF0, "d := pd^ + 1");
    assert_eq!(u32_at(&s, 48), 8, "SIZEOF(buf)");
    assert_eq!(s[56], 1, "ADR(buf[3]) = ADR(buf) + 3");
}

/// The OSCAT string-scanning idiom: walk a STRING byte by byte through a
/// `POINTER TO BYTE` (BIN_TO_BYTE, HEX_TO_DWORD, ...), and reinterpret a REAL's
/// bits through a `POINTER TO DWORD` (REAL_TO_DW).
const OSCAT_SHAPED: &str = r#"
FUNCTION BITS_OF : BYTE
VAR_INPUT
    str : STRING(12);
END_VAR
VAR
    pt : POINTER TO BYTE;
    k : INT;
    c : BYTE;
END_VAR
    pt := ADR(str);
    FOR k := 1 TO LEN(str) DO
        c := pt^;
        IF c = 49 THEN
            BITS_OF := SHL(BITS_OF, 1) OR 1;
        ELSIF c = 48 THEN
            BITS_OF := SHL(BITS_OF, 1);
        END_IF;
        pt := pt + 1;
    END_FOR;
END_FUNCTION

PROGRAM Q
VAR
    s : STRING(12);
    r : REAL := 1.0;
    v : BYTE;
    bits : DWORD;
    pr : POINTER TO DWORD;
END_VAR
    v := BITS_OF(s);
    pr := ADR(r);
    bits := pr^;
END_PROGRAM
"#;

#[test]
fn oscat_shaped_string_walk_and_bit_reinterpretation() {
    // s@0..13 r@16 v@20 bits@24 pr@32
    let s = run(OSCAT_SHAPED, "q_scan", 40, |s| {
        s[0..6].copy_from_slice(b"10110\0");
    });
    assert_eq!(s[20], 0b10110, "BITS_OF('10110')");
    assert_eq!(u32_at(&s, 24), 1.0f32.to_bits(), "REAL bits through POINTER TO DWORD");
}

#[test]
fn dereferencing_a_non_pointer_is_reported() {
    let msg = expect_error(
        "PROGRAM P\nVAR\n    r : REAL;\n    b : BYTE;\nEND_VAR\n    b := r^;\nEND_PROGRAM\n",
    );
    assert!(msg.contains("r^"), "names the dereference; got: {msg}");
    assert!(msg.contains("POINTER"), "says why; got: {msg}");
}

#[test]
fn adr_of_a_non_location_is_reported() {
    let msg = expect_error(
        "PROGRAM P\nVAR\n    p : POINTER TO INT;\nEND_VAR\n    p := ADR(1 + 2);\nEND_PROGRAM\n",
    );
    assert!(msg.contains("ADR"), "got: {msg}");
    assert!(msg.contains("no address"), "got: {msg}");
}

/// FUNCTION locals and the result start at 0 on every call (IEC: a FUNCTION has no
/// memory between calls). They were bare allocas, so the OSCAT idiom
/// `F := SHL(F, 1) OR 1` accumulated whatever the stack held.
#[test]
fn function_locals_and_result_start_at_zero_each_call() {
    let src = r#"
FUNCTION BUMP : INT
VAR_INPUT
    k : INT;
END_VAR
VAR
    n : INT;
END_VAR
    n := n + k;
    BUMP := BUMP + n;
END_FUNCTION

PROGRAM Z
VAR
    a : INT;
    b : INT;
END_VAR
    a := BUMP(5);
    b := BUMP(7);
END_PROGRAM
"#;
    let s = run(src, "z_scan", 4, |_| {});
    assert_eq!(i16::from_ne_bytes([s[0], s[1]]), 5, "first call");
    assert_eq!(i16::from_ne_bytes([s[2], s[3]]), 7, "second call does not see the first");
}
