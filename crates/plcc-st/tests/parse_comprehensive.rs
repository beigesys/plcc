// SPDX-License-Identifier: MPL-2.0

//! Comprehensive parser coverage tests for plcc IEC 61131-3 ST compiler.
//! Each test uses inline ST source and validates parse results.

use plcc_st::*;

// ── Helpers ──

fn parse_ok(source: &str) -> CompilationUnit {
    let (unit, errors) = plcc_st::parse(source);
    assert!(
        errors.is_empty(),
        "expected no parse errors, got {errors:#?}"
    );
    unit
}

fn parse_expect_errors(source: &str) -> (CompilationUnit, Vec<ParseError>) {
    let (unit, errors) = plcc_st::parse(source);
    assert!(
        !errors.is_empty(),
        "expected parse errors but got none"
    );
    (unit, errors)
}

// ── 1. Configuration / Resource / Task ──

#[test]
fn configuration_resource_task() {
    let src = r#"
CONFIGURATION MyConfig
  RESOURCE MyRes ON CPU
    TASK FastTask(INTERVAL := T#10ms, PRIORITY := 1);
    PROGRAM Main WITH FastTask : MainProg;
  END_RESOURCE
END_CONFIGURATION
"#;
    let unit = parse_ok(src);
    assert_eq!(unit.declarations.len(), 1);
    match &unit.declarations[0] {
        Declaration::Configuration(cfg) => {
            assert_eq!(cfg.name.name, "MyConfig");
            assert_eq!(cfg.resources.len(), 1);
            let res = &cfg.resources[0];
            assert_eq!(res.name.name, "MyRes");
            assert!(res.on.is_some());
            assert_eq!(res.on.as_ref().unwrap().name, "CPU");
            assert_eq!(res.tasks.len(), 1);
            assert_eq!(res.tasks[0].name.name, "FastTask");
            assert_eq!(res.tasks[0].properties.len(), 2);
            assert_eq!(res.program_configs.len(), 1);
            assert_eq!(res.program_configs[0].name.name, "Main");
            assert_eq!(res.program_configs[0].task.as_ref().unwrap().name, "FastTask");
            assert_eq!(res.program_configs[0].program_type.name, "MainProg");
        }
        _ => panic!("expected Configuration declaration"),
    }
}

// ── 2. Class with methods ──

#[test]
fn class_with_methods() {
    let src = r#"
INTERFACE IRunnable
  METHOD Run : BOOL
  END_METHOD
END_INTERFACE

CLASS Motor IMPLEMENTS IRunnable
VAR
  speed : INT := 0;
END_VAR
  METHOD Run : BOOL
  VAR_INPUT
    target : INT;
  END_VAR
    speed := target;
    Run := TRUE;
  END_METHOD
  METHOD Stop
    speed := 0;
  END_METHOD
END_CLASS
"#;
    let unit = parse_ok(src);
    assert_eq!(unit.declarations.len(), 2);

    // Interface
    match &unit.declarations[0] {
        Declaration::Interface(iface) => {
            assert_eq!(iface.name.name, "IRunnable");
            assert_eq!(iface.methods.len(), 1);
            assert_eq!(iface.methods[0].name.name, "Run");
            assert!(iface.methods[0].return_type.is_some());
        }
        _ => panic!("expected Interface declaration"),
    }

    // Class
    match &unit.declarations[1] {
        Declaration::Class(cls) => {
            assert_eq!(cls.name.name, "Motor");
            assert_eq!(cls.implements.len(), 1);
            assert_eq!(cls.implements[0].name, "IRunnable");
            assert_eq!(cls.var_blocks.len(), 1);
            assert_eq!(cls.methods.len(), 2);
            assert_eq!(cls.methods[0].name.name, "Run");
            assert!(cls.methods[0].return_type.is_some());
            assert_eq!(cls.methods[0].var_blocks.len(), 1);
            assert_eq!(cls.methods[0].body.len(), 2);
            assert_eq!(cls.methods[1].name.name, "Stop");
            assert!(cls.methods[1].return_type.is_none());
            assert_eq!(cls.methods[1].body.len(), 1);
        }
        _ => panic!("expected Class declaration"),
    }
}

// ── 3. All elementary types ──

#[test]
fn all_elementary_types() {
    let src = r#"
PROGRAM AllTypes
VAR
    v_bool : BOOL;
    v_byte : BYTE;
    v_word : WORD;
    v_dword : DWORD;
    v_lword : LWORD;
    v_sint : SINT;
    v_int : INT;
    v_dint : DINT;
    v_lint : LINT;
    v_usint : USINT;
    v_uint : UINT;
    v_udint : UDINT;
    v_ulint : ULINT;
    v_real : REAL;
    v_lreal : LREAL;
    v_char : CHAR;
    v_wchar : WCHAR;
    v_time : TIME;
    v_ltime : LTIME;
    v_date : DATE;
    v_tod : TOD;
    v_dt : DT;
END_VAR
END_PROGRAM
"#;
    let unit = parse_ok(src);
    assert_eq!(unit.declarations.len(), 1);
    match &unit.declarations[0] {
        Declaration::Program(p) => {
            assert_eq!(p.name.name, "AllTypes");
            assert_eq!(p.var_blocks.len(), 1);
            let decls = &p.var_blocks[0].declarations;
            assert_eq!(decls.len(), 22);
            // Spot-check a few type names
            assert_eq!(decls[0].name.name, "v_bool");
            assert_eq!(decls[13].name.name, "v_real");
            assert_eq!(decls[21].name.name, "v_dt");
        }
        _ => panic!("expected Program declaration"),
    }
}

// ── 4. Array multidimensional ──

#[test]
fn array_multidimensional() {
    let src = r#"
PROGRAM ArrayTest
VAR
    arr1d : ARRAY[0..9] OF INT;
    arr2d : ARRAY[0..2, 0..4] OF REAL;
    arr3d : ARRAY[1..3, 1..3, 1..3] OF BOOL;
END_VAR
    arr1d[0] := 42;
    arr2d[1, 2] := 3.14;
    arr3d[1, 2, 3] := TRUE;
END_PROGRAM
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::Program(p) => {
            assert_eq!(p.var_blocks[0].declarations.len(), 3);
            // Check array dimensions via TypeSpecKind
            let ts = &p.var_blocks[0].declarations[0].type_spec;
            match &ts.kind {
                TypeSpecKind::Array { ranges, .. } => assert_eq!(ranges.len(), 1),
                _ => panic!("expected Array type"),
            }
            let ts2 = &p.var_blocks[0].declarations[1].type_spec;
            match &ts2.kind {
                TypeSpecKind::Array { ranges, .. } => assert_eq!(ranges.len(), 2),
                _ => panic!("expected Array type"),
            }
            let ts3 = &p.var_blocks[0].declarations[2].type_spec;
            match &ts3.kind {
                TypeSpecKind::Array { ranges, .. } => assert_eq!(ranges.len(), 3),
                _ => panic!("expected Array type"),
            }
            // Body should have 3 assignment statements
            assert_eq!(p.body.len(), 3);
        }
        _ => panic!("expected Program declaration"),
    }
}

// ── 5. Struct nested ──

#[test]
fn struct_nested() {
    let src = r#"
TYPE Point :
    STRUCT
        x : REAL;
        y : REAL;
    END_STRUCT;
END_TYPE

PROGRAM StructTest
VAR
    p : Point;
    dist : REAL;
END_VAR
    p.x := 1.0;
    p.y := 2.0;
    dist := p.x + p.y;
END_PROGRAM
"#;
    let unit = parse_ok(src);
    assert_eq!(unit.declarations.len(), 2);
    match &unit.declarations[0] {
        Declaration::TypeDecl(td) => {
            assert_eq!(td.name.name, "Point");
            match &td.type_spec.kind {
                TypeSpecKind::Struct(fields) => {
                    assert_eq!(fields.len(), 2);
                    assert_eq!(fields[0].name.name, "x");
                    assert_eq!(fields[1].name.name, "y");
                }
                _ => panic!("expected Struct type"),
            }
        }
        _ => panic!("expected TypeDecl"),
    }
    match &unit.declarations[1] {
        Declaration::Program(p) => {
            assert_eq!(p.body.len(), 3);
            // First statement is p.x := 1.0 (member access assignment)
            match &p.body[0].kind {
                StatementKind::Assignment { target, .. } => {
                    match &target.kind {
                        ExpressionKind::MemberAccess { member, .. } => {
                            assert_eq!(member.name, "x");
                        }
                        _ => panic!("expected MemberAccess on assignment target"),
                    }
                }
                _ => panic!("expected Assignment"),
            }
        }
        _ => panic!("expected Program"),
    }
}

// ── 6. Enum declaration ──

#[test]
fn enum_declaration() {
    let src = r#"
TYPE Color : (Red, Green, Blue);
END_TYPE

TYPE Direction : INT (North := 0, South := 1, East := 2, West := 3);
END_TYPE

PROGRAM EnumTest
VAR
    c : Color;
    d : Direction;
END_VAR
    c := Red;
    d := North;
END_PROGRAM
"#;
    let unit = parse_ok(src);
    assert_eq!(unit.declarations.len(), 3);

    // First enum: untyped
    match &unit.declarations[0] {
        Declaration::TypeDecl(td) => {
            assert_eq!(td.name.name, "Color");
            match &td.type_spec.kind {
                TypeSpecKind::Enum(spec) => {
                    assert!(spec.base_type.is_none());
                    assert_eq!(spec.values.len(), 3);
                    assert_eq!(spec.values[0].name.name, "Red");
                    assert_eq!(spec.values[1].name.name, "Green");
                    assert_eq!(spec.values[2].name.name, "Blue");
                }
                _ => panic!("expected Enum type"),
            }
        }
        _ => panic!("expected TypeDecl"),
    }

    // Second enum: typed with explicit values
    match &unit.declarations[1] {
        Declaration::TypeDecl(td) => {
            assert_eq!(td.name.name, "Direction");
            match &td.type_spec.kind {
                TypeSpecKind::Enum(spec) => {
                    assert!(spec.base_type.is_some());
                    assert_eq!(spec.base_type.as_ref().unwrap().name, "INT");
                    assert_eq!(spec.values.len(), 4);
                    assert!(spec.values[0].value.is_some());
                }
                _ => panic!("expected Enum type"),
            }
        }
        _ => panic!("expected TypeDecl"),
    }
}

// ── 7. Subrange type ──

#[test]
fn subrange_type() {
    let src = r#"
TYPE SmallInt : INT(0..100);
END_TYPE

PROGRAM SubrangeTest
VAR
    s : SmallInt;
END_VAR
    s := 50;
END_PROGRAM
"#;
    let unit = parse_ok(src);
    assert_eq!(unit.declarations.len(), 2);
    match &unit.declarations[0] {
        Declaration::TypeDecl(td) => {
            assert_eq!(td.name.name, "SmallInt");
            match &td.type_spec.kind {
                TypeSpecKind::Subrange { base, .. } => {
                    assert_eq!(base.name, "INT");
                }
                _ => panic!("expected Subrange type, got {:?}", td.type_spec.kind),
            }
        }
        _ => panic!("expected TypeDecl"),
    }
}

// ── 8. Pointer type ──

#[test]
fn pointer_type() {
    let src = r#"
PROGRAM PointerTest
VAR
    pInt : POINTER TO INT;
    val : INT;
END_VAR
    val := pInt^;
END_PROGRAM
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::Program(p) => {
            // Check pointer type
            let ts = &p.var_blocks[0].declarations[0].type_spec;
            match &ts.kind {
                TypeSpecKind::Pointer(inner) => {
                    match &inner.kind {
                        TypeSpecKind::Named(ident) => assert_eq!(ident.name, "INT"),
                        _ => panic!("expected Named inner type"),
                    }
                }
                _ => panic!("expected Pointer type"),
            }
            // Check dereference in body
            assert_eq!(p.body.len(), 1);
            match &p.body[0].kind {
                StatementKind::Assignment { value, .. } => {
                    match &value.kind {
                        ExpressionKind::Dereference(_) => {}
                        _ => panic!("expected Dereference expression"),
                    }
                }
                _ => panic!("expected Assignment"),
            }
        }
        _ => panic!("expected Program"),
    }
}

// ── 9. String with length ──

#[test]
fn string_with_length() {
    let src = r#"
PROGRAM StringTest
VAR
    s1 : STRING;
    s2 : STRING[80];
    s3 : WSTRING[256];
    s4 : STRING(100);
END_VAR
    s1 := 'hello';
    s2 := 'world';
END_PROGRAM
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::Program(p) => {
            let decls = &p.var_blocks[0].declarations;
            assert_eq!(decls.len(), 4);

            // STRING with no length
            match &decls[0].type_spec.kind {
                TypeSpecKind::StringType { wide, length } => {
                    assert!(!wide);
                    assert!(length.is_none());
                }
                _ => panic!("expected StringType for s1"),
            }

            // STRING[80]
            match &decls[1].type_spec.kind {
                TypeSpecKind::StringType { wide, length } => {
                    assert!(!wide);
                    assert!(length.is_some());
                }
                _ => panic!("expected StringType for s2"),
            }

            // WSTRING[256]
            match &decls[2].type_spec.kind {
                TypeSpecKind::StringType { wide, length } => {
                    assert!(*wide);
                    assert!(length.is_some());
                }
                _ => panic!("expected StringType for s3"),
            }

            // STRING(100) -- CODESYS style
            match &decls[3].type_spec.kind {
                TypeSpecKind::StringType { wide, length } => {
                    assert!(!wide);
                    assert!(length.is_some());
                }
                _ => panic!("expected StringType for s4"),
            }
        }
        _ => panic!("expected Program"),
    }
}

// ── 10. All statement types ──

#[test]
fn all_statement_types() {
    let src = r#"
FUNCTION MyFunc : INT
VAR_INPUT
    a : INT;
END_VAR
    MyFunc := a;
END_FUNCTION

PROGRAM AllStatements
VAR
    x : INT := 0;
    y : INT := 0;
    b : BOOL := FALSE;
    state : INT := 0;
    i : INT;
    sum : INT := 0;
END_VAR
    (* Assignment *)
    x := 42;

    (* Function call *)
    y := MyFunc(x);

    (* IF / ELSIF / ELSE *)
    IF x > 10 THEN
        y := 1;
    ELSIF x > 5 THEN
        y := 2;
    ELSE
        y := 3;
    END_IF;

    (* CASE *)
    CASE state OF
        0:
            x := 0;
        1, 2:
            x := 1;
        3..5:
            x := 2;
    ELSE
        x := 99;
    END_CASE;

    (* FOR *)
    FOR i := 0 TO 10 BY 2 DO
        sum := sum + i;
    END_FOR;

    (* WHILE *)
    WHILE sum > 0 DO
        sum := sum - 1;
    END_WHILE;

    (* REPEAT *)
    REPEAT
        sum := sum + 1;
    UNTIL sum >= 100
    END_REPEAT;

    (* EXIT inside loop *)
    FOR i := 0 TO 100 DO
        IF i > 50 THEN
            EXIT;
        END_IF;
    END_FOR;

    (* CONTINUE inside loop *)
    FOR i := 0 TO 100 DO
        IF i = 5 THEN
            CONTINUE;
        END_IF;
        sum := sum + i;
    END_FOR;

    (* RETURN *)
    RETURN;
END_PROGRAM
"#;
    let unit = parse_ok(src);
    assert_eq!(unit.declarations.len(), 2);
    match &unit.declarations[1] {
        Declaration::Program(p) => {
            assert_eq!(p.name.name, "AllStatements");
            // Verify we have many statements (assignment, call, if, case, for, while, repeat, for+exit, for+continue, return)
            assert!(p.body.len() >= 10, "expected at least 10 statements, got {}", p.body.len());

            // Check statement kinds
            assert!(matches!(p.body[0].kind, StatementKind::Assignment { .. }));
            assert!(matches!(p.body[1].kind, StatementKind::Assignment { .. })); // y := MyFunc(x) is assignment
            assert!(matches!(p.body[2].kind, StatementKind::If { .. }));
            assert!(matches!(p.body[3].kind, StatementKind::Case { .. }));
            assert!(matches!(p.body[4].kind, StatementKind::For { .. }));
            assert!(matches!(p.body[5].kind, StatementKind::While { .. }));
            assert!(matches!(p.body[6].kind, StatementKind::Repeat { .. }));

            // Check IF has ELSIF and ELSE
            match &p.body[2].kind {
                StatementKind::If { elsif_branches, else_body, .. } => {
                    assert_eq!(elsif_branches.len(), 1);
                    assert!(else_body.is_some());
                }
                _ => unreachable!(),
            }

            // Check CASE has branches and else
            match &p.body[3].kind {
                StatementKind::Case { branches, else_body, .. } => {
                    assert_eq!(branches.len(), 3);
                    assert!(else_body.is_some());
                }
                _ => unreachable!(),
            }

            // Check FOR has BY clause
            match &p.body[4].kind {
                StatementKind::For { by, .. } => {
                    assert!(by.is_some());
                }
                _ => unreachable!(),
            }

            // Last statement should be RETURN
            let last = p.body.last().unwrap();
            assert!(matches!(last.kind, StatementKind::Return { .. }));
        }
        _ => panic!("expected Program"),
    }
}

// ── 11. Complex expressions ──

#[test]
fn complex_expressions() {
    let src = r#"
PROGRAM ComplexExpr
VAR
    x : INT;
    y : REAL;
    arr : ARRAY[0..9] OF INT;
    b : BOOL;
END_VAR
    (* Deeply nested parentheses *)
    x := ((((1 + 2) * 3) - 4) / 5);

    (* Operator precedence: power, mult, add *)
    y := 2.0 ** 3.0 + 1.0 * 4.0;

    (* MOD operator *)
    x := 17 MOD 5;

    (* Chained comparison *)
    b := (x > 0) AND (x < 100) OR (x = 42);

    (* NOT operator *)
    b := NOT b;

    (* Negation *)
    x := -x;

    (* XOR *)
    b := b XOR TRUE;

    (* Typed literals *)
    x := INT#5;
    y := REAL#3.14;

    (* Array index of complex expression *)
    x := arr[x + 1];

    (* Nested member access — uses identifier-based access *)
    (* p.x + p.y style would need struct vars; just test basic chaining *)
    x := x + y;
END_PROGRAM
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::Program(p) => {
            assert!(p.body.len() >= 10, "expected many statements, got {}", p.body.len());

            // Check deeply nested parens produce Parenthesized
            match &p.body[0].kind {
                StatementKind::Assignment { value, .. } => {
                    match &value.kind {
                        ExpressionKind::Parenthesized(_) => {}
                        _ => panic!("expected Parenthesized expression"),
                    }
                }
                _ => panic!("expected Assignment"),
            }

            // Check power expression
            match &p.body[1].kind {
                StatementKind::Assignment { value, .. } => {
                    match &value.kind {
                        ExpressionKind::BinaryOp { op: BinaryOp::Add, .. } => {}
                        _ => panic!("expected Add at top level of power expression (precedence)"),
                    }
                }
                _ => panic!("expected Assignment"),
            }

            // Check MOD
            match &p.body[2].kind {
                StatementKind::Assignment { value, .. } => {
                    match &value.kind {
                        ExpressionKind::BinaryOp { op: BinaryOp::Mod, .. } => {}
                        _ => panic!("expected Mod expression"),
                    }
                }
                _ => panic!("expected Assignment"),
            }

            // Check NOT (index 4: b := NOT b)
            match &p.body[4].kind {
                StatementKind::Assignment { value, .. } => {
                    match &value.kind {
                        ExpressionKind::UnaryOp { op: UnaryOp::Not, .. } => {}
                        _ => panic!("expected NOT expression"),
                    }
                }
                _ => panic!("expected Assignment"),
            }

            // Check negation (index 5: x := -x)
            match &p.body[5].kind {
                StatementKind::Assignment { value, .. } => {
                    match &value.kind {
                        ExpressionKind::UnaryOp { op: UnaryOp::Neg, .. } => {}
                        _ => panic!("expected Neg expression"),
                    }
                }
                _ => panic!("expected Assignment"),
            }

            // Check typed literal (index 7: x := INT#5)
            match &p.body[7].kind {
                StatementKind::Assignment { value, .. } => {
                    match &value.kind {
                        ExpressionKind::TypedLiteral { type_name, .. } => {
                            assert_eq!(type_name.name, "INT");
                        }
                        _ => panic!("expected TypedLiteral expression"),
                    }
                }
                _ => panic!("expected Assignment"),
            }
        }
        _ => panic!("expected Program"),
    }
}

// ── 12. Multiple variable blocks ──

#[test]
fn multiple_var_blocks() {
    let src = r#"
FUNCTION_BLOCK MultiVarFB
VAR_INPUT
    input1 : INT;
END_VAR
VAR_OUTPUT
    output1 : INT;
END_VAR
VAR_IN_OUT
    inout1 : INT;
END_VAR
VAR_TEMP
    temp1 : INT;
END_VAR
VAR
    local1 : INT;
END_VAR
VAR CONSTANT
    CVAL : INT := 42;
END_VAR
VAR RETAIN
    retained1 : INT;
END_VAR
    output1 := input1 + CVAL;
END_FUNCTION_BLOCK
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::FunctionBlock(fb) => {
            assert_eq!(fb.name.name, "MultiVarFB");
            assert_eq!(fb.var_blocks.len(), 7);

            assert_eq!(fb.var_blocks[0].kind, VarBlockKind::VarInput);
            assert_eq!(fb.var_blocks[1].kind, VarBlockKind::VarOutput);
            assert_eq!(fb.var_blocks[2].kind, VarBlockKind::VarInOut);
            assert_eq!(fb.var_blocks[3].kind, VarBlockKind::VarTemp);
            assert_eq!(fb.var_blocks[4].kind, VarBlockKind::Var);
            assert!(!fb.var_blocks[4].is_constant);

            // VAR CONSTANT
            assert_eq!(fb.var_blocks[5].kind, VarBlockKind::Var);
            assert!(fb.var_blocks[5].is_constant);

            // VAR RETAIN
            assert_eq!(fb.var_blocks[6].kind, VarBlockKind::Var);
            assert!(fb.var_blocks[6].is_retain);
        }
        _ => panic!("expected FunctionBlock"),
    }
}

// ── 13. Multi-variable declaration ──

#[test]
fn multi_var_declaration() {
    let src = r#"
PROGRAM MultiVar
VAR
    a, b, c : INT;
    x, y : REAL := 0.0;
END_VAR
END_PROGRAM
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::Program(p) => {
            let decls = &p.var_blocks[0].declarations;
            // a, b, c : INT  =>  3 VarDecls
            // x, y : REAL := 0.0  =>  2 VarDecls
            assert_eq!(decls.len(), 5);
            assert_eq!(decls[0].name.name, "a");
            assert_eq!(decls[1].name.name, "b");
            assert_eq!(decls[2].name.name, "c");
            assert_eq!(decls[3].name.name, "x");
            assert_eq!(decls[4].name.name, "y");
            // All three a,b,c should have INT type
            for d in &decls[0..3] {
                match &d.type_spec.kind {
                    TypeSpecKind::Named(ident) => assert_eq!(ident.name, "INT"),
                    _ => panic!("expected Named(INT)"),
                }
            }
            // x,y should have initializer
            assert!(decls[3].initializer.is_some());
            assert!(decls[4].initializer.is_some());
        }
        _ => panic!("expected Program"),
    }
}

// ── 14. Function block with EXTENDS and IMPLEMENTS ──

#[test]
fn function_block_extends() {
    let src = r#"
FUNCTION_BLOCK BaseFB
VAR
    base_val : INT;
END_VAR
END_FUNCTION_BLOCK

FUNCTION_BLOCK DerivedFB EXTENDS BaseFB IMPLEMENTS IRunnable, ILoggable
VAR
    derived_val : INT;
END_VAR
    METHOD Run : BOOL
        Run := TRUE;
    END_METHOD
END_FUNCTION_BLOCK
"#;
    let unit = parse_ok(src);
    assert_eq!(unit.declarations.len(), 2);

    match &unit.declarations[1] {
        Declaration::FunctionBlock(fb) => {
            assert_eq!(fb.name.name, "DerivedFB");
            assert!(fb.extends.is_some());
            assert_eq!(fb.extends.as_ref().unwrap().name, "BaseFB");
            assert_eq!(fb.implements.len(), 2);
            assert_eq!(fb.implements[0].name, "IRunnable");
            assert_eq!(fb.implements[1].name, "ILoggable");
            assert_eq!(fb.methods.len(), 1);
            assert_eq!(fb.methods[0].name.name, "Run");
        }
        _ => panic!("expected FunctionBlock"),
    }
}

// ── 15. Negative: incomplete IF ──

#[test]
fn negative_parse_incomplete_if() {
    let src = r#"
PROGRAM Bad
VAR
    x : INT;
    y : INT;
END_VAR
    IF x > 0 THEN
        y := 1;
END_PROGRAM
"#;
    let (_unit, errors) = parse_expect_errors(src);
    // Should have errors about missing END_IF
    assert!(
        !errors.is_empty(),
        "expected errors for missing END_IF"
    );
}

// ── 16. Negative: missing semicolon ──

#[test]
fn negative_parse_missing_semicolon() {
    // Completely broken syntax: missing colon in var decl
    let src = r#"
PROGRAM Bad
VAR
    x INT;
END_VAR
END_PROGRAM
"#;
    let (_unit, errors) = plcc_st::parse(src);
    assert!(
        !errors.is_empty(),
        "expected errors for missing colon in variable declaration"
    );
}

// ── 17. Comment styles ──

#[test]
fn comment_styles() {
    let src = r#"
(* This is a block comment *)
PROGRAM CommentTest
VAR
    x : INT; (* inline block comment *)
    // line comment
    y : INT;
END_VAR
    // assignment with comment
    x := 1; (* another comment *)
    y := 2;
END_PROGRAM
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::Program(p) => {
            assert_eq!(p.name.name, "CommentTest");
            assert_eq!(p.var_blocks[0].declarations.len(), 2);
            assert_eq!(p.body.len(), 2);
        }
        _ => panic!("expected Program"),
    }
}

// ── 18. Empty program ──

#[test]
fn empty_program() {
    let src = "PROGRAM Empty END_PROGRAM";
    let unit = parse_ok(src);
    assert_eq!(unit.declarations.len(), 1);
    match &unit.declarations[0] {
        Declaration::Program(p) => {
            assert_eq!(p.name.name, "Empty");
            assert!(p.var_blocks.is_empty());
            assert!(p.body.is_empty());
        }
        _ => panic!("expected Program"),
    }
}

// ── 19. Pragma in source ──

#[test]
fn pragma_in_source() {
    // Pragmas are tokenized but not consumed by the parser.
    // The parser will hit error recovery when it sees a Pragma token
    // at the declaration level. Test that the parser recovers and still
    // produces the declarations.
    let src = r#"
PROGRAM PragmaTest
VAR
    x : INT;
END_VAR
    x := 1;
END_PROGRAM
"#;
    // This should parse cleanly since there are no pragmas inside
    let unit = parse_ok(src);
    assert_eq!(unit.declarations.len(), 1);

    // Now test with pragma at top level -- parser should recover
    let src_with_pragma = r#"
{warning disable}
PROGRAM PragmaTest2
VAR
    x : INT;
END_VAR
    x := 1;
END_PROGRAM
"#;
    let (unit, _errors) = plcc_st::parse(src_with_pragma);
    // The program should still be found even if pragma causes an error
    assert!(
        !unit.declarations.is_empty(),
        "expected at least one declaration even with pragma"
    );
}

// ── 20. Direct variable expressions ──

#[test]
fn direct_variable_expressions() {
    let src = r#"
PROGRAM DirectVarTest
VAR
    x : INT;
    b : BOOL;
END_VAR
    x := %IW0;
    b := %MX0.1;
    %QW10 := x;
END_PROGRAM
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::Program(p) => {
            assert_eq!(p.body.len(), 3);

            // x := %IW0
            match &p.body[0].kind {
                StatementKind::Assignment { value, .. } => {
                    match &value.kind {
                        ExpressionKind::DirectVariable(repr) => {
                            assert_eq!(repr, "%IW0");
                        }
                        _ => panic!("expected DirectVariable expression"),
                    }
                }
                _ => panic!("expected Assignment"),
            }

            // b := %MX0.1
            match &p.body[1].kind {
                StatementKind::Assignment { value, .. } => {
                    match &value.kind {
                        ExpressionKind::DirectVariable(repr) => {
                            assert_eq!(repr, "%MX0.1");
                        }
                        _ => panic!("expected DirectVariable expression"),
                    }
                }
                _ => panic!("expected Assignment"),
            }

            // %QW10 := x (direct variable as assignment target)
            match &p.body[2].kind {
                StatementKind::Assignment { target, .. } => {
                    match &target.kind {
                        ExpressionKind::DirectVariable(repr) => {
                            assert_eq!(repr, "%QW10");
                        }
                        _ => panic!("expected DirectVariable as assignment target"),
                    }
                }
                _ => panic!("expected Assignment"),
            }
        }
        _ => panic!("expected Program"),
    }
}

// ── Additional coverage tests ──

#[test]
fn function_no_return_type() {
    let src = r#"
FUNCTION DoSomething
VAR_INPUT
    a : INT;
END_VAR
END_FUNCTION
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::Function(f) => {
            assert_eq!(f.name.name, "DoSomething");
            assert!(f.return_type.is_none());
        }
        _ => panic!("expected Function"),
    }
}

#[test]
fn class_abstract_and_final() {
    let src = r#"
ABSTRACT CLASS AbstractBase
  METHOD DoWork
  END_METHOD
END_CLASS

FINAL CLASS Concrete
  METHOD DoWork
    ;
  END_METHOD
END_CLASS
"#;
    let unit = parse_ok(src);
    assert_eq!(unit.declarations.len(), 2);

    match &unit.declarations[0] {
        Declaration::Class(cls) => {
            assert!(cls.is_abstract);
            assert!(!cls.is_final);
            assert_eq!(cls.name.name, "AbstractBase");
        }
        _ => panic!("expected Class"),
    }

    match &unit.declarations[1] {
        Declaration::Class(cls) => {
            assert!(!cls.is_abstract);
            assert!(cls.is_final);
            assert_eq!(cls.name.name, "Concrete");
        }
        _ => panic!("expected Class"),
    }
}

#[test]
fn interface_extends() {
    let src = r#"
INTERFACE IBase
  METHOD BaseMethod : BOOL
  END_METHOD
END_INTERFACE

INTERFACE IDerived EXTENDS IBase
  METHOD DerivedMethod : INT
  END_METHOD
END_INTERFACE
"#;
    let unit = parse_ok(src);
    assert_eq!(unit.declarations.len(), 2);
    match &unit.declarations[1] {
        Declaration::Interface(iface) => {
            assert_eq!(iface.name.name, "IDerived");
            assert_eq!(iface.extends.len(), 1);
            assert_eq!(iface.extends[0].name, "IBase");
            assert_eq!(iface.methods.len(), 1);
        }
        _ => panic!("expected Interface"),
    }
}

#[test]
fn case_with_range_labels() {
    let src = r#"
PROGRAM CaseRange
VAR
    x : INT;
    y : INT;
END_VAR
    CASE x OF
        1..5:
            y := 1;
        6..10:
            y := 2;
        11, 12, 13..20:
            y := 3;
    END_CASE;
END_PROGRAM
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::Program(p) => {
            match &p.body[0].kind {
                StatementKind::Case { branches, else_body, .. } => {
                    assert_eq!(branches.len(), 3);
                    assert!(else_body.is_none());
                    // First branch: range 1..5
                    assert_eq!(branches[0].labels.len(), 1);
                    assert!(matches!(branches[0].labels[0], CaseLabel::Range(_, _)));
                    // Third branch: 11, 12, 13..20 (3 labels)
                    assert_eq!(branches[2].labels.len(), 3);
                    assert!(matches!(branches[2].labels[0], CaseLabel::Value(_)));
                    assert!(matches!(branches[2].labels[1], CaseLabel::Value(_)));
                    assert!(matches!(branches[2].labels[2], CaseLabel::Range(_, _)));
                }
                _ => panic!("expected Case statement"),
            }
        }
        _ => panic!("expected Program"),
    }
}

#[test]
fn named_function_call_args() {
    let src = r#"
PROGRAM NamedArgs
VAR
    timer : INT;
END_VAR
    timer := MyFunc(a := 1, b := 2);
END_PROGRAM
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::Program(p) => {
            match &p.body[0].kind {
                StatementKind::Assignment { value, .. } => {
                    match &value.kind {
                        ExpressionKind::FunctionCall { args, .. } => {
                            assert_eq!(args.len(), 2);
                            assert!(args[0].name.is_some());
                            assert_eq!(args[0].name.as_ref().unwrap().name, "a");
                            assert!(args[1].name.is_some());
                            assert_eq!(args[1].name.as_ref().unwrap().name, "b");
                        }
                        _ => panic!("expected FunctionCall"),
                    }
                }
                _ => panic!("expected Assignment"),
            }
        }
        _ => panic!("expected Program"),
    }
}

#[test]
fn union_type_declaration() {
    let src = r#"
TYPE MyUnion :
    UNION
        intVal : INT;
        realVal : REAL;
        boolVal : BOOL;
    END_UNION;
END_TYPE
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::TypeDecl(td) => {
            assert_eq!(td.name.name, "MyUnion");
            match &td.type_spec.kind {
                TypeSpecKind::Union(fields) => {
                    assert_eq!(fields.len(), 3);
                    assert_eq!(fields[0].name.name, "intVal");
                    assert_eq!(fields[1].name.name, "realVal");
                    assert_eq!(fields[2].name.name, "boolVal");
                }
                _ => panic!("expected Union type"),
            }
        }
        _ => panic!("expected TypeDecl"),
    }
}

#[test]
fn var_at_address() {
    let src = r#"
PROGRAM VarAtTest
VAR
    motor_on AT %QX0.0 : BOOL;
END_VAR
    motor_on := TRUE;
END_PROGRAM
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::Program(p) => {
            let decl = &p.var_blocks[0].declarations[0];
            assert_eq!(decl.name.name, "motor_on");
            assert!(decl.at_address.is_some());
            assert_eq!(decl.at_address.as_ref().unwrap().repr, "%QX0.0");
        }
        _ => panic!("expected Program"),
    }
}

#[test]
fn global_var_block() {
    let src = r#"
VAR_GLOBAL
    globalCounter : INT := 0;
    globalFlag : BOOL;
END_VAR
"#;
    let unit = parse_ok(src);
    assert_eq!(unit.declarations.len(), 1);
    match &unit.declarations[0] {
        Declaration::GlobalVarDecl(vb) => {
            assert_eq!(vb.kind, VarBlockKind::VarGlobal);
            assert_eq!(vb.declarations.len(), 2);
        }
        _ => panic!("expected GlobalVarDecl"),
    }
}

#[test]
fn method_with_access_modifiers() {
    let src = r#"
CLASS AccessTest
  PUBLIC METHOD PublicMethod : BOOL
    PublicMethod := TRUE;
  END_METHOD
  PRIVATE METHOD PrivateMethod
    ;
  END_METHOD
  PROTECTED METHOD ProtectedMethod
    ;
  END_METHOD
END_CLASS
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::Class(cls) => {
            assert_eq!(cls.methods.len(), 3);
            assert_eq!(cls.methods[0].access, Some(AccessModifier::Public));
            assert_eq!(cls.methods[1].access, Some(AccessModifier::Private));
            assert_eq!(cls.methods[2].access, Some(AccessModifier::Protected));
        }
        _ => panic!("expected Class"),
    }
}

#[test]
fn bool_and_time_literals() {
    let src = r#"
PROGRAM LiteralTest
VAR
    b1 : BOOL;
    b2 : BOOL;
    t : TIME;
    s : STRING;
END_VAR
    b1 := TRUE;
    b2 := FALSE;
    t := T#100ms;
    s := 'hello world';
END_PROGRAM
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::Program(p) => {
            assert_eq!(p.body.len(), 4);
            // TRUE
            match &p.body[0].kind {
                StatementKind::Assignment { value, .. } => {
                    match &value.kind {
                        ExpressionKind::BoolLiteral(true) => {}
                        _ => panic!("expected BoolLiteral(true)"),
                    }
                }
                _ => panic!("expected Assignment"),
            }
            // FALSE
            match &p.body[1].kind {
                StatementKind::Assignment { value, .. } => {
                    match &value.kind {
                        ExpressionKind::BoolLiteral(false) => {}
                        _ => panic!("expected BoolLiteral(false)"),
                    }
                }
                _ => panic!("expected Assignment"),
            }
            // TIME literal
            match &p.body[2].kind {
                StatementKind::Assignment { value, .. } => {
                    match &value.kind {
                        ExpressionKind::TimeLiteral(_) => {}
                        _ => panic!("expected TimeLiteral"),
                    }
                }
                _ => panic!("expected Assignment"),
            }
            // String literal
            match &p.body[3].kind {
                StatementKind::Assignment { value, .. } => {
                    match &value.kind {
                        ExpressionKind::StringLiteral(s) => {
                            assert_eq!(s, "hello world");
                        }
                        _ => panic!("expected StringLiteral"),
                    }
                }
                _ => panic!("expected Assignment"),
            }
        }
        _ => panic!("expected Program"),
    }
}

#[test]
fn array_of_struct_type() {
    let src = r#"
PROGRAM ArrayOfStruct
VAR
    points : ARRAY[0..9] OF STRUCT
        x : REAL;
        y : REAL;
    END_STRUCT;
END_VAR
END_PROGRAM
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::Program(p) => {
            let ts = &p.var_blocks[0].declarations[0].type_spec;
            match &ts.kind {
                TypeSpecKind::Array { base, ranges } => {
                    assert_eq!(ranges.len(), 1);
                    match &base.kind {
                        TypeSpecKind::Struct(fields) => {
                            assert_eq!(fields.len(), 2);
                        }
                        _ => panic!("expected Struct as array base type"),
                    }
                }
                _ => panic!("expected Array type"),
            }
        }
        _ => panic!("expected Program"),
    }
}

#[test]
fn multiple_declarations_in_one_source() {
    let src = r#"
FUNCTION Add : INT
VAR_INPUT
    a : INT;
    b : INT;
END_VAR
    Add := a + b;
END_FUNCTION

FUNCTION_BLOCK Accumulator
VAR_INPUT
    val : INT;
END_VAR
VAR_OUTPUT
    total : INT;
END_VAR
VAR
    sum : INT := 0;
END_VAR
    sum := sum + val;
    total := sum;
END_FUNCTION_BLOCK

PROGRAM Main
VAR
    acc : Accumulator;
    result : INT;
END_VAR
    result := Add(a := 1, b := 2);
END_PROGRAM
"#;
    let unit = parse_ok(src);
    assert_eq!(unit.declarations.len(), 3);
    assert!(matches!(unit.declarations[0], Declaration::Function(_)));
    assert!(matches!(unit.declarations[1], Declaration::FunctionBlock(_)));
    assert!(matches!(unit.declarations[2], Declaration::Program(_)));
}

#[test]
fn empty_statement_semicolons() {
    let src = r#"
PROGRAM EmptyStmts
VAR
    x : INT;
END_VAR
    ;
    ;
    x := 1;
    ;
END_PROGRAM
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::Program(p) => {
            // Should have empty statements plus the assignment
            let non_empty: Vec<_> = p.body.iter()
                .filter(|s| !matches!(s.kind, StatementKind::Empty))
                .collect();
            assert_eq!(non_empty.len(), 1);
            let empties: Vec<_> = p.body.iter()
                .filter(|s| matches!(s.kind, StatementKind::Empty))
                .collect();
            assert_eq!(empties.len(), 3);
        }
        _ => panic!("expected Program"),
    }
}

#[test]
fn ref_to_type() {
    let src = r#"
PROGRAM RefTest
VAR
    pRef : REF_TO INT;
END_VAR
END_PROGRAM
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::Program(p) => {
            match &p.var_blocks[0].declarations[0].type_spec.kind {
                TypeSpecKind::Pointer(inner) => {
                    match &inner.kind {
                        TypeSpecKind::Named(ident) => assert_eq!(ident.name, "INT"),
                        _ => panic!("expected Named inner type for REF_TO"),
                    }
                }
                _ => panic!("expected Pointer type for REF_TO"),
            }
        }
        _ => panic!("expected Program"),
    }
}

#[test]
fn struct_with_initializers() {
    let src = r#"
TYPE Config :
    STRUCT
        maxSpeed : INT := 100;
        enabled : BOOL := TRUE;
        name : STRING[32] := 'default';
    END_STRUCT;
END_TYPE
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::TypeDecl(td) => {
            match &td.type_spec.kind {
                TypeSpecKind::Struct(fields) => {
                    assert_eq!(fields.len(), 3);
                    assert!(fields[0].initializer.is_some());
                    assert!(fields[1].initializer.is_some());
                    assert!(fields[2].initializer.is_some());
                }
                _ => panic!("expected Struct type"),
            }
        }
        _ => panic!("expected TypeDecl"),
    }
}

#[test]
fn var_non_retain() {
    let src = r#"
FUNCTION_BLOCK NonRetainFB
VAR NON_RETAIN
    temp : INT;
END_VAR
END_FUNCTION_BLOCK
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::FunctionBlock(fb) => {
            assert!(fb.var_blocks[0].is_non_retain);
            assert!(!fb.var_blocks[0].is_retain);
        }
        _ => panic!("expected FunctionBlock"),
    }
}

#[test]
fn configuration_with_global_vars() {
    let src = r#"
CONFIGURATION ProdConfig
  VAR_GLOBAL
    systemMode : INT := 0;
  END_VAR
  RESOURCE PLC1 ON CPU1
    TASK MainTask(INTERVAL := T#20ms, PRIORITY := 0);
    PROGRAM Ctrl WITH MainTask : Controller;
  END_RESOURCE
  RESOURCE PLC2 ON CPU2
    TASK SlowTask(INTERVAL := T#100ms, PRIORITY := 5);
    PROGRAM Logger WITH SlowTask : DataLogger;
  END_RESOURCE
END_CONFIGURATION
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::Configuration(cfg) => {
            assert_eq!(cfg.name.name, "ProdConfig");
            assert_eq!(cfg.global_vars.len(), 1);
            assert_eq!(cfg.resources.len(), 2);
            assert_eq!(cfg.resources[0].name.name, "PLC1");
            assert_eq!(cfg.resources[1].name.name, "PLC2");
        }
        _ => panic!("expected Configuration"),
    }
}

#[test]
fn bit_access_on_variable() {
    let src = r#"
PROGRAM BitAccess
VAR
    w : WORD;
    b : BOOL;
END_VAR
    b := w.0;
    w.15 := TRUE;
END_PROGRAM
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::Program(p) => {
            assert_eq!(p.body.len(), 2);
            // b := w.0 — member access with integer member
            match &p.body[0].kind {
                StatementKind::Assignment { value, .. } => {
                    match &value.kind {
                        ExpressionKind::MemberAccess { member, .. } => {
                            assert_eq!(member.name, "0");
                        }
                        _ => panic!("expected MemberAccess for bit access"),
                    }
                }
                _ => panic!("expected Assignment"),
            }
        }
        _ => panic!("expected Program"),
    }
}

#[test]
fn for_without_by() {
    let src = r#"
PROGRAM ForNoBY
VAR
    i : INT;
    sum : INT := 0;
END_VAR
    FOR i := 1 TO 10 DO
        sum := sum + i;
    END_FOR;
END_PROGRAM
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::Program(p) => {
            match &p.body[0].kind {
                StatementKind::For { by, .. } => {
                    assert!(by.is_none(), "expected no BY clause");
                }
                _ => panic!("expected For statement"),
            }
        }
        _ => panic!("expected Program"),
    }
}

#[test]
fn nested_if_statements() {
    let src = r#"
PROGRAM NestedIf
VAR
    a : INT;
    b : INT;
    c : INT;
END_VAR
    IF a > 0 THEN
        IF b > 0 THEN
            IF c > 0 THEN
                a := 1;
            ELSE
                a := 2;
            END_IF;
        END_IF;
    ELSIF a < 0 THEN
        b := -1;
    END_IF;
END_PROGRAM
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::Program(p) => {
            assert_eq!(p.body.len(), 1);
            match &p.body[0].kind {
                StatementKind::If { then_body, elsif_branches, .. } => {
                    assert_eq!(then_body.len(), 1);
                    assert_eq!(elsif_branches.len(), 1);
                    // Nested IF inside then_body
                    match &then_body[0].kind {
                        StatementKind::If { then_body: inner, .. } => {
                            assert_eq!(inner.len(), 1);
                            assert!(matches!(inner[0].kind, StatementKind::If { .. }));
                        }
                        _ => panic!("expected nested If"),
                    }
                }
                _ => panic!("expected If"),
            }
        }
        _ => panic!("expected Program"),
    }
}

#[test]
fn return_with_value() {
    let src = r#"
FUNCTION EarlyReturn : INT
VAR_INPUT
    x : INT;
END_VAR
    IF x < 0 THEN
        RETURN;
    END_IF;
    EarlyReturn := x * 2;
END_FUNCTION
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::Function(f) => {
            assert_eq!(f.body.len(), 2);
            // The IF contains RETURN
            match &f.body[0].kind {
                StatementKind::If { then_body, .. } => {
                    assert_eq!(then_body.len(), 1);
                    assert!(matches!(then_body[0].kind, StatementKind::Return { .. }));
                }
                _ => panic!("expected If"),
            }
        }
        _ => panic!("expected Function"),
    }
}

#[test]
fn expression_as_statement() {
    let src = r#"
PROGRAM ExprStmt
VAR
    x : INT;
END_VAR
    MyProc(x);
END_PROGRAM
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::Program(p) => {
            assert_eq!(p.body.len(), 1);
            assert!(matches!(p.body[0].kind, StatementKind::FunctionCall { .. }));
        }
        _ => panic!("expected Program"),
    }
}

#[test]
fn alias_type() {
    let src = r#"
TYPE MyInt : INT;
END_TYPE
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::TypeDecl(td) => {
            assert_eq!(td.name.name, "MyInt");
            match &td.type_spec.kind {
                TypeSpecKind::Named(ident) => assert_eq!(ident.name, "INT"),
                _ => panic!("expected Named type for alias"),
            }
        }
        _ => panic!("expected TypeDecl"),
    }
}

#[test]
fn array_index_chained() {
    let src = r#"
PROGRAM ArrayChain
VAR
    matrix : ARRAY[0..9] OF ARRAY[0..9] OF INT;
    x : INT;
END_VAR
    x := matrix[1][2];
END_PROGRAM
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::Program(p) => {
            match &p.body[0].kind {
                StatementKind::Assignment { value, .. } => {
                    // matrix[1][2] should be ArrayIndex(ArrayIndex(matrix, [1]), [2])
                    match &value.kind {
                        ExpressionKind::ArrayIndex { array, indices } => {
                            assert_eq!(indices.len(), 1);
                            match &array.kind {
                                ExpressionKind::ArrayIndex { .. } => {}
                                _ => panic!("expected nested ArrayIndex"),
                            }
                        }
                        _ => panic!("expected ArrayIndex"),
                    }
                }
                _ => panic!("expected Assignment"),
            }
        }
        _ => panic!("expected Program"),
    }
}

#[test]
fn fb_method_call_chain() {
    let src = r#"
PROGRAM MethodChain
VAR
    obj : MyClass;
    result : INT;
END_VAR
    result := obj.GetValue();
    obj.SetValue(42);
END_PROGRAM
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::Program(p) => {
            assert_eq!(p.body.len(), 2);
            // result := obj.GetValue() — assignment with method call
            match &p.body[0].kind {
                StatementKind::Assignment { value, .. } => {
                    match &value.kind {
                        ExpressionKind::FunctionCall { callee, args } => {
                            assert!(args.is_empty());
                            match &callee.kind {
                                ExpressionKind::MemberAccess { member, .. } => {
                                    assert_eq!(member.name, "GetValue");
                                }
                                _ => panic!("expected MemberAccess callee"),
                            }
                        }
                        _ => panic!("expected FunctionCall expression"),
                    }
                }
                _ => panic!("expected Assignment"),
            }
        }
        _ => panic!("expected Program"),
    }
}

#[test]
fn var_with_initializer_expression() {
    let src = r#"
PROGRAM InitExpr
VAR
    x : INT := 2 + 3;
    y : REAL := 1.0 * 2.5;
    z : BOOL := NOT FALSE;
END_VAR
END_PROGRAM
"#;
    let unit = parse_ok(src);
    match &unit.declarations[0] {
        Declaration::Program(p) => {
            let decls = &p.var_blocks[0].declarations;
            assert_eq!(decls.len(), 3);
            // x := 2 + 3 should be BinaryOp
            match &decls[0].initializer.as_ref().unwrap().kind {
                ExpressionKind::BinaryOp { op: BinaryOp::Add, .. } => {}
                other => panic!("expected Add expression initializer, got {other:?}"),
            }
            // z := NOT FALSE should be UnaryOp
            match &decls[2].initializer.as_ref().unwrap().kind {
                ExpressionKind::UnaryOp { op: UnaryOp::Not, .. } => {}
                other => panic!("expected Not expression initializer, got {other:?}"),
            }
        }
        _ => panic!("expected Program"),
    }
}
