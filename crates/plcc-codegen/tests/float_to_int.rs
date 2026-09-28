// SPDX-License-Identifier: MPL-2.0

//! REAL/LREAL → integer conversions.
//!
//! CODESYS rounds to nearest with halves away from zero ("for 1 to 4 after the
//! decimal point, the number is rounded down. For 5 to 9, the number is rounded
//! up"; `REAL_TO_INT(-1.5)` is -2). `TRUNC` truncates. Every REAL_TO_<int>,
//! LREAL_TO_<int> and implicit REAL→integer store goes through one helper.
//!
//! Each test JIT-executes `p_init` then one `p_scan` and reads the PROGRAM state.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;

fn run_opt(source: &str, opt: OptimizationLevel) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let ctx = Context::create();
    let mut compiler = Compiler::new(&ctx, "f2i");
    compiler.compile(&unit).expect("codegen failed");
    if let Err(e) = compiler.module().verify() {
        panic!("invalid IR: {e}\n{}", compiler.emit_ir());
    }
    let ee = compiler
        .module()
        .create_jit_execution_engine(opt)
        .expect("failed to create JIT");
    let mut state = vec![0u8; 4096];
    let ptr = state.as_mut_ptr();
    if let Ok(addr) = ee.get_function_address("p_init") {
        let f: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(addr) };
        f(ptr);
    }
    let addr = ee.get_function_address("p_scan").expect("p_scan");
    let f: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(addr) };
    f(ptr);
    state
}

fn run(source: &str) -> Vec<u8> {
    run_opt(source, OptimizationLevel::None)
}

fn i64s(state: &[u8], n: usize) -> Vec<i64> {
    (0..n)
        .map(|i| i64::from_ne_bytes(state[i * 8..i * 8 + 8].try_into().unwrap()))
        .collect()
}

/// `r0..rN : LINT` results first, then REAL inputs; each result is widened from the
/// named conversion's own result type, so its signedness is visible.
fn conv(func: &str, inputs: &[&str], lreal: bool) -> Vec<i64> {
    let ty = if lreal { "LREAL" } else { "REAL" };
    let mut decl = String::new();
    let mut body = String::new();
    for i in 0..inputs.len() {
        decl.push_str(&format!("    r{i} : LINT;\n"));
    }
    for (i, v) in inputs.iter().enumerate() {
        decl.push_str(&format!("    x{i} : {ty} := {v};\n"));
        body.push_str(&format!("    r{i} := {func}(x{i});\n"));
    }
    let src = format!("PROGRAM p\nVAR\n{decl}END_VAR\n{body}END_PROGRAM\n");
    i64s(&run(&src), inputs.len())
}

const TIES: &[&str] = &["2.7", "2.5", "1.5", "0.5", "1.4", "-1.5", "-2.5", "-2.7", "-0.4"];
const TIES_ROUNDED: &[i64] = &[3, 3, 2, 1, 1, -2, -3, -3, 0];

#[test]
fn real_to_signed_rounds_half_away_from_zero() {
    for f in ["REAL_TO_SINT", "REAL_TO_INT", "REAL_TO_DINT", "REAL_TO_LINT"] {
        assert_eq!(conv(f, TIES, false), TIES_ROUNDED, "{f}");
    }
    for f in ["LREAL_TO_SINT", "LREAL_TO_INT", "LREAL_TO_DINT", "LREAL_TO_LINT"] {
        assert_eq!(conv(f, TIES, true), TIES_ROUNDED, "{f}");
    }
}

#[test]
fn real_to_unsigned_rounds_half_away_from_zero() {
    let pos = &["2.7", "2.5", "1.5", "0.5", "1.4", "0.49"];
    let want = [3, 3, 2, 1, 1, 0];
    for f in [
        "REAL_TO_USINT",
        "REAL_TO_UINT",
        "REAL_TO_UDINT",
        "REAL_TO_ULINT",
        "REAL_TO_BYTE",
        "REAL_TO_WORD",
        "REAL_TO_DWORD",
        "REAL_TO_LWORD",
    ] {
        assert_eq!(conv(f, pos, false), want, "{f}");
        let lf = format!("L{f}");
        assert_eq!(conv(&lf, pos, true), want, "{lf}");
    }
}

#[test]
fn trunc_still_truncates() {
    assert_eq!(
        conv("TRUNC", &["2.7", "-2.7", "2.5", "-0.5"], false),
        [2, -2, 2, 0]
    );
}

#[test]
fn implicit_real_to_integer_store_rounds() {
    let src = r#"
PROGRAM p
VAR
    a : DINT;
    b : DINT;
    c : INT;
    d : UDINT;
    x : REAL := 2.5;
    y : LREAL := -3.5;
END_VAR
    a := x;
    b := y;
    c := x * 3.0;
    d := x;
END_PROGRAM
"#;
    let s = run(src);
    let a = i32::from_ne_bytes(s[0..4].try_into().unwrap());
    let b = i32::from_ne_bytes(s[4..8].try_into().unwrap());
    let c = i16::from_ne_bytes(s[8..10].try_into().unwrap());
    let d = u32::from_ne_bytes(s[12..16].try_into().unwrap());
    assert_eq!((a, b, c, d), (3, -4, 8, 3));
}

#[test]
fn real_argument_to_integer_parameter_rounds() {
    let src = r#"
FUNCTION ID : DINT
VAR_INPUT v : DINT; END_VAR
    ID := v;
END_FUNCTION
PROGRAM p
VAR
    a : DINT;
    x : REAL := 6.5;
END_VAR
    a := ID(x);
END_PROGRAM
"#;
    let s = run(src);
    assert_eq!(i32::from_ne_bytes(s[0..4].try_into().unwrap()), 7);
}

// ---------------------------------------------------------------------------
// Out of range: saturate, NaN → 0
// ---------------------------------------------------------------------------
//
// CODESYS leaves an out-of-range result "undefined, target system-dependent" and
// allows an exception. plcc saturates (llvm.fptosi.sat / llvm.fptoui.sat), which is
// defined on every target; a plain fptosi is LLVM poison, i.e. undefined behaviour
// once the optimizer sees it.

#[test]
fn out_of_range_saturates_signed() {
    assert_eq!(
        conv("REAL_TO_INT", &["1.0E6", "-1.0E6", "32767.4", "-32768.4"], false),
        [32767, -32768, 32767, -32768]
    );
    assert_eq!(
        conv("REAL_TO_SINT", &["300.0", "-300.0"], false),
        [127, -128]
    );
    assert_eq!(
        conv("REAL_TO_DINT", &["1.0E20", "-1.0E20"], false),
        [i32::MAX as i64, i32::MIN as i64]
    );
    assert_eq!(
        conv("LREAL_TO_LINT", &["1.0E30", "-1.0E30"], true),
        [i64::MAX, i64::MIN]
    );
    assert_eq!(conv("TRUNC", &["1.0E20", "-1.0E20"], false), [i32::MAX as i64, i32::MIN as i64]);
}

#[test]
fn out_of_range_saturates_unsigned() {
    assert_eq!(
        conv("REAL_TO_UDINT", &["-5.0", "1.0E10"], false),
        [0, u32::MAX as i64]
    );
    assert_eq!(conv("REAL_TO_BYTE", &["-1.0", "256.0"], false), [0, 255]);
    assert_eq!(conv("REAL_TO_WORD", &["70000.0"], false), [65535]);
    // ULINT::MAX does not fit the LINT result slot; read it back as unsigned.
    let v = conv("LREAL_TO_ULINT", &["-1.0", "1.0E30"], true);
    assert_eq!((v[0], v[1] as u64), (0, u64::MAX));
}

#[test]
fn nan_and_infinity() {
    let src = r#"
PROGRAM p
VAR
    a : LINT;
    b : DINT;
    c : INT;
    d : UDINT;
    e : LINT;
    f : LINT;
    z : REAL;
    nan : REAL;
    inf : LREAL;
END_VAR
    nan := z / z;
    inf := 1.0 / REAL_TO_LREAL(z);
    a := REAL_TO_LINT(nan);
    b := REAL_TO_DINT(nan);
    c := nan;
    d := REAL_TO_UDINT(nan);
    e := LREAL_TO_LINT(inf);
    f := LREAL_TO_LINT(-inf);
END_PROGRAM
"#;
    let s = run(src);
    let a = i64::from_ne_bytes(s[0..8].try_into().unwrap());
    let b = i32::from_ne_bytes(s[8..12].try_into().unwrap());
    let c = i16::from_ne_bytes(s[12..14].try_into().unwrap());
    let d = u32::from_ne_bytes(s[16..20].try_into().unwrap());
    let e = i64::from_ne_bytes(s[24..32].try_into().unwrap());
    let f = i64::from_ne_bytes(s[32..40].try_into().unwrap());
    assert_eq!((a, b, c, d), (0, 0, 0, 0), "NaN converts to 0");
    assert_eq!((e, f), (i64::MAX, i64::MIN), "infinities saturate");
}

#[test]
fn implicit_store_saturates() {
    let src = r#"
PROGRAM p
VAR
    a : INT;
    b : INT;
    x : REAL := 1.0E6;
END_VAR
    a := x;
    b := -x;
END_PROGRAM
"#;
    let s = run(src);
    let a = i16::from_ne_bytes(s[0..2].try_into().unwrap());
    let b = i16::from_ne_bytes(s[2..4].try_into().unwrap());
    assert_eq!((a, b), (32767, -32768));
}

#[test]
fn saturation_holds_under_optimization() {
    let src = r#"
PROGRAM p
VAR
    a : LINT;
    b : LINT;
    x : REAL := 1.0E6;
END_VAR
    a := REAL_TO_INT(x);
    b := REAL_TO_INT(3.0E9);
END_PROGRAM
"#;
    let v = i64s(&run_opt(src, OptimizationLevel::Aggressive), 2);
    assert_eq!(v, [32767, 32767]);
}
