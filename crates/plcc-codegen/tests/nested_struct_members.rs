// SPDX-License-Identifier: MPL-2.0

//! Member chains more than one level deep.
//!
//! Both the lvalue and the rvalue member-access paths required the object to be a
//! bare identifier. `p.v := 9;` worked; `s.i.v := 7;` and `o := s.i.v;` matched
//! nothing and emitted nothing at all — no store, no load, no diagnostic. Nested
//! STRUCTs are ordinary IEC 61131-3 derived types, so this was silent wrong code for
//! any program that groups its data.
//!
//! Every assertion here is a value read back out of the state block after a scan.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;

fn jit_scan(source: &str, pou: &str) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let ctx = Context::create();
    let mut compiler = Compiler::new(&ctx, "members");
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

fn read_i16(state: &[u8], offset: usize) -> i16 {
    i16::from_ne_bytes([state[offset], state[offset + 1]])
}

#[test]
fn a_two_level_member_chain_stores_and_loads() {
    let source = r#"
TYPE
    Inner : STRUCT
        v : INT;
    END_STRUCT;
    Outer : STRUCT
        i : Inner;
        w : INT;
    END_STRUCT;
END_TYPE

PROGRAM P
VAR
    s : Outer;
    p : Inner;
    o : INT;
END_VAR
    p.v := 9;
    s.w := 8;
    s.i.v := 7;
    o := s.i.v;
END_PROGRAM
"#;
    // { { { i16 v } i, i16 w } s, { i16 v } p, i16 o }
    // s.i.v@0 s.w@2 p.v@4 o@6
    let state = jit_scan(source, "p");
    assert_eq!(read_i16(&state, 4), 9, "single-level p.v still works");
    assert_eq!(read_i16(&state, 2), 8, "single-level s.w still works");
    assert_eq!(read_i16(&state, 0), 7, "s.i.v := 7 must actually store");
    assert_eq!(read_i16(&state, 6), 7, "o := s.i.v must actually load");
}

#[test]
fn a_four_level_member_chain_works() {
    let source = r#"
TYPE
    L4 : STRUCT
        v : INT;
    END_STRUCT;
    L3 : STRUCT
        d : L4;
    END_STRUCT;
    L2 : STRUCT
        c : L3;
    END_STRUCT;
    L1 : STRUCT
        b : L2;
    END_STRUCT;
END_TYPE

PROGRAM Deep
VAR
    a : L1;
    n : INT;
END_VAR
    a.b.c.d.v := 11;
    n := a.b.c.d.v + 1;
END_PROGRAM
"#;
    // { { { { i16 v } d } c } b } a, i16 n } => a.b.c.d.v@0, n@2
    let state = jit_scan(source, "deep");
    assert_eq!(read_i16(&state, 0), 11, "four levels deep must store");
    assert_eq!(read_i16(&state, 2), 12, "and read back in an expression");
}

#[test]
fn nested_members_work_inside_a_function_block() {
    let source = r#"
TYPE
    Pt : STRUCT
        x : INT;
        y : INT;
    END_STRUCT;
    Box : STRUCT
        lo : Pt;
        hi : Pt;
    END_STRUCT;
END_TYPE

FUNCTION_BLOCK Sizer
VAR
    b : Box;
END_VAR
VAR_INPUT
    w : INT;
END_VAR
VAR_OUTPUT
    area : INT;
END_VAR
    b.lo.x := 0;
    b.lo.y := 0;
    b.hi.x := w;
    b.hi.y := 3;
    area := (b.hi.x - b.lo.x) * (b.hi.y - b.lo.y);
END_FUNCTION_BLOCK

PROGRAM UseSizer
VAR
    s : Sizer;
    n : INT;
END_VAR
    s(w := 5);
    n := s.area;
END_PROGRAM
"#;
    // { { { i16 x, i16 y } lo, { i16 x, i16 y } hi } b, i16 w, i16 area } s, i16 n }
    // b.lo.x@0 b.lo.y@2 b.hi.x@4 b.hi.y@6 w@8 area@10 n@12
    let state = jit_scan(source, "usesizer");
    assert_eq!(read_i16(&state, 4), 5, "b.hi.x := w");
    assert_eq!(read_i16(&state, 10), 15, "5 * 3");
    assert_eq!(read_i16(&state, 12), 15, "the program read s.area");
}

#[test]
fn a_nested_member_binds_an_fb_input() {
    // The FB-input path compiles the argument as an expression, so it goes through
    // the same member walk.
    let source = r#"
TYPE
    Inner : STRUCT
        v : INT;
    END_STRUCT;
    Outer : STRUCT
        i : Inner;
    END_STRUCT;
END_TYPE

FUNCTION_BLOCK Echo
VAR_INPUT
    a : INT;
END_VAR
VAR_OUTPUT
    b : INT;
END_VAR
    b := a;
END_FUNCTION_BLOCK

PROGRAM Bind
VAR
    s : Outer;
    e : Echo;
    n : INT;
END_VAR
    s.i.v := 13;
    e(a := s.i.v);
    n := e.b;
END_PROGRAM
"#;
    // { { { i16 v } i } s, { i16 a, i16 b } e, i16 n } => s.i.v@0 e.a@2 e.b@4 n@6
    let state = jit_scan(source, "bind");
    assert_eq!(read_i16(&state, 2), 13, "the FB input got s.i.v");
    assert_eq!(read_i16(&state, 4), 13, "and echoed it");
    assert_eq!(read_i16(&state, 6), 13, "and the program read it back");
}
