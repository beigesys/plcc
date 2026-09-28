// SPDX-License-Identifier: MPL-2.0

//! Structure initializers `x : Pt := (a := 1, b := 2);` (IEC 61131-3 §6.4.4.6,
//! CODESYS). They did not parse. Fields not named keep their declared default;
//! nesting, arrays of structures, VAR_GLOBAL and FB members all work.

mod common;
use common::{compile_error, run};

const TYPES: &str = r#"
TYPE Pt : STRUCT x : INT; y : INT := 7; z : REAL; END_STRUCT END_TYPE
TYPE Line : STRUCT a : Pt; b : Pt; name : STRING[8]; END_STRUCT END_TYPE
"#;

#[test]
fn structure_initializers_everywhere() {
    let src = format!(
        "{TYPES}
VAR_GLOBAL g : Pt := (x := 42); END_VAR
FUNCTION_BLOCK F
VAR m : Pt := (y := 8); END_VAR
VAR_OUTPUT my : INT; END_VAR
my := m.y;
END_FUNCTION_BLOCK
PROGRAM p
VAR
  p1 : Pt := (x := 1, z := 2.5);
  l : Line := (a := (x := 3), b := (x := 4, y := 5), name := 'ab');
  arr : ARRAY[0..1] OF Pt := [(x := 10), (y := 20)];
  f : F;
  x1 : INT; y1 : INT; z1 : REAL; ax : INT; ay : INT; bx : INT; byy : INT; nm : STRING;
  a0x : INT; a0y : INT; a1x : INT; a1y : INT; gx : INT; fy : INT;
END_VAR
x1 := p1.x; y1 := p1.y; z1 := p1.z;
ax := l.a.x; ay := l.a.y; bx := l.b.x; byy := l.b.y; nm := l.name;
a0x := arr[0].x; a0y := arr[0].y; a1x := arr[1].x; a1y := arr[1].y;
gx := g.x;
f();
fy := f.my;
END_PROGRAM"
    );
    let s = run(&src);
    assert_eq!((s.i64("x1"), s.i64("y1"), s.f64("z1")), (1, 7, 2.5));
    assert_eq!((s.i64("ax"), s.i64("ay")), (3, 7));
    assert_eq!((s.i64("bx"), s.i64("byy")), (4, 5));
    assert_eq!(s.str("nm"), "ab");
    assert_eq!((s.i64("a0x"), s.i64("a0y")), (10, 7));
    assert_eq!((s.i64("a1x"), s.i64("a1y")), (0, 20));
    assert_eq!(s.i64("gx"), 42);
    assert_eq!(s.i64("fy"), 8);
}

#[test]
fn unknown_field_in_a_structure_initializer_is_an_error() {
    let src = format!(
        "{TYPES}
PROGRAM p
VAR p1 : Pt := (w := 1); END_VAR
END_PROGRAM"
    );
    let e = compile_error(&src);
    assert!(e.contains("no field `w`"), "{e}");
}
