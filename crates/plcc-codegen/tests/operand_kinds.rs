// SPDX-License-Identifier: MPL-2.0

//! Operators and builtins handed an operand of a kind they did not expect — a
//! POINTER, or a REAL where an integer belongs — must lower it or report it. They
//! used to call `into_int_value()` on it and panic the whole compiler (13 OSCAT
//! files: `pt := pt + 1` in BIN_TO_BYTE/HEX_TO_DWORD/_BUFFER_INIT, a REAL reaching
//! `INT_TO_REAL` in CMP, `INT_TO_BYTE(TRUNC(..))` in RANGE_TO_BYTE).

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

/// Compile and return the error message; the compile must fail, not panic.
fn expect_error(source: &str) -> String {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let context = Context::create();
    let mut compiler = Compiler::new(&context, "operand_kinds");
    match compiler.compile(&unit) {
        Ok(()) => panic!("expected a codegen error, but compilation succeeded"),
        Err(e) => e.to_string(),
    }
}

fn u64_at(s: &[u8], o: usize) -> u64 {
    u64::from_ne_bytes(s[o..o + 8].try_into().unwrap())
}
fn i32_at(s: &[u8], o: usize) -> i32 {
    i32::from_ne_bytes(s[o..o + 4].try_into().unwrap())
}
fn f32_at(s: &[u8], o: usize) -> f32 {
    f32::from_ne_bytes(s[o..o + 4].try_into().unwrap())
}

const PTR_SRC: &str = r#"
PROGRAM P
VAR
    p1 : POINTER TO BYTE;
    p2 : POINTER TO BYTE;
    p3 : POINTER TO DWORD;
    dist : DINT;
    lt : BOOL;
    eq : BOOL;
    gt : BOOL;
    back : DINT;
END_VAR
    p2 := p1 + 3;
    p3 := 2 + p1;
    dist := p2 - p1;
    lt := p1 < p2;
    eq := p3 = p1 + 2;
    gt := p1 > p2;
    back := (p2 - 3) - p1;
END_PROGRAM
"#;

/// CODESYS pointer arithmetic, which OSCAT is written against: the offset counts
/// bytes whatever the pointee type, `ptr - ptr` is the byte distance, and pointers
/// compare as addresses.
#[test]
fn pointer_arithmetic_is_byte_addressed() {
    let base: u64 = 0x1000_0000;
    let s = run(PTR_SRC, "p_scan", 48, |s| {
        s[0..8].copy_from_slice(&base.to_ne_bytes());
    });
    assert_eq!(u64_at(&s, 8), base + 3, "p1 + 3");
    assert_eq!(u64_at(&s, 16), base + 2, "2 + p1, into a POINTER TO DWORD");
    assert_eq!(i32_at(&s, 24), 3, "p2 - p1");
    assert_eq!(s[28], 1, "p1 < p2");
    assert_eq!(s[29], 1, "p3 = p1 + 2");
    assert_eq!(s[30], 0, "p1 > p2");
    assert_eq!(i32_at(&s, 32), 0, "(p2 - 3) - p1");
}

const TRUNC_SRC: &str = r#"
PROGRAM Q
VAR
    x : REAL;
    d : DINT;
    b : BYTE;
    r : REAL;
    n : INT;
END_VAR
    d := TRUNC(x);
    b := INT_TO_BYTE(TRUNC(x * 10.0));
    r := INT_TO_REAL(FLOOR(x) - n + 1);
END_PROGRAM
"#;

/// IEC TRUNC is ANY_REAL -> ANY_INT (DINT), toward zero. It returned a REAL, so
/// the integer conversion around it (OSCAT's RANGE_TO_BYTE) panicked. And a REAL
/// that reaches an `X_TO_REAL` conversion (CMP: `INT_TO_REAL(FLOOR(..) - N + 1)`,
/// FLOOR being REAL-valued here) is resized instead of crashing the compiler.
#[test]
fn trunc_is_integer_and_int_to_real_accepts_a_real() {
    let s = run(TRUNC_SRC, "q_scan", 20, |s| {
        s[0..4].copy_from_slice(&(-3.75f32).to_ne_bytes());
        s[16..18].copy_from_slice(&2i16.to_ne_bytes());
    });
    assert_eq!(i32_at(&s, 4), -3, "TRUNC(-3.75)");
    assert_eq!(s[8], (-37i32) as u8, "INT_TO_BYTE(TRUNC(-37.5))");
    assert_eq!(f32_at(&s, 12), -4.0 - 2.0 + 1.0, "INT_TO_REAL(FLOOR(x) - n + 1)");

    let s = run(TRUNC_SRC, "q_scan", 20, |s| {
        s[0..4].copy_from_slice(&(3.99f32).to_ne_bytes());
    });
    assert_eq!(i32_at(&s, 4), 3, "TRUNC(3.99)");
    assert_eq!(s[8], 39, "INT_TO_BYTE(TRUNC(39.9))");
}

fn program(decls: &str, body: &str) -> String {
    format!("PROGRAM Main\nVAR\n{decls}\nEND_VAR\n{body}\nEND_PROGRAM\n")
}

/// Every one of these used to panic in `into_int_value()`. REAL is not implicitly
/// an integer in IEC 61131-3, so each is a diagnostic that says so. (A REAL passed
/// to a conversion such as `DINT_TO_INT(r)` is converted to DINT first, rounding,
/// as CODESYS does for any lossy implicit conversion.)
#[test]
fn real_where_an_integer_belongs_is_reported_not_a_panic() {
    let decls = "    r : REAL;\n    i : INT;\n    a : ARRAY[0..3] OF INT;\n    w : WORD;";
    for body in [
        "    IF r THEN i := 1; END_IF;",
        "    WHILE r DO i := 1; END_WHILE;",
        "    i := a[r];",
        "    a[r] := 1;",
        "    CASE r OF 1: i := 1; END_CASE;",
        "    w := NOT r;",
        "    w := SHL(r, 1);",
        "    i := SEL(r, 1, 2);",
    ] {
        let msg = expect_error(&program(decls, body));
        assert!(
            msg.contains("REAL"),
            "`{}` must say a REAL was given where an integer is needed; got: {msg}",
            body.trim()
        );
    }
}

/// A float operand combined with a pointer is meaningless and is rejected.
#[test]
fn pointer_with_real_is_reported() {
    let msg = expect_error(&program(
        "    p : POINTER TO BYTE;\n    r : REAL;\n    b : BOOL;",
        "    b := p < r;",
    ));
    assert!(msg.contains("POINTER"), "got: {msg}");
}

/// An assignment whose right-hand side yields no value is a diagnostic. The store
/// used to be skipped silently, so `pt := ADR(bin);` compiled to nothing when ADR
/// was unknown and left `pt` unset.
#[test]
fn assignment_of_no_value_is_reported() {
    let msg = expect_error(&program("    i : INT;", "    i := NOT_A_FUNCTION(1);"));
    assert!(msg.contains("NOT_A_FUNCTION"), "got: {msg}");
    assert!(msg.contains("right-hand side"), "got: {msg}");
}
