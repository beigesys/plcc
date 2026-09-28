# plcc

IEC 61131-3 Structured Text compiler written in Rust. Compiles ST to native code via LLVM for any target: x86_64, ARM, RISC-V, WebAssembly.

## Quick Start

```bash
# Parse and check (the same type check `compile` runs first; warnings do not
# stop a build, errors do — `compile --no-typecheck` skips it)
plcc parse program.st --dump-ast
plcc check program.st
plcc check main.st motor.st utils.st

# Compile to LLVM IR
plcc compile program.st -o program.ll

# Compile to native object
plcc compile program.st -o program.o --target thumbv7em-unknown-none-eabi

# Multi-file compilation
plcc compile main.st motor.st utils.st -o system.o

# Ladder / FBD / ST from a PLCopen XML project (mixes with .st files)
plcc compile plant.xml utils.st -o plant.o
```

Ladder Diagram and FBD come in as PLCopen XML (IEC 61131-10) and lower to the
same AST as ST; see [docs/ladder.md](docs/ladder.md).

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

and, for every module, a program-independent runtime contract: a statically
allocated instance of each program, the `%I`/`%Q`/`%M` process image, a task
table, and `plcc_init()` / `plcc_run_task(i)`. A runtime written once against
that contract runs any program -- see [Integration](#integration).

## Architecture

```
plcc/
├── crates/
│   ├── plcc-st/           Lexer (logos) + recursive-descent parser + AST
│   ├── plcc-plcopen/      PLCopen XML reader: LD/FBD/ST bodies lowered to the ST AST
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
| VAR, VAR_INPUT, VAR_OUTPUT, VAR_IN_OUT (by reference), VAR_TEMP, VAR_GLOBAL | Full |
| VAR CONSTANT, VAR RETAIN | Full |
| All elementary types (BOOL through LREAL, STRING, WSTRING, TIME, DATE) | Full — every TIME/date type is i64 nanoseconds (since 1970-01-01 for DATE and DT, since midnight for TOD); converted to/from numbers in CODESYS units (see Standard Library) |
| ARRAY (1D, multi-dimensional, negative and non-zero lower bounds) | Full |
| ARRAY aggregate initializers (`[10, 20, 30]`, `[3(0)]`) | Full |
| STRUCT (incl. field default initializers), ENUM, UNION, subranges, alias types | Full |
| IF/ELSIF/ELSE, CASE, FOR/TO/BY, WHILE, REPEAT/UNTIL | Full |
| EXIT, CONTINUE, RETURN | Full |
| CONFIGURATION, RESOURCE, TASK, program instances (`PROGRAM p WITH t : Main (in := g, out => h)`) | Full -- compiled to a task table; INTERVAL, PRIORITY, SINGLE |
| Direct representation (%I, %Q, %M), `AT` | Full -- CODESYS addressing (`%IW1` = bytes 2..3), bit/byte/word/dword/lword, in declarations and statements; partial `%I*` / VAR_CONFIG not yet |
| Typed literals (INT#5, REAL#3.14) | Full |
| Exponentiation `**` / EXPT | Full — per IEC Table 23/29 the result is ANY_REAL even for integer operands: an integer base converts to REAL (8/16-bit) or LREAL (32/64-bit, and bare literals), so `2 ** -1` is 0.5, `0 ** 0` is 1.0, `0 ** -1` is +inf; the result is converted back (rounded) when it is assigned to an integer variable (exact up to 2**53), and the type checker warns about that REAL-into-integer conversion, as CODESYS does |
| POINTER TO, dereference (^), ADR, SIZEOF | Full — `pt^` as a value and a target, `pt^[i]`, `pt^.f`; CODESYS byte-addressed pointer arithmetic (`pt := pt + 1`) |
| Pragmas, block/line comments | Full |

## Standard Library

**85+ functions** callable from ST code:

| Category | Functions |
|----------|-----------|
| Math | ABS, SQRT, SIN, COS, TAN, ASIN, ACOS, ATAN, ATAN2, EXP, LN, LOG, EXPT |
| Rounding | TRUNC, FLOOR, CEIL, ROUND |
| Selection | MIN, MAX, LIMIT, SEL |
| Bit ops | SHL, SHR, ROL, ROR |
| Memory | ADR, SIZEOF |
| String | LEN, CONCAT (2 or more inputs), LEFT, RIGHT, MID, INSERT, DELETE, FIND, REPLACE, and `=` `<>` `<` `<=` `>` `>=` on STRING — usable anywhere in an expression, nested, with literal arguments; results are truncated to the destination's length. WSTRING values are supported in declarations and assignments only |
| Time | ADD_TIME, SUB_TIME, MUL_TIME, DIV_TIME (and the L- variants), ADD_TOD_TIME, ADD_DT_TIME, SUB_DATE_DATE, SUB_TOD_TIME, SUB_TOD_TOD, SUB_DT_TIME, SUB_DT_DT, CONCAT_DATE_TOD, CONCAT_DATE, CONCAT_TOD, CONCAT_DT, DAY_OF_WEEK, CODESYS `TIME()`; operators `DT - DT`, `DT + TIME`, `TOD + TIME`, `DATE - DATE` |
| Type conversion | `<SRC>_TO_<DST>` between every pair of non-string elementary types (BOOL, bit strings, integers, REAL/LREAL, TIME/LTIME, DATE/TOD/DT and their L- forms, CHAR/WCHAR), and the overloaded `TO_<DST>`. REAL → integer rounds (halves away from zero) and saturates. TIME and TOD convert as milliseconds, DATE and DT as seconds since 1970-01-01, LTIME/LTOD/LDATE/LDT as nanoseconds (CODESYS units) |

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

Every compiled module exports the same runtime contract, so one runtime runs any
program -- no per-program structs or I/O glue:

```bash
plcc compile plc.st -o plc.o --target thumbv7em-none-eabihf \
    --emit-header plc.h --emit-symbols plc.json
```

```c
#include "plc.h"            // image sizes, task table, layouts checked by _Static_assert

plcc_init();                // every program instance, statically allocated
while (running) {
    read_field_inputs(plcc_image_i, PLCC_IMAGE_I_SIZE);    // latch %I
    for (uint32_t t = 0; t < PLCC_TASK_COUNT; t++)
        if (task_is_due(&plcc_tasks[t], now()))           // INTERVAL / SINGLE / PRIORITY
            plcc_run_task(t);
    write_field_outputs(plcc_image_q, PLCC_IMAGE_Q_SIZE); // flush %Q
}
```

- `AT %IX0.3`, `%QW1`, `%MD4` variables *are* bytes of `plcc_image_i/q/m`
  (CODESYS addressing, native byte order); the header lists every binding.
- A `CONFIGURATION` becomes the task table; without one, every PROGRAM runs in
  one cyclic `MainTask` (T#20ms, `--task-interval` to change).
- `plcc_retain_regions[]` points at every RETAIN variable, for persistence.
- `--emit-symbols` gives every variable's offset for HMI / Modbus mapping.
- The per-program `main_init(state)` / `main_scan(state)` are still exported.

The full contract, the addressing rules, the scheduling semantics and a complete
C runtime loop are in [docs/process-image.md](docs/process-image.md). In Rust,
`plcc_hal::scan::ScanCycle` runs a compiled module on any `plcc_hal::Platform`:

```bash
cargo run --example linux_sim -p plcc-hal -- plc.st --input 0=1 --scans 10
```

## Building

Requires Rust 1.75+ and LLVM development headers.

```bash
# Install LLVM (Ubuntu/Debian)
sudo apt install llvm-21-dev

# Build
cargo build --release

# Run tests (668 tests)
cargo test

# Run the Linux simulator example
cargo run --example linux_sim -p plcc-hal
```

## Test Suite

668 tests across all crates, all passing:

| Suite | Tests | What's Verified |
|-------|-------|-----------------|
| Parser (unit + fixtures + comprehensive) | 75 | Every grammar construct, error recovery, OSCAT corpus 98.6% |
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

## License

[Mozilla Public License 2.0](LICENSE), with a
[compiler output exception](LICENSE-EXCEPTION).

Use plcc in whatever you build and ship whatever you like — compiling your ST
program places no license obligation on it, and linking the runtime into a
proprietary product is expressly permitted. MPL reciprocity is file-level: if
you improve plcc itself, those files come back under the MPL.

Contributions are accepted under the same terms, signed off under the
[DCO](CONTRIBUTING.md). No CLA.
