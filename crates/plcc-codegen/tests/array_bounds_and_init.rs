// SPDX-License-Identifier: MPL-2.0

//! Arrays: negative lower bounds, FB instances held in array elements, and aggregate
//! initializers.
//!
//! Three separate defects, all proved by execution:
//!
//! * `ARRAY[-2..2] OF DINT` allocated `[3 x i32]` (should be 5) and indexed with the
//!   raw subscript, so `a[-2] := ...` stored *outside* the object. A JIT probe died
//!   with `free(): invalid next size (fast)`. The bound expression `-2` is a
//!   `UnaryOp`, and `resolve_type_spec` only accepted a bare `IntegerLiteral`,
//!   silently substituting `0` for anything else — which also zeroed the offset used
//!   to normalise the index.
//! * An FB instance held in an array element was never called: `a[1](s := 4);`
//!   emitted nothing at all.
//! * `ARRAY[0..2] OF DINT := [10, 20, 30]` did not compile.
//!
//! Every test writes decoys into neighbouring elements so lowering to the *wrong*
//! address fails as loudly as not lowering at all.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;

/// Compile, JIT, run `p_init` then one `p_scan`, and return the leading DINT — every
/// program below declares its result `n` first.
fn run_n(source: &str) -> i32 {
    run_n_scans(source, 1)
}

/// Same, but run `p_scan` `scans` times.
fn run_n_scans(source: &str, scans: usize) -> i32 {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let ctx = Context::create();
    let mut compiler = Compiler::new(&ctx, "arrbounds");
    compiler.compile(&unit).expect("codegen failed");

    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("failed to create JIT");
    // Deliberately oversized and guarded: a store outside the object still lands in
    // this buffer rather than in the allocator's metadata, so the assertion reports
    // the bug instead of the process dying.
    let mut state = vec![0u8; 65536];
    let ptr = unsafe { state.as_mut_ptr().add(32768) };
    if let Ok(addr) = ee.get_function_address("p_init") {
        let f: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(addr) };
        f(ptr);
    }
    let addr = ee.get_function_address("p_scan").expect("p_scan");
    let f: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(addr) };
    for _ in 0..scans {
        f(ptr);
    }
    i32::from_ne_bytes([state[32768], state[32769], state[32770], state[32771]])
}

// ---------------------------------------------------------------------------
// A1 — negative lower bounds
// ---------------------------------------------------------------------------

/// The whole range of `ARRAY[-2..2]` is written and read back.
///
/// Reading back is not on its own enough: an un-normalised index reads from the same
/// wrong address it wrote to, so the round trip "succeeds" while scribbling over the
/// neighbours. The guards on either side of `a` are what catches that — on the
/// unfixed compiler `a[-1]` lands on `lo_guard`.
#[test]
fn negative_lower_bound_whole_range_round_trips() {
    let n = run_n(
        r#"
PROGRAM P
VAR
    n : DINT;
    lo_guard : DINT;
    a : ARRAY[-2..2] OF DINT;
    hi_guard : DINT;
    i : DINT;
    sum : DINT;
END_VAR
    lo_guard := 111111;
    hi_guard := 222222;
    a[-2] := 1;
    a[-1] := 20;
    a[0]  := 300;
    a[1]  := 4000;
    a[2]  := 50000;
    sum := 0;
    FOR i := -2 TO 2 DO
        sum := sum + a[i];
    END_FOR;
    IF lo_guard <> 111111 OR hi_guard <> 222222 THEN
        sum := -1;
    END_IF;
    n := sum;
END_PROGRAM
"#,
    );
    assert_eq!(
        n, 54321,
        "every element of ARRAY[-2..2] read back, neighbours untouched"
    );
}

/// Each slot is distinct: writing one must not disturb its neighbours.
#[test]
fn negative_lower_bound_elements_are_distinct() {
    let n = run_n(
        r#"
PROGRAM P
VAR
    n : DINT;
    lo_guard : DINT;
    a : ARRAY[-3..1] OF DINT;
    hi_guard : DINT;
    r : DINT;
END_VAR
    lo_guard := 111111;
    hi_guard := 222222;
    a[-3] := 11;
    a[-2] := 22;
    a[-1] := 33;
    a[0]  := 44;
    a[1]  := 55;
    r := a[-3] * 100000 + a[-1];
    IF lo_guard <> 111111 OR hi_guard <> 222222 THEN
        r := -1;
    END_IF;
    n := r;
END_PROGRAM
"#,
    );
    assert_eq!(n, 1_100_033, "a[-3] and a[-1] kept their own values");
}

/// The element count is `hi - lo + 1`, so a wholly negative range still has room.
#[test]
fn wholly_negative_range() {
    let n = run_n(
        r#"
PROGRAM P
VAR
    n : DINT;
    lo_guard : DINT;
    a : ARRAY[-5..-1] OF DINT;
    hi_guard : DINT;
    i : DINT;
    sum : DINT;
END_VAR
    lo_guard := 111111;
    hi_guard := 222222;
    FOR i := -5 TO -1 DO
        a[i] := i * i;
    END_FOR;
    sum := 0;
    FOR i := -5 TO -1 DO
        sum := sum + a[i];
    END_FOR;
    IF lo_guard <> 111111 OR hi_guard <> 222222 THEN
        sum := -1;
    END_IF;
    n := sum;
END_PROGRAM
"#,
    );
    assert_eq!(n, 55, "25+16+9+4+1 over ARRAY[-5..-1]");
}

/// Multi-dimensional, with only *some* dimensions negative — the row stride depends
/// on the second dimension's true extent, so a wrong element count aliases rows.
#[test]
fn multi_dim_mixed_sign_bounds() {
    let n = run_n(
        r#"
PROGRAM P
VAR
    n : DINT;
    m : ARRAY[0..2, -1..1] OF DINT;
    i : DINT;
    j : DINT;
    sum : DINT;
END_VAR
    FOR i := 0 TO 2 DO
        FOR j := -1 TO 1 DO
            m[i, j] := (i + 1) * 10 + (j + 1);
        END_FOR;
    END_FOR;
    sum := 0;
    FOR i := 0 TO 2 DO
        FOR j := -1 TO 1 DO
            sum := sum + m[i, j];
        END_FOR;
    END_FOR;
    n := sum * 1000 + m[0, -1] * 100 + m[2, 1];
END_PROGRAM
"#,
    );
    // rows: 10,11,12 / 20,21,22 / 30,31,32 -> sum 189; m[0,-1]=10, m[2,1]=32
    assert_eq!(n, 189_000 + 1000 + 32);
}

/// A nested array with a negative inner bound.
#[test]
fn nested_array_negative_inner_bound() {
    let n = run_n(
        r#"
PROGRAM P
VAR
    n : DINT;
    lo_guard : DINT;
    o : ARRAY[1..2] OF ARRAY[-1..1] OF DINT;
    hi_guard : DINT;
    r : DINT;
END_VAR
    lo_guard := 111111;
    hi_guard := 222222;
    o[1][-1] := 7;
    o[1][0]  := 70;
    o[1][1]  := 700;
    o[2][-1] := 1;
    o[2][0]  := 10;
    o[2][1]  := 100;
    r := o[1][-1] * 10000 + o[2][1];
    IF lo_guard <> 111111 OR hi_guard <> 222222 THEN
        r := -1;
    END_IF;
    n := r;
END_PROGRAM
"#,
    );
    assert_eq!(n, 70_100, "o[1][-1]=7 and o[2][1]=100, no aliasing");
}

/// An array-typed FUNCTION_BLOCK input with a negative lower bound.
#[test]
fn fb_input_array_negative_lower_bound() {
    let n = run_n(
        r#"
FUNCTION_BLOCK SUMMER
VAR_INPUT
    v : ARRAY[-2..0] OF DINT;
END_VAR
VAR_OUTPUT
    o : DINT;
END_VAR
    o := v[-2] + v[-1] + v[0];
END_FUNCTION_BLOCK

PROGRAM P
VAR
    n : DINT;
    f : SUMMER;
    a : ARRAY[-2..0] OF DINT;
END_VAR
    a[-2] := 100;
    a[-1] := 20;
    a[0] := 3;
    f(v := a);
    n := f.o;
END_PROGRAM
"#,
    );
    assert_eq!(n, 123, "the FB saw all three elements of ARRAY[-2..0]");
}

// ---------------------------------------------------------------------------
// A2 — an FB instance held in an array element
// ---------------------------------------------------------------------------

#[test]
fn fb_instance_in_array_element_is_called() {
    let n = run_n(
        r#"
FUNCTION_BLOCK ACC
VAR_INPUT
    s : DINT;
END_VAR
VAR_OUTPUT
    o : DINT;
END_VAR
    o := o + s;
END_FUNCTION_BLOCK

PROGRAM P
VAR
    n : DINT;
    a : ARRAY[0..2] OF ACC;
END_VAR
    a[0](s := 1000);
    a[1](s := 4);
    a[1](s := 8);
    a[2](s := 2000);
    n := a[1].o;
END_PROGRAM
"#,
    );
    assert_eq!(n, 12, "a[1] accumulated 4 then 8, independent of a[0]/a[2]");
}

/// Each element keeps its own state across scans.
#[test]
fn fb_instances_in_array_keep_separate_state() {
    let n = run_n_scans(
        r#"
FUNCTION_BLOCK ACC
VAR_INPUT
    s : DINT;
END_VAR
VAR_OUTPUT
    o : DINT;
END_VAR
    o := o + s;
END_FUNCTION_BLOCK

PROGRAM P
VAR
    n : DINT;
    a : ARRAY[1..3] OF ACC;
END_VAR
    a[1](s := 1);
    a[2](s := 10);
    a[3](s := 100);
    n := a[1].o * 10000 + a[2].o * 100 + a[3].o;
END_PROGRAM
"#,
        3,
    );
    assert_eq!(
        n,
        3 * 10000 + 30 * 100 + 300,
        "three scans, three instances"
    );
}

/// A negative lower bound on the array of FB instances.
#[test]
fn fb_instance_array_negative_lower_bound() {
    let n = run_n(
        r#"
FUNCTION_BLOCK ACC
VAR_INPUT
    s : DINT;
END_VAR
VAR_OUTPUT
    o : DINT;
END_VAR
    o := o + s;
END_FUNCTION_BLOCK

PROGRAM P
VAR
    n : DINT;
    a : ARRAY[-1..1] OF ACC;
END_VAR
    a[-1](s := 5);
    a[-1](s := 6);
    a[0](s := 900);
    a[1](s := 900);
    n := a[-1].o;
END_PROGRAM
"#,
    );
    assert_eq!(n, 11, "a[-1] accumulated 5 then 6");
}

// ---------------------------------------------------------------------------
// A3 — aggregate initializers
// ---------------------------------------------------------------------------

#[test]
fn array_aggregate_initializer() {
    let n = run_n(
        r#"
PROGRAM P
VAR
    n : DINT;
    a : ARRAY[0..2] OF DINT := [10, 20, 30];
END_VAR
    n := a[0] * 10000 + a[1] * 100 + a[2];
END_PROGRAM
"#,
    );
    assert_eq!(n, 102_030);
}

#[test]
fn array_aggregate_initializer_negative_lower_bound() {
    let n = run_n(
        r#"
PROGRAM P
VAR
    n : DINT;
    a : ARRAY[-1..1] OF DINT := [7, 8, 9];
END_VAR
    n := a[-1] * 10000 + a[0] * 100 + a[1];
END_PROGRAM
"#,
    );
    assert_eq!(n, 70_809, "the first initializer lands on index -1");
}

#[test]
fn array_aggregate_initializer_repetition() {
    // IEC 61131-3 repetition syntax: `3(0)` means three copies of 0.
    let n = run_n(
        r#"
PROGRAM P
VAR
    n : DINT;
    a : ARRAY[1..5] OF DINT := [2(7), 3(4)];
END_VAR
    n := a[1] * 10000 + a[3] * 100 + a[5];
END_PROGRAM
"#,
    );
    assert_eq!(n, 70_404, "a[1]=7, a[3]=4, a[5]=4");
}

#[test]
fn array_aggregate_initializer_multi_dim() {
    let n = run_n(
        r#"
PROGRAM P
VAR
    n : DINT;
    m : ARRAY[0..1, 0..2] OF DINT := [1, 2, 3, 4, 5, 6];
END_VAR
    n := m[0, 0] * 100000 + m[0, 2] * 1000 + m[1, 0] * 10 + m[1, 2];
END_PROGRAM
"#,
    );
    assert_eq!(n, 100_000 + 3000 + 40 + 6);
}

/// Fewer initializers than elements: the rest stay zero.
#[test]
fn array_aggregate_initializer_partial() {
    let n = run_n(
        r#"
PROGRAM P
VAR
    n : DINT;
    a : ARRAY[0..3] OF DINT := [5, 6];
END_VAR
    n := a[0] * 1000 + a[1] * 100 + a[2] * 10 + a[3];
END_PROGRAM
"#,
    );
    assert_eq!(n, 5600);
}

/// A VAR_GLOBAL array's aggregate becomes the global's constant contents — there is
/// no `_init` function to store it from.
#[test]
fn global_array_aggregate_initializer() {
    let n = run_n(
        r#"
VAR_GLOBAL
    g : ARRAY[0..3] OF DINT := [2(4), 5, 6];
END_VAR

PROGRAM P
VAR
    n : DINT;
END_VAR
    n := g[0] * 1000 + g[1] * 100 + g[2] * 10 + g[3];
END_PROGRAM
"#,
    );
    assert_eq!(n, 4456);
}

// ---------------------------------------------------------------------------
// Same dispatch family as A2: a METHOD invoked on an instance in an array element.
// The callee is a `MemberAccess` whose object is an `ArrayIndex`, not a named
// instance.
// ---------------------------------------------------------------------------

#[test]
fn method_call_on_array_element_instance() {
    let n = run_n(
        r#"
FUNCTION_BLOCK ACC
VAR_OUTPUT
    o : DINT;
END_VAR
METHOD Bump : DINT
VAR_INPUT
    amount : DINT;
END_VAR
    o := o + amount;
    Bump := o;
END_METHOD
END_FUNCTION_BLOCK

PROGRAM P
VAR
    n : DINT;
    a : ARRAY[0..2] OF ACC;
END_VAR
    a[0].Bump(900);
    a[1].Bump(5);
    a[1].Bump(6);
    a[2].Bump(900);
    n := a[1].o;
END_PROGRAM
"#,
    );
    assert_eq!(n, 11, "a[1].Bump ran twice on a[1] alone");
}
