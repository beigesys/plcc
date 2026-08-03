// SPDX-License-Identifier: MPL-2.0

//! LLVM IR safety verification tests for critical infrastructure PLC code.
//!
//! These tests compile ST source to LLVM IR and inspect the textual IR for
//! properties that are essential for safe, deterministic PLC execution.

use inkwell::context::Context;
use plcc_codegen::Compiler;
use regex::Regex;

// ---------------------------------------------------------------------------
// Test programs
// ---------------------------------------------------------------------------

const BLINK: &str = r#"
PROGRAM Blink
VAR
    output : BOOL := FALSE;
END_VAR
    output := NOT output;
END_PROGRAM
"#;

const ARITHMETIC: &str = r#"
PROGRAM Arith
VAR
    a : INT := 0;
    b : INT := 0;
    c : INT := 0;
    d : INT := 0;
END_VAR
    a := 10 + 5;
    b := 10 - 3;
    c := 4 * 7;
    d := 20 / 4;
END_PROGRAM
"#;

const STATE_MACHINE: &str = r#"
PROGRAM SM
VAR
    state : INT := 0;
    result : INT := 0;
END_VAR
    CASE state OF
        0:
            result := 10;
            state := 1;
        1:
            result := 20;
            state := 2;
        2:
            result := 30;
            state := 0;
    END_CASE;
END_PROGRAM
"#;

const FOR_LOOP: &str = r#"
PROGRAM ForSum
VAR
    sum : INT := 0;
    i : INT;
END_VAR
    sum := 0;
    FOR i := 1 TO 10 DO
        sum := sum + i;
    END_FOR;
END_PROGRAM
"#;

const WHILE_LOOP: &str = r#"
PROGRAM WhileTest
VAR
    count : INT := 0;
END_VAR
    count := 0;
    WHILE count < 7 DO
        count := count + 1;
    END_WHILE;
END_PROGRAM
"#;

const IF_ELSE: &str = r#"
PROGRAM IfElse
VAR
    x : INT := 0;
    result : INT := 0;
END_VAR
    x := x + 1;
    IF x > 5 THEN
        result := 1;
    ELSE
        result := 0;
    END_IF;
END_PROGRAM
"#;

const NESTED_LOOPS: &str = r#"
PROGRAM Nested
VAR
    i : INT;
    j : INT;
    total : INT := 0;
END_VAR
    total := 0;
    FOR i := 1 TO 3 DO
        FOR j := 1 TO 4 DO
            total := total + 1;
        END_FOR;
    END_FOR;
END_PROGRAM
"#;

const FUNCTION_CALL: &str = r#"
FUNCTION Double : INT
VAR_INPUT
    val : INT;
END_VAR
    Double := val * 2;
END_FUNCTION

PROGRAM CallTest
VAR
    x : INT := 0;
END_VAR
    x := Double(5);
END_PROGRAM
"#;

/// All test programs collected for batch verification.
const ALL_PROGRAMS: &[(&str, &str)] = &[
    ("blink", BLINK),
    ("arithmetic", ARITHMETIC),
    ("state_machine", STATE_MACHINE),
    ("for_loop", FOR_LOOP),
    ("while_loop", WHILE_LOOP),
    ("if_else", IF_ELSE),
    ("nested_loops", NESTED_LOOPS),
    ("function_call", FUNCTION_CALL),
];

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

/// Extract the text of a basic block by its label name from LLVM IR.
/// Returns the block content from the label to the next label or closing `}`.
fn extract_block<'a>(ir: &'a str, label: &str) -> Option<&'a str> {
    let needle = format!("{label}:");
    let start = ir.find(&needle)?;
    let after = &ir[start + needle.len()..];
    // Find the end: next line starting with a non-whitespace char that ends with ':'
    // or a closing '}' on its own line.
    let mut end = after.len();
    for (i, line) in after.lines().enumerate() {
        if i == 0 {
            continue; // skip the rest of the label line
        }
        let trimmed = line.trim();
        // A new label or closing brace ends the block.
        if (!trimmed.is_empty()
            && !trimmed.starts_with(';')
            && line.chars().next().map_or(false, |c| !c.is_whitespace()))
            || trimmed == "}"
        {
            end = after[..].find(line).unwrap_or(after.len());
            break;
        }
    }
    Some(&after[..end])
}

fn compile_to_ir(source: &str) -> String {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let context = Context::create();
    let mut compiler = Compiler::new(&context, "safety_test");
    compiler.compile(&unit).expect("codegen failed");
    compiler.emit_ir()
}

// ===========================================================================
// Memory Safety
// ===========================================================================

/// Verify IR contains no dynamically-sized alloca (stack overflow risk).
/// All alloca instructions must have a fixed type size, not a runtime value.
#[test]
fn no_unbounded_alloca() {
    // A dynamic alloca looks like: `%p = alloca i32, i32 %n` (has a count operand
    // that is not a constant). A fixed alloca is just `%p = alloca i32` or
    // `%p = alloca i32, align 4`. We flag any alloca whose count operand is a
    // non-constant (starts with %).
    let dynamic_alloca = Regex::new(r"alloca\s+\S+,\s+\S+\s+%").unwrap();

    for (name, src) in ALL_PROGRAMS {
        let ir = compile_to_ir(src);
        assert!(
            !dynamic_alloca.is_match(&ir),
            "[{name}] IR contains dynamically-sized alloca — stack overflow risk:\n{ir}"
        );
    }
}

/// All getelementptr instructions must use the `inbounds` flag for UB-free
/// memory access.
#[test]
fn all_gep_inbounds() {
    let gep_re = Regex::new(r"getelementptr\b").unwrap();
    let gep_inbounds_re = Regex::new(r"getelementptr\s+inbounds\b").unwrap();

    for (name, src) in ALL_PROGRAMS {
        let ir = compile_to_ir(src);
        let total = gep_re.find_iter(&ir).count();
        let inbounds = gep_inbounds_re.find_iter(&ir).count();
        assert_eq!(
            total,
            inbounds,
            "[{name}] {total} GEP instructions but only {inbounds} are inbounds — \
             {diff} lack the inbounds flag",
            diff = total - inbounds,
        );
    }
}

/// No `unreachable` instructions in the IR — their presence indicates a
/// compiler bug (dead code that LLVM cannot resolve).
#[test]
fn no_unreachable_instructions() {
    // Match the LLVM `unreachable` instruction (standalone on a line, not a
    // metadata label like `!unreachable`).
    let unreachable_re = Regex::new(r"(?m)^\s+unreachable\s*$").unwrap();

    for (name, src) in ALL_PROGRAMS {
        let ir = compile_to_ir(src);
        assert!(
            !unreachable_re.is_match(&ir),
            "[{name}] IR contains `unreachable` instruction — indicates compiler bug:\n{ir}"
        );
    }
}

/// Every function definition must end with a `ret` instruction (no
/// fall-through off the end of a function).
#[test]
fn all_functions_have_return() {
    // Each `define` block should contain at least one `ret` before its closing `}`.
    let define_re = Regex::new(r"(?ms)^define\s.*?\{(.+?)\n\}").unwrap();

    for (name, src) in ALL_PROGRAMS {
        let ir = compile_to_ir(src);
        for cap in define_re.captures_iter(&ir) {
            let body = &cap[1];
            let fn_header = cap[0].lines().next().unwrap_or("");
            assert!(
                body.contains("ret ") || body.contains("ret\n"),
                "[{name}] function `{fn_header}` has no ret instruction"
            );
        }
    }
}

/// For programs with N variables, verify GEP struct indices are all < N.
#[test]
fn struct_field_access_bounded() {
    // Extract struct GEP indices and check they are within bounds.
    // Struct GEPs look like: getelementptr inbounds %StructType, ptr %p, i32 0, i32 <idx>
    let struct_gep_re =
        Regex::new(r"getelementptr\s+inbounds\s+\S+,\s+ptr\s+\S+,\s+i32\s+0,\s+i32\s+(\d+)")
            .unwrap();

    // Programs and their variable counts:
    let cases: &[(&str, usize)] = &[
        (BLINK, 1),
        (ARITHMETIC, 4),
        (STATE_MACHINE, 2),
        (FOR_LOOP, 2),
        (WHILE_LOOP, 1),
        (IF_ELSE, 2),
        (NESTED_LOOPS, 3),
    ];

    for (src, var_count) in cases {
        let ir = compile_to_ir(src);
        for cap in struct_gep_re.captures_iter(&ir) {
            let idx: usize = cap[1].parse().unwrap();
            assert!(
                idx < *var_count,
                "GEP index {idx} >= variable count {var_count} — out-of-bounds struct access"
            );
        }
    }
}

// ===========================================================================
// Control Flow Safety
// ===========================================================================

/// Every basic block must end with a terminator instruction (ret, br, switch,
/// unreachable). A block without a terminator is undefined behavior.
#[test]
fn all_branches_terminate() {
    // In LLVM IR textual form, a basic block is introduced by a label
    // (or the implicit entry) and ends at the next label or closing `}`.
    // Terminators: ret, br, switch, indirectbr, invoke, resume, unreachable.
    let terminators = [
        "ret ",
        "ret\n",
        "br ",
        "switch ",
        "indirectbr ",
        "invoke ",
        "unreachable",
    ];

    let define_re = Regex::new(r"(?ms)^define\s.*?\{(.+?)\n\}").unwrap();
    // A label line looks like: `name:` (possibly with preds comment).
    let label_re = Regex::new(r"(?m)^(\S+):").unwrap();

    for (name, src) in ALL_PROGRAMS {
        let ir = compile_to_ir(src);
        for func_cap in define_re.captures_iter(&ir) {
            let body = &func_cap[1];
            let fn_header = func_cap[0].lines().next().unwrap_or("");

            // Split body into basic blocks by label boundaries.
            let labels: Vec<usize> = label_re.find_iter(body).map(|m| m.start()).collect();

            // Check each block (region between consecutive labels, or from
            // start to first label, or from last label to end).
            let mut boundaries = vec![0usize];
            boundaries.extend_from_slice(&labels);
            boundaries.push(body.len());

            for window in boundaries.windows(2) {
                let block = &body[window[0]..window[1]];
                let trimmed = block.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let has_terminator = terminators.iter().any(|t| trimmed.contains(t));
                assert!(
                    has_terminator,
                    "[{name}] basic block in `{fn_header}` lacks a terminator:\n{trimmed}"
                );
            }
        }
    }
}

/// FOR loops must have a comparison + conditional branch (not infinite loop).
#[test]
fn for_loop_has_exit_condition() {
    let programs_with_for = &[FOR_LOOP, NESTED_LOOPS];

    for src in programs_with_for {
        let ir = compile_to_ir(src);

        // FOR loops should have a `for_loop` block with icmp + br
        assert!(
            ir.contains("for_loop:") || ir.contains("for_loop"),
            "expected for_loop basic block label in IR"
        );
        // Must have a comparison instruction (icmp) for the loop bound
        assert!(
            ir.contains("icmp sle")
                || ir.contains("icmp slt")
                || ir.contains("icmp ule")
                || ir.contains("icmp ult"),
            "FOR loop IR lacks a comparison instruction for exit condition"
        );
        // Must have a conditional branch in the for_loop block
        let for_block = extract_block(&ir, "for_loop");
        assert!(
            for_block.map_or(false, |b| b.contains("br i1")),
            "FOR loop block lacks conditional branch (br i1)"
        );
    }
}

/// WHILE loops must have a comparison + conditional branch.
#[test]
fn while_loop_has_exit_condition() {
    let ir = compile_to_ir(WHILE_LOOP);

    // WHILE should produce a while_cond block with comparison + conditional branch
    assert!(
        ir.contains("while_cond:") || ir.contains("while_cond"),
        "expected while_cond basic block label in IR"
    );

    let while_block = extract_block(&ir, "while_cond");
    assert!(
        while_block.map_or(false, |b| b.contains("br i1")),
        "WHILE loop condition block lacks conditional branch (br i1)"
    );
}

/// CASE/switch instructions must always have a default label so that
/// unmatched selector values have defined behavior.
#[test]
fn case_has_default() {
    let ir = compile_to_ir(STATE_MACHINE);

    // In LLVM IR, a switch instruction always has a default label:
    // `switch i16 %sel, label %default [...]`
    // Verify the switch exists and has a default.
    let switch_re = Regex::new(r"switch\s+\S+\s+\S+,\s+label\s+%\S+").unwrap();
    assert!(
        switch_re.is_match(&ir),
        "CASE statement did not produce a switch instruction with default label:\n{ir}"
    );
}

// ===========================================================================
// Type Safety
// ===========================================================================

/// Binary arithmetic operations (add, sub, mul, sdiv, udiv, srem, urem) must
/// operate on same-width integer types — no mismatched widths like `add i16, i32`.
#[test]
fn no_type_mismatch_in_ops() {
    let bin_op_re =
        Regex::new(r"\b(add|sub|mul|sdiv|udiv|srem|urem)\s+(?:nsw\s+|nuw\s+|nsw nuw\s+)?(i\d+)\s")
            .unwrap();
    let int_type_re = Regex::new(r"\bi(\d+)\b").unwrap();

    for (name, src) in ALL_PROGRAMS {
        let ir = compile_to_ir(src);
        for line in ir.lines() {
            let trimmed = line.trim();
            if let Some(cap) = bin_op_re.captures(trimmed) {
                let op = &cap[1];
                let op_type = &cap[2]; // e.g. "i16"
                let op_width = &op_type[1..]; // e.g. "16"

                // Look at operand portion (after the declared type), skip metadata
                let after_type = &trimmed[cap.get(2).unwrap().end()..];
                let operand_part: &str = after_type.split('!').next().unwrap_or(after_type);

                // Skip lines with GEP (index types differ legitimately)
                if operand_part.contains("getelementptr") {
                    continue;
                }

                // Find all iN types in the operand part and check they match.
                // We must skip matches that are part of SSA value names like %i2
                // or %sum1 — only standalone type annotations count.
                for m in int_type_re.find_iter(operand_part) {
                    let start = m.start();
                    // Skip if preceded by '%' (it's a value name, not a type)
                    if start > 0 && operand_part.as_bytes()[start - 1] == b'%' {
                        continue;
                    }
                    let found_width = &m.as_str()[1..]; // strip leading 'i'
                    if found_width != op_width {
                        panic!(
                            "[{name}] type mismatch in `{op}` instruction: declared {op_type} \
                             but operand uses i{found_width}\nLine: {trimmed}"
                        );
                    }
                }
            }
        }
    }
}

/// Store instructions must write values whose type width matches the
/// pointed-to type. A store looks like: `store <ty> <val>, ptr <dest>`.
/// We verify the stored type is consistent (no storing i32 into an i16 slot).
#[test]
fn store_matches_alloca_type() {
    // Collect alloca names and their types, then verify stores to those
    // pointers use the same type.
    let alloca_re = Regex::new(r"%(\S+)\s*=\s*alloca\s+(i\d+|float|double)").unwrap();
    let store_re = Regex::new(r"store\s+(i\d+|float|double)\s+\S+,\s+ptr\s+%(\S+)").unwrap();

    for (name, src) in ALL_PROGRAMS {
        let ir = compile_to_ir(src);
        let mut alloca_types: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        for cap in alloca_re.captures_iter(&ir) {
            alloca_types.insert(cap[1].to_string(), cap[2].to_string());
        }

        for cap in store_re.captures_iter(&ir) {
            let store_type = &cap[1];
            let dest_name = &cap[2];
            if let Some(expected_type) = alloca_types.get(dest_name) {
                assert_eq!(
                    store_type,
                    expected_type.as_str(),
                    "[{name}] store type mismatch: storing {store_type} into alloca \
                     %{dest_name} of type {expected_type}"
                );
            }
        }
    }
}

/// Branch conditions must always be i1 — wider integers used as booleans
/// without proper truncation/comparison would be a type safety issue.
#[test]
fn bool_comparisons_use_i1() {
    // Conditional branches: `br i1 %cond, label %t, label %f`
    // We verify every `br` with two labels uses i1.
    let cond_br_re = Regex::new(r"br\s+(i\d+)\s+%\S+,\s+label").unwrap();

    for (name, src) in ALL_PROGRAMS {
        let ir = compile_to_ir(src);
        for cap in cond_br_re.captures_iter(&ir) {
            let br_type = &cap[1];
            assert_eq!(
                br_type, "i1",
                "[{name}] conditional branch uses {br_type} instead of i1 — \
                 wider int used as bool without truncation"
            );
        }
    }
}

// ===========================================================================
// Determinism (critical for PLCs)
// ===========================================================================

/// scan() functions must not call any external functions — only internal ones
/// defined in the same module. This ensures the program is self-contained and
/// deterministic.
#[test]
fn no_external_calls_in_scan() {
    // Collect all functions defined in the module (they appear as `define`).
    let define_re = Regex::new(r"define\s+\S+\s+@(\S+)\(").unwrap();
    // Call instructions: `call <retty> @<name>(`
    let call_re = Regex::new(r"call\s+\S+\s+@(\S+)\(").unwrap();

    for (name, src) in ALL_PROGRAMS {
        let ir = compile_to_ir(src);

        let defined_fns: std::collections::HashSet<String> = define_re
            .captures_iter(&ir)
            .map(|c| c[1].to_string())
            .collect();

        // Extract scan function bodies and check their calls.
        let scan_body_re =
            Regex::new(r"(?ms)^define\s+void\s+@\S+_scan\S*\(.*?\{(.+?)\n\}").unwrap();

        for func_cap in scan_body_re.captures_iter(&ir) {
            let body = &func_cap[1];
            for call_cap in call_re.captures_iter(body) {
                let callee = &call_cap[1];
                // LLVM intrinsics (llvm.*) are always safe
                if callee.starts_with("llvm.") {
                    continue;
                }
                assert!(
                    defined_fns.contains(callee),
                    "[{name}] scan function calls external function @{callee} — \
                     PLC scan must be self-contained"
                );
            }
        }
    }
}

/// No `fdiv` by a constant zero in the IR — floating-point divide-by-zero
/// produces infinity/NaN which is non-deterministic behavior in PLC context.
#[test]
fn no_floating_point_divide_by_zero() {
    // Pattern: `fdiv float|double %x, 0.0` or similar zero constants
    let fdiv_zero_re =
        Regex::new(r"fdiv\s+(?:float|double)\s+\S+,\s+(?:0\.0+(?:e\+?0+)?|0x0+)\b").unwrap();

    for (name, src) in ALL_PROGRAMS {
        let ir = compile_to_ir(src);
        assert!(
            !fdiv_zero_re.is_match(&ir),
            "[{name}] IR contains fdiv by constant zero — non-deterministic result"
        );
    }
}

/// Compile the same source twice and verify the IR is byte-identical.
/// Non-deterministic compilation would be catastrophic for PLC safety
/// certification (same source must always produce same binary).
#[test]
fn consistent_ir_across_compilations() {
    for (name, src) in ALL_PROGRAMS {
        let ir1 = compile_to_ir(src);
        let ir2 = compile_to_ir(src);
        assert_eq!(
            ir1, ir2,
            "[{name}] IR differs between two compilations of identical source — \
             non-deterministic codegen"
        );
    }
}
