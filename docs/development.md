# Developing plcc

## Layout

```
plcc/
├── crates/
│   ├── plcc-st/           Lexer (logos) + recursive-descent parser + AST
│   ├── plcc-plcopen/      PLCopen XML reader: LD/FBD/ST bodies lowered to the ST AST
│   ├── plcc-twincat/      TwinCAT 3 reader: .plcproj, .TcPOU/.TcDUT/.TcGVL/.TcIO/.TcTTO
│   ├── plcc-l5x/          Rockwell L5X reader: ladder + Logix ST lowered to the ST AST, Logix prelude
│   ├── plcc-ladder/       Dialect-neutral ladder model (JSON), rung text, IEC ↔ Logix translation
│   ├── plcc-hir/          Type checker, name resolution, IEC type hierarchy
│   ├── plcc-codegen/      LLVM codegen via inkwell
│   ├── plcc-stdlib/       IEC standard FBs as bundled ST source (TON, CTU, ...)
│   ├── plcc-runtime/      Runtime contract: host clock, FB traits, function specs
│   ├── plcc-hal/          Hardware Abstraction Layer for platform integration
│   └── plcc-cli/          CLI binary
└── tests/
    ├── fixtures/          ST test files by language feature
    └── external/          OSCAT, RuSTy corpora (gitignored)
```

## Building

Requires Rust 1.75+ and LLVM development headers.

The device catalog in `devices/` is a git submodule
([beigesys/plcc-devices](https://github.com/beigesys/plcc-devices)). Clone with
`git clone --recursive`, or run `git submodule update --init` in an existing
checkout. Without it, plcc falls back to built-in copies of the Opta and
Simulator manifests.

```bash
# Install LLVM (Ubuntu/Debian)
sudo apt install llvm-21-dev

# Build
cargo build --release

# Run tests
cargo test

# Run the Linux simulator example
cargo run --example linux_sim -p plcc-hal
```

## Test Suite

Representative suites (run `cargo test --workspace` for the current totals):

| Suite | Tests | What's Verified |
|-------|-------|-----------------|
| Parser (unit + fixtures + comprehensive) | 75 | Every grammar construct, error recovery; all 559 OSCAT files parse, and the whole corpus compiles in one invocation (oscat-conformance.md) |
| Type checker | 22 | IEC type hierarchy, implicit conversions, negative tests |
| Runtime (FBs + functions) | 64 | All 11 standard FBs, all math/selection/conversion functions |
| Codegen (JIT execution) | 156 | Arithmetic, control flow, functions, FB instantiation, arrays, OOP, stdlib, IEC conformance, IR safety, cross-compile, real-world PLC patterns |
| HAL (simulator + scan cycle) | 17 | Process image, clock, retain, diagnostics; compiled programs run end-to-end through the generic scan cycle (tasks, priorities, SINGLE, RETAIN warm start) |

Real-world PLC patterns verified end-to-end with JIT execution:
- PID controllers
- State machines with timed transitions
- Traffic light sequencing
- Pump interlock logic
- Batch counting
- Moving average filters
- Conveyor startup sequences
- Alarm priority encoding

