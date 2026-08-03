# plcc

IEC 61131-3 Structured Text compiler written in Rust. Compiles ST to native code via LLVM for any target: x86_64, ARM, RISC-V, WebAssembly.

## Quick Start

```bash
# Parse and check
plcc parse program.st --dump-ast
plcc check program.st

# Compile to LLVM IR
plcc compile program.st -o program.ll

# Compile to native object
plcc compile program.st -o program.o --target thumbv7em-unknown-none-eabi

# Multi-file compilation
plcc compile main.st motor.st utils.st -o system.o
```

## What It Compiles

```iec
FUNCTION_BLOCK PID
VAR_INPUT
    setpoint : REAL;
    measured : REAL;
    kp : REAL := 1.0;
    ki : REAL := 0.1;
    kd : REAL := 0.05;
    dt : REAL := 0.01;
END_VAR
VAR_OUTPUT
    output : REAL;
END_VAR
VAR
    err : REAL;
    prev_err : REAL := 0.0;
    integral : REAL := 0.0;
END_VAR
    err := setpoint - measured;
    integral := integral + err * dt;
    output := kp * err + ki * integral + kd * (err - prev_err) / dt;
    output := LIMIT(-100.0, output, 100.0);
    prev_err := err;
END_FUNCTION_BLOCK

PROGRAM Main
VAR
    pid : PID;
    sensor : REAL;
    target : REAL := 50.0;
    control : REAL;
END_VAR
    pid(setpoint := target, measured := sensor, kp := 2.0);
    control := pid.output;
END_PROGRAM
```

This compiles to two native functions:

- `main_init(state: *mut u8)` -- applies variable initializers
- `main_scan(state: *mut u8)` -- executes one scan cycle

The PLC runtime calls `init` once, then `scan` in a loop at the configured task rate.

## Architecture

```
plcc/
├── crates/
│   ├── plcc-st/           Lexer (logos) + recursive-descent parser + AST
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

## Language Support

Complete IEC 61131-3:2013 (3rd edition) Structured Text:

| Feature | Status |
|---------|--------|
| PROGRAM, FUNCTION, FUNCTION_BLOCK | Full |
| CLASS, INTERFACE, METHOD (OOP) | Full |
| VAR, VAR_INPUT, VAR_OUTPUT, VAR_IN_OUT, VAR_TEMP, VAR_GLOBAL | Full |
| VAR CONSTANT, VAR RETAIN | Full |
| All elementary types (BOOL through LREAL, STRING, WSTRING, TIME, DATE) | Full |
| ARRAY (1D, multi-dimensional, non-zero lower bounds) | Full |
| STRUCT, ENUM, UNION, subranges, alias types | Full |
| IF/ELSIF/ELSE, CASE, FOR/TO/BY, WHILE, REPEAT/UNTIL | Full |
| EXIT, CONTINUE, RETURN | Full |
| CONFIGURATION, RESOURCE, TASK | Parsed |
| Direct representation (%I, %Q, %M) | Parsed |
| Typed literals (INT#5, REAL#3.14) | Full |
| POINTER TO, dereference (^) | Full |
| Pragmas, block/line comments | Full |

## Standard Library

**65+ functions** callable from ST code:

| Category | Functions |
|----------|-----------|
| Math | ABS, SQRT, SIN, COS, TAN, ASIN, ACOS, ATAN, ATAN2, EXP, LN, LOG, EXPT |
| Rounding | TRUNC, FLOOR, CEIL, ROUND |
| Selection | MIN, MAX, LIMIT, SEL |
| Bit ops | SHL, SHR, ROL, ROR |
| String | LEN, CONCAT, LEFT, RIGHT, MID, FIND |
| Time | ADD_TIME, SUB_TIME, MUL_TIME, DIV_TIME |
| Type conversion | 40+ variants: INT_TO_REAL, REAL_TO_INT, BYTE_TO_WORD, BOOL_TO_DINT, etc. |

**10 standard function blocks**, per IEC 61131-3 section 2.5.2:

SR, RS, R_TRIG, F_TRIG, CTU, CTD, CTUD, TON, TOF, TP

These are written in ST (`crates/plcc-stdlib/st/`), embedded in the compiler with
`include_str!`, and compiled into your module alongside your own POUs. There is no
runtime library to link and no ABI boundary — LLVM optimizes across the whole
program. Control it with `--stdlib`:

```
plcc compile prog.st -o prog.o                  # bundled-st (default)
plcc compile prog.st -o prog.o --stdlib none    # no prelude at all
```

A POU you define yourself supersedes the bundled one of the same name; the
bundled declaration is dropped.

`RTC` is not provided: it needs a wall clock, and the runtime contract
deliberately defines only a monotonic one.

The timers read time through the external `plcc_monotonic_ns()` symbol, so
elapsed time is real time and does not drift with scan period. Host and
simulator builds get an implementation from `plcc-runtime`; bare-metal
integrators supply their own. See [docs/runtime-symbols.md](docs/runtime-symbols.md).

Instantiating a function block that is not in scope is a **compile error** naming
the type — never a silently empty `scan()`.

## Cross-Compilation Targets

Any LLVM target triple. Tested:

- `x86_64-unknown-linux-gnu` -- desktop/server
- `aarch64-unknown-linux-gnu` -- ARM64 (RPi, server)
- `armv7-unknown-none-eabi` -- bare-metal ARM Cortex-A
- `thumbv7em-unknown-none-eabi` -- ARM Cortex-M4/M7 (PLC-class MCU)
- `wasm32-unknown-unknown` -- WebAssembly
- `riscv32-unknown-none-elf` -- RISC-V

## Hardware Abstraction Layer

The `plcc-hal` crate provides traits for PLC platform integrators:

```rust
use plcc_hal::*;

struct MyPlatform { /* your hardware */ }

impl Platform for MyPlatform {
    type Image = MyProcessImage;    // %I/%Q/%M memory-mapped I/O
    type Clk = MyRtc;              // monotonic + wall clock
    type Scheduler = MyTaskRunner;  // cyclic task execution
    type Retain = MyFlashStorage;   // RETAIN variable persistence
    type Dog = MyWatchdog;          // safety watchdog
    type Diag = MyDiagnostics;      // error reporting
    // ...
}
```

**HAL traits:**

| Trait | Purpose |
|-------|---------|
| `ProcessImage` | %I/%Q/%M I/O image with coherent update/commit |
| `Clock` | Monotonic time, wall clock, per-scan elapsed time |
| `TaskScheduler` | Cyclic and event-triggered tasks with priority |
| `IoDriver` | Fieldbus abstraction (EtherCAT, Modbus, PROFINET, CANopen) |
| `RetainStorage` | Persistent variables across power cycles |
| `Watchdog` | Safety monitoring with configurable timeout |
| `DiagnosticSink` | Structured error/warning reporting |
| `VariableAccess` | HMI/OPC UA read/write interface |

A `LinuxSimulator` reference implementation is included for development and testing.

## Integration

The compiler outputs native functions with C calling convention:

```c
// Compiled from ST
extern void main_init(void *state);
extern void main_scan(void *state);

// Your runtime loop
struct main_state state = {0};
main_init(&state);
while (running) {
    read_inputs(&process_image);
    main_scan(&state);
    write_outputs(&process_image);
    watchdog_kick();
    sleep_until_next_cycle();
}
```

## Building

Requires Rust 1.75+ and LLVM development headers.

```bash
# Install LLVM (Ubuntu/Debian)
sudo apt install llvm-21-dev

# Build
cargo build --release

# Run tests (328 tests)
cargo test

# Run the Linux simulator example
cargo run --example linux_sim -p plcc-hal
```

## Test Suite

328 tests across all crates, all passing:

| Suite | Tests | What's Verified |
|-------|-------|-----------------|
| Parser (unit + fixtures + comprehensive) | 75 | Every grammar construct, error recovery, OSCAT corpus 98.6% |
| Type checker | 22 | IEC type hierarchy, implicit conversions, negative tests |
| Runtime (FBs + functions) | 64 | All 11 standard FBs, all math/selection/conversion functions |
| Codegen (JIT execution) | 156 | Arithmetic, control flow, functions, FB instantiation, arrays, OOP, stdlib, IEC conformance, IR safety, cross-compile, real-world PLC patterns |
| HAL (simulator) | 11 | Process image, clock, retain, diagnostics, scan cycle |

Real-world PLC patterns verified end-to-end with JIT execution:
- PID controllers
- State machines with timed transitions
- Traffic light sequencing
- Pump interlock logic
- Batch counting
- Moving average filters
- Conveyor startup sequences
- Alarm priority encoding

## License

[Mozilla Public License 2.0](LICENSE), with a
[compiler output exception](LICENSE-EXCEPTION).

Use plcc in whatever you build and ship whatever you like — compiling your ST
program places no license obligation on it, and linking the runtime into a
proprietary product is expressly permitted. MPL reciprocity is file-level: if
you improve plcc itself, those files come back under the MPL.

Contributions are accepted under the same terms, signed off under the
[DCO](CONTRIBUTING.md). No CLA.
