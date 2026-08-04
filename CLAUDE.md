# CLAUDE.md — plcc

## Project Overview

IEC 61131-3 Structured Text compiler written in Rust. Full language implementation targeting complete, modern, PLC-grade ST per the 3rd edition standard (2013) — not a subset. Uses LLVM (via inkwell) for codegen. Cross-compiles to any LLVM target triple: wasm32, thumbv8m, aarch64, x86_64, whatever.

Licensed MPL-2.0 with a compiler output exception. Dependencies must be
permissively licensed (MIT, Apache-2.0, BSD, ISC) — no GPL/LGPL.

Compiler runs on the build host. Cross-compiles via LLVM target triples.

## Architecture

Workspace crates:

```
plcc/
├── CLAUDE.md
├── Cargo.toml             # workspace root
├── crates/
│   ├── plcc-st/           # lexer, parser, AST, source spans
│   ├── plcc-hir/          # high-level IR, name resolution, type checking
│   ├── plcc-codegen/      # LLVM codegen via inkwell — any target triple
│   ├── plcc-runtime/      # runtime interface types (FB lifecycle, std FBs, std functions)
│   └── plcc-cli/          # CLI binary
└── tests/
    ├── fixtures/          # our .st test files by language feature
    └── external/          # fetched test corpora (gitignored)
```

### Crate Responsibilities

**plcc-st** — Lexer (`logos`) and recursive-descent parser. Produces AST with full source spans. Error recovery — report as many diagnostics as possible per parse, don't bail on first error. Use `miette` or `ariadne` for diagnostic rendering.

Must parse the complete IEC 61131-3 ST language:
- POUs: PROGRAM, FUNCTION_BLOCK, FUNCTION, METHOD, CLASS, INTERFACE (3rd edition OOP)
- Variable blocks: VAR, VAR_INPUT, VAR_OUTPUT, VAR_IN_OUT, VAR_GLOBAL, VAR_EXTERNAL, VAR_TEMP, VAR_ACCESS, VAR_CONFIG
- All elementary types: BOOL, BYTE, WORD, DWORD, LWORD, SINT, INT, DINT, LINT, USINT, UINT, UDINT, ULINT, REAL, LREAL, STRING, WSTRING, CHAR, WCHAR, TIME, LTIME, DATE, TIME_OF_DAY, DATE_AND_TIME, LDATE, LTOD, LDT
- Derived types: ARRAY, STRUCT, ENUM (typed enums), subranges, alias types, UNION
- All statements: assignment (:=), IF/ELSIF/ELSE/END_IF, CASE/END_CASE, FOR/TO/BY/DO/END_FOR, WHILE/DO/END_WHILE, REPEAT/UNTIL/END_REPEAT, EXIT, CONTINUE, RETURN
- All expressions: arithmetic, comparison, logical, bitwise, exponent (**), MOD, function calls, method calls, array indexing, struct member access, type conversions, parenthesized
- Direct representation: %I, %Q, %M with size prefixes X/B/W/D/L
- Configuration: CONFIGURATION, RESOURCE, TASK, WITH, ON
- CONSTANT, RETAIN, NON_RETAIN, AT, R_EDGE, F_EDGE
- Typed literals: INT#5, REAL#3.14, BOOL#TRUE, typed time/date literals
- Pragmas: {pragma} syntax

**plcc-hir** — Lower AST to HIR:
- Name resolution (nested scopes, POU lookup, USES/namespace resolution)
- Type inference and checking per the IEC type hierarchy (see below)
- FB instance tracking (each instantiation gets unique state)
- Constant folding
- Semantic validation (no recursion by default, assignment target validation, CASE completeness, etc.)
- Method resolution and interface conformance (3rd edition OOP)

IEC type hierarchy:
```
ANY
├── ANY_DERIVED
├── ANY_ELEMENTARY
│   ├── ANY_MAGNITUDE
│   │   ├── ANY_NUM
│   │   │   ├── ANY_REAL (REAL, LREAL)
│   │   │   └── ANY_INT
│   │   │       ├── ANY_SIGNED (SINT, INT, DINT, LINT)
│   │   │       └── ANY_UNSIGNED (USINT, UINT, UDINT, ULINT)
│   │   └── ANY_DURATION (TIME, LTIME)
│   ├── ANY_BIT (BOOL, BYTE, WORD, DWORD, LWORD)
│   ├── ANY_STRING (STRING, WSTRING)
│   ├── ANY_DATE (DATE, TIME_OF_DAY, DATE_AND_TIME, LDATE, LTOD, LDT)
│   └── ANY_CHARS
│       ├── ANY_STRING
│       └── CHAR, WCHAR
```

**plcc-codegen** — LLVM codegen via `inkwell`. One crate, any target. Emit LLVM IR, let LLVM handle the backend. Runtime model:
- FB instance state as LLVM struct types in memory
- Exported `scan()` entry point per PROGRAM
- Inputs/outputs via struct pointers from the host
- Timer FBs call external `elapsed_time_ms` symbol from the runtime
- Standard math via LLVM intrinsics

**plcc-runtime** — Shared types/traits defining the runtime contract. Not the runtime itself (that's platform-specific), just the interface:
- FB lifecycle (init, scan, reset)
- Standard FB interfaces (TON, TOF, TP, CTU, CTD, CTUD, R_TRIG, F_TRIG, SR, RS, RTC)
- Standard function signatures (math, string, type conversion — everything from Section 2.5 of the standard)
- Memory layout conventions

**plcc-cli** — Binary. Commands:
- `plcc compile <input.st> -o <o> --target <triple>` — compile to object/bitcode
- `plcc check <input.st>` — parse + type check only
- `plcc parse <input.st> --dump-ast` — dump AST (debug)

## Dependencies

Use `cargo add` to install everything — do NOT hand-write version numbers in Cargo.toml. Key crates:

- `logos` — lexer generator
- `inkwell` — LLVM C API bindings (check which LLVM version it needs, install matching `llvm-*-dev` system package)
- `miette` with `fancy` feature — error reporting with source spans
- `clap` with `derive` feature — CLI
- `thiserror` — error types
- `serde` with `derive` feature + `serde_json` — AST dump

All must be permissively licensed (MIT, Apache-2.0, or similar). No GPL/LGPL deps.

## Test Infrastructure

### Our Fixtures

Create `.st` files in `tests/fixtures/` covering every language feature:

```
tests/fixtures/
├── parse/                       # one file per grammar area
│   ├── literals.st
│   ├── expressions.st
│   ├── statements.st
│   ├── variables.st
│   ├── types.st
│   ├── pou_function.st
│   ├── pou_function_block.st
│   ├── pou_program.st
│   ├── oop.st                   # CLASS, INTERFACE, METHOD
│   ├── direct_representation.st
│   └── error_recovery/          # intentionally broken files
├── typecheck/
│   ├── implicit_conversions.st
│   ├── type_errors.st
│   ├── overloaded_functions.st
│   └── fb_instantiation.st
├── codegen/
│   ├── arithmetic.st
│   ├── control_flow.st
│   ├── function_calls.st
│   ├── function_blocks.st
│   ├── timers.st
│   └── arrays_structs.st
└── programs/
    ├── blink.st                 # toggle BOOL each scan
    ├── pid_simple.st            # PID controller FB
    └── state_machine.st         # CASE-based state machine
```

### External Test Corpora

Fetch into `tests/external/` (gitignored). These are test inputs only — we don't link or incorporate any of this code.

```bash
mkdir -p tests/external

# OSCAT Basic — ~40k lines, 500+ FBs, largest open-source ST corpus
git clone --depth 1 https://github.com/simsum/oscat.git tests/external/oscat
git clone --depth 1 https://github.com/RWTH-EBC/AixOCAT.git tests/external/aixocat

# RuSTy test suite — extensive per-feature .st files (LGPL project, test inputs only)
git clone --depth 1 https://github.com/PLC-lang/rusty.git tests/external/rusty

# iec-checker — OCaml ST static analyzer with test .st files
git clone --depth 1 https://github.com/jubnzv/iec-checker.git tests/external/iec-checker

# K-ST — 567 ST programs validated against CODESYS, CX-Programmer, GX Works2
# paper: "K-ST: A Formal Executable Semantics of the Structured Text Language for PLCs"
# Poskitt et al., IEEE TSE 2023 — look for supplementary material at https://github.com/cposkitt
```

### Test Patterns

```rust
// parse-only: valid ST parses without error
#[test]
fn parse_oscat_basic() {
    for entry in glob("tests/external/oscat/**/*.st") { /* parse, assert no errors */ }
}

// negative: invalid ST produces expected diagnostics
#[test]
fn typecheck_rejects_bool_plus_int() {
    let src = "PROGRAM t VAR x:BOOL; y:INT; z:INT; END_VAR z := x + y; END_PROGRAM";
    // should error: BOOL not in ANY_NUM
}

// codegen: compile to host native, dlopen, call scan(), check outputs
#[test]
fn codegen_arithmetic() {
    let obj = compile("tests/fixtures/codegen/arithmetic.st", Target::Host);
    // link, load, call, assert
}
```

## IEC 61131-3 Reference

- **Standard**: IEC 61131-3:2013 (3rd edition) is the target. The 4th edition (2025) exists but most compilers still implement 3rd. The EBNF grammar in Annex A is the authoritative parser spec.
- **Type system**: Section 2.3
- **Standard functions**: Section 2.5.1 — ADD, MUL, ABS, SQRT, SIN, COS, LN, EXP, etc. with overloading rules
- **Standard FBs**: Section 2.5.2 — SR, RS, R_TRIG, F_TRIG, CTU, CTD, CTUD, TON, TOF, TP, RTC
- **OOP**: Section 6 (3rd edition) — CLASS, INTERFACE, METHOD, EXTENDS, IMPLEMENTS

## Coding Conventions

- **Use CLI tools for all scaffolding.** `cargo init`, `cargo new`, `cargo add`, etc. Do NOT hand-write Cargo.toml boilerplate or manually create directory structures.
- Use `miette::Result` for user-facing errors with source spans. `thiserror` for internal errors.
- No `unwrap()` in library crates. Tests and CLI only.
- Source spans on every AST/HIR node. Non-negotiable.
- Consider arena allocation (`bumpalo` or typed-arena) for AST nodes.

## Memory discipline — read this before running anything

**Always run `cargo` and any plcc binary under a memory cap.** Test binaries in this
repo have reached 90+ GiB resident and OOM-killed the whole WSL VM:

```
hircheck          92.8 GiB     (probe binary)
plcc_st-f0d3eb6   38.6 GiB     (cargo test binary for plcc-st)
plcc_base_review  89.7 GiB     (probe binary)
```

Use `bin/cap`, a two-line `ulimit -v` wrapper, so a runaway hits MemoryError and dies
alone instead of taking the machine with it:

```bash
bin/cap cargo test --workspace          # 8 GiB default
CAP_GB=16 bin/cap cargo build --release
```

If `bin/` is missing (it is gitignored), it is just:

```bash
ulimit -v $(( 8 * 1024 * 1024 )) && exec "$@"
```

Do NOT use systemd-run, cgroups, or anything that touches system state for this.

Two specific things that blow up, and must be capped:

- **Compiling the whole OSCAT corpus as a single unit.** 559 files in one LLVM module
  is genuinely enormous. Compile files individually unless the merged number is the
  point, and cap it either way.
- **Parallel agents each running cargo.** Every worktree carries its own `target/`
  (multi-GB) and its own compile. Keep concurrent cargo jobs low, and remove worktrees
  when done (`git worktree remove`) — they do not get cleaned up automatically.

`plcc_st` reaching 38.6 GiB is a *parser* test binary and almost certainly indicates a
real unbounded-growth bug in plcc, not just test weight. Investigate under a cap.

## Phase Plan

### Phase 1: Parse
- Lexer with logos — all IEC tokens, keywords, literals
- Recursive-descent parser — complete ST grammar
- AST types with spans
- CLI `plcc parse --dump-ast`
- **Exit criteria**: parses OSCAT Basic with <5% failures

### Phase 2: Type Check
- Name resolution, scope analysis
- Full IEC type hierarchy and implicit conversion rules
- FB instance tracking, OOP method resolution
- CLI `plcc check`
- **Exit criteria**: rejects type errors, accepts valid OSCAT programs

### Phase 3: Codegen
- Lower HIR to LLVM IR via inkwell
- FB instance state as LLVM structs
- Scan cycle entry point
- Standard functions via LLVM intrinsics
- CLI `plcc compile -o out --target <triple>`
- **Exit criteria**: blink.st and state_machine.st compile and execute correctly

### Phase 4: Standard Library
- All standard FBs: TON, TOF, TP, R_TRIG, F_TRIG, CTU, CTD, CTUD, SR, RS, RTC
- All standard functions per Section 2.5.1
- **Exit criteria**: pid_simple.st compiles and produces correct output

## License

MPL-2.0 (`LICENSE`) with a compiler output exception (`LICENSE-EXCEPTION`).
Every source file gets:
```rust
// SPDX-License-Identifier: MPL-2.0
```

File-level reciprocity: modifications to plcc's own files come back under the
MPL, but linking plcc into a proprietary product — and compiling ST programs
with it — carries no obligation. Contributions are inbound = outbound, signed
off under the DCO (`CONTRIBUTING.md`). No CLA.

Do NOT incorporate any GPL/LGPL code. Do NOT copy from RuSTy, matiec, Beremiz,
OpenPLC v3, or OpenPLC Editor. `Autonomy-Logic/openplc-runtime` (OpenPLC v4) is
MIT and may be used with attribution — verify per-file headers, not just the
repo badge. External .st files are test inputs only.

## Getting Started

```bash
cargo init --name plcc .
# convert to workspace, create crates:
cargo new --lib crates/plcc-st
cargo new --lib crates/plcc-hir
cargo new --lib crates/plcc-codegen
cargo new --lib crates/plcc-runtime
cargo new --name plcc crates/plcc-cli
# cargo add for all deps, cargo add --path for inter-crate deps
# fetch test corpora into tests/external/
```

Start with the lexer. Get all tokens recognized. Build parser top-down from PROGRAM/END_PROGRAM with simple var decls and assignments, expand until OSCAT parses clean.
