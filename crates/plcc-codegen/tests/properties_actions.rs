// SPDX-License-Identifier: MPL-2.0

//! CODESYS PROPERTY (GET/SET), ACTION, VAR_STAT, struct EXTENDS and
//! namespace/GVL-qualified names, compiled and executed.

mod common;
use common::{compile_error, run, run_with};

#[test]
fn property_get_and_set() {
    let src = r#"
FUNCTION_BLOCK Tank
VAR
    fLevel : REAL;
    reads : INT;
END_VAR
PROPERTY Level : REAL
GET
    reads := reads + 1;
    Level := fLevel;
END_GET
SET
    IF Level < 0.0 THEN
        fLevel := 0.0;
    ELSE
        fLevel := Level;
    END_IF
END_SET
END_PROPERTY
PROPERTY Half : REAL
GET
    Half := THIS^.Level / 2.0;
END_GET
END_PROPERTY
METHOD Fill : REAL
VAR_INPUT amount : REAL; END_VAR
    Level := Level + amount;
    Fill := Level;
END_METHOD
END_FUNCTION_BLOCK

PROGRAM p
VAR
    t : Tank;
    tanks : ARRAY[1..2] OF Tank;
    a, b, c, d, e : REAL;
    n : INT;
END_VAR
t.Level := 10.0;
a := t.Level;
t.Level := -5.0;
b := t.Level;
c := t.Fill(4.0);
tanks[2].Level := 8.0;
d := tanks[2].Half;
e := tanks[2].Level + t.Level;
n := t.reads;
END_PROGRAM
"#;
    let s = run(src);
    assert_eq!(s.f64("a"), 10.0);
    assert_eq!(s.f64("b"), 0.0, "SET clamps negative levels");
    assert_eq!(s.f64("c"), 4.0);
    assert_eq!(
        s.f64("d"),
        4.0,
        "GET calling another property through THIS^"
    );
    assert_eq!(s.f64("e"), 12.0);
    assert_eq!(
        s.i64("n"),
        5,
        "every read of Level ran GET (Fill reads it twice)"
    );
}

#[test]
fn inherited_and_interface_properties() {
    let src = r#"
INTERFACE IShape
PROPERTY Area : DINT
GET
END_GET
END_PROPERTY
END_INTERFACE

FUNCTION_BLOCK Base IMPLEMENTS IShape
VAR w, h : DINT; END_VAR
PROPERTY Area : DINT
GET
    Area := w * h;
END_GET
END_PROPERTY
PROPERTY Width : DINT
GET Width := w; END_GET
SET w := Width; END_SET
END_PROPERTY
END_FUNCTION_BLOCK

FUNCTION_BLOCK Square EXTENDS Base
METHOD Side
VAR_INPUT s : DINT; END_VAR
    Width := s;
    h := s;
END_METHOD
END_FUNCTION_BLOCK

PROGRAM p
VAR
    sq : Square;
    shape : IShape;
    area, via_itf, width : DINT;
END_VAR
sq.Side(6);
area := sq.Area;
shape := sq;
via_itf := shape.Area;
width := sq.Width;
END_PROGRAM
"#;
    let s = run(src);
    assert_eq!(s.i64("area"), 36);
    assert_eq!(s.i64("via_itf"), 36);
    assert_eq!(s.i64("width"), 6);
}

#[test]
fn actions_run_in_the_instance() {
    let src = r#"
FUNCTION_BLOCK Motor
VAR_OUTPUT running : BOOL; starts : INT; END_VAR
    ;
ACTION Start:
    running := TRUE;
    starts := starts + 1;
END_ACTION
ACTION Stop:
    running := FALSE;
END_ACTION
ACTION Toggle:
    IF running THEN Stop(); ELSE Start(); END_IF
END_ACTION
END_FUNCTION_BLOCK

PROGRAM p
VAR m1, m2 : Motor; r1, r2 : BOOL; n : INT; END_VAR
m1.Start();
m2.Start();
m2.Toggle();
m1.Toggle();
m1.Toggle();
r1 := m1.running;
r2 := m2.running;
n := m1.starts;
END_PROGRAM
"#;
    let s = run(src);
    assert!(s.bool("r1"));
    assert!(!s.bool("r2"));
    assert_eq!(s.i64("n"), 2);
}

#[test]
fn var_stat_is_shared_by_all_instances() {
    let src = r#"
FUNCTION_BLOCK Counted
VAR_STAT instances : INT; END_VAR
VAR id : INT; END_VAR
    IF id = 0 THEN
        instances := instances + 1;
        id := instances;
    END_IF
END_FUNCTION_BLOCK

PROGRAM p
VAR a, b, c : Counted; ia, ic : INT; END_VAR
a(); b(); c(); a();
ia := a.id;
ic := c.id;
END_PROGRAM
"#;
    let s = run_with(src, 2, 10, false);
    assert_eq!(s.i64("ia"), 1);
    assert_eq!(s.i64("ic"), 3);
}

#[test]
fn struct_extends_and_qualified_names() {
    let src = r#"
TYPE Base : STRUCT x : INT := 1; y : INT; END_STRUCT END_TYPE
TYPE Point3 EXTENDS Base : STRUCT z : INT := 3; END_STRUCT END_TYPE

PROGRAM p
VAR
    pt : Point3;
    t : Tc2_Standard.TON;
    sum : INT;
    q : BOOL;
END_VAR
pt.y := 2;
sum := pt.x + pt.y + pt.z;
t(IN := TRUE, PT := T#0MS);
q := t.Q;
END_PROGRAM
"#;
    let s = run(src);
    assert_eq!(s.i64("sum"), 6);
    assert!(s.bool("q"));
}

#[test]
fn reference_properties_read_and_write_through() {
    let src = r#"
FUNCTION_BLOCK Dialog
VAR bHide : BOOL; nCount : INT := 5; END_VAR
PROPERTY Hide : REFERENCE TO BOOL
GET
    Hide REF= bHide;
END_GET
END_PROPERTY
PROPERTY Count : REFERENCE TO INT
GET
    Count REF= nCount;
END_GET
END_PROPERTY
END_FUNCTION_BLOCK

PROGRAM p
VAR d : Dialog; h : BOOL; c : INT; END_VAR
d.Hide := TRUE;
d.Count := d.Count + 10;
h := d.bHide;
c := d.nCount;
END_PROGRAM
"#;
    let s = run(src);
    assert!(s.bool("h"));
    assert_eq!(s.i64("c"), 15);
}

#[test]
fn overloaded_to_string_uses_the_argument_type() {
    let src = r#"
PROGRAM p
VAR i : INT := -42; b : BOOL := TRUE; s1, s2 : STRING; END_VAR
s1 := TO_STRING(i);
s2 := TO_STRING(b);
END_PROGRAM
"#;
    let s = run(src);
    assert_eq!(s.str("s1"), "-42");
    assert_eq!(s.str("s2"), "TRUE");
}

#[test]
fn to_string_of_an_enumeration() {
    let src = r#"
{attribute 'qualified_only'}
{attribute 'to_string'}
TYPE E_State : (Idle, Running := 10, Done) INT; END_TYPE
TYPE E_Plain : (A := 3, B); END_TYPE

PROGRAM p
VAR st : E_State := E_State.Running; pl : E_Plain := E_Plain.B; s1, s2 : STRING; END_VAR
s1 := TO_STRING(st);
s2 := TO_STRING(pl);
END_PROGRAM
"#;
    let s = run(src);
    assert_eq!(s.str("s1"), "Running", "with the to_string attribute: the name");
    assert_eq!(s.str("s2"), "4", "without it: the value");
}

#[test]
fn reading_a_set_only_property_is_an_error() {
    let err = compile_error(
        r#"
FUNCTION_BLOCK F
VAR v : INT; END_VAR
PROPERTY P : INT
SET v := P; END_SET
END_PROPERTY
END_FUNCTION_BLOCK
PROGRAM p VAR f : F; x : INT; END_VAR x := f.P; END_PROGRAM
"#,
    );
    assert!(err.contains("has no GET"), "{err}");
}
