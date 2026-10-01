# Language support

What plcc compiles: the IEC 61131-3 language, the standard library, and the targets it builds for.

## An example

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
that contract runs any program -- see [runtime.md](runtime.md).

## Language Support

Complete IEC 61131-3:2013 (3rd edition) Structured Text:

| Feature | Status |
|---------|--------|
| PROGRAM, FUNCTION, FUNCTION_BLOCK | Full |
| CLASS, METHOD, EXTENDS, THIS^, SUPER^ (OOP) | Full — methods are late-bound for every call through a variable of a known FB/CLASS type (an inherited method is compiled per derived POU); `SUPER^()` runs the base body |
| INTERFACE | Full — `itf := inst`, late-bound `itf.M(..)` (inputs, outputs, in-outs), `itf = 0` / `itf <> 0`, interface-typed FB inputs, FUNCTION parameters and arrays, EXTENDS between interfaces, IMPLEMENTS inherited from a base; no `__QUERYINTERFACE` yet |
| PROPERTY (GET/SET), ACTION, FB_init, VAR_STAT, AND_THEN / OR_ELSE, REFERENCE TO (CODESYS / TwinCAT) | Full — see [twincat.md](twincat.md) |
| TwinCAT 3 projects (`.plcproj`, `.TcPOU`, `.TcDUT`, `.TcGVL`, `.TcIO`, `.TcTTO` tasks) | ST bodies, with diagnostics at the line in the TwinCAT file; LD/FBD/CFC/SFC bodies and Beckhoff libraries (Tc2_System, ...) are reported, not compiled. of 61 open-source projects, all parse, 5 compile and 44 more stop only at Beckhoff libraries ([twincat.md](twincat.md)) |
| VAR, VAR_INPUT, VAR_OUTPUT, VAR_IN_OUT (by reference), VAR_TEMP, VAR_GLOBAL | Full |
| VAR CONSTANT, VAR RETAIN | Full |
| All elementary types (BOOL through LREAL, STRING, WSTRING, TIME, DATE) | Full — every TIME/date type is i64 nanoseconds (since 1970-01-01 for DATE and DT, since midnight for TOD); converted to/from numbers in CODESYS units (see Standard Library) |
| ARRAY (1D, multi-dimensional, negative and non-zero lower bounds) | Full |
| ARRAY aggregate initializers (`[10, 20, 30]`, `[3(0)]`) | Full |
| STRUCT (incl. field default initializers and `(a := 1, b := 2)` structure initializers), ENUM (bare, `E#V` and `E.V` enumerators), UNION, subranges, alias types | Full |
| IF/ELSIF/ELSE, CASE, FOR/TO/BY, WHILE, REPEAT/UNTIL | Full |
| EXIT, CONTINUE, RETURN | Full |
| CONFIGURATION, RESOURCE, TASK, program instances (`PROGRAM p WITH t : Main (in := g, out => h)`) | Full -- compiled to a task table; INTERVAL, PRIORITY, SINGLE |
| Direct representation (%I, %Q, %M), `AT` | Full -- CODESYS addressing (`%IW1` = bytes 2..3), bit/byte/word/dword/lword, in declarations and statements; partial `%I*` / VAR_CONFIG not yet |
| Typed literals (INT#5, REAL#3.14) | Full |
| Exponentiation `**` / EXPT | Full — per IEC Table 23/29 the result is ANY_REAL even for integer operands: an integer base converts to REAL (8/16-bit) or LREAL (32/64-bit, and bare literals), so `2 ** -1` is 0.5, `0 ** 0` is 1.0, `0 ** -1` is +inf; the result is converted back (rounded) when it is assigned to an integer variable (exact up to 2**53), and the type checker warns about that REAL-into-integer conversion, as CODESYS does |
| POINTER TO, dereference (^), ADR, SIZEOF | Full — `pt^` as a value and a target, `pt^[i]`, `pt^.f`; CODESYS byte-addressed pointer arithmetic (`pt := pt + 1`) |
| Pragmas, block/line comments | Full — `(* *)` nests, `/* */`, `//` |
| Ladder model (`plcc-ladder`, `plcc convert --to ladder-json\|plcopen\|l5x`) | One series/parallel ladder model (JSON, stable element ids) for PLCopen LD and Rockwell RLL; readers for both, PLCopen XML writer with automatic layout, L5X writer with tags derived from use; the L5X compiler lowers RLL through it; round trips tested — see [ladder-translation.md](ladder-translation.md) |
| IEC LD ↔ Logix RLL (`plcc convert --to l5x` / `--to plcopen`, `--dialect`) | Element-by-element translation with a mapping table (TIMER/COUNTER ↔ TON/TOF/RTO/CTU/CTD instances, ms ↔ TIME, one-shots with their storage bits, compares, math, CPT/CMP with Logix precedence); a warning names every element whose behaviour differs; what has no counterpart is reported NOT TRANSLATED. Translated programs are JIT-checked against the originals |
| ST → ladder (`plcc convert x.st --to plcopen`) | Boolean assignments as contacts and coils, set/reset coils, compare boxes, FB calls as boxes, math/MOVE boxes; everything else in ST boxes; exact (JIT-checked on the fixtures and a generated corpus) |
| ST emitter (`plcc convert --to st`) | Any AST (ST, PLCopen, L5X, TwinCAT) printed as canonical ST (`plcc_st::print_unit`); parse → print → parse gives the same AST on every fixture and all 559 OSCAT files. Source comments are not kept (the parser drops them); ladder rungs print with `(* rung N *)` comments |
| Rockwell Logix 5000 (`.L5X`) | Ladder (RLL) and ST routines, UDTs, AOIs, controller/program/module tags with initial data, aliases, tasks, JSR/SBR/RET; 100+ instruction mnemonics with Logix rung-condition, prescan, status-flag and fault semantics; FBD/SFC routines and motion/PID instructions not yet — see [l5x.md](l5x.md) |
| CODESYS extensions | Bit access `x.3` / `x.%X3`, `S=` / `R=`, `REFERENCE TO` / `REF=` / `__ISVALIDREF`, calling a PROGRAM from another POU, the CODESYS parameter names of SR/RS/CTU/CTD/CTUD, `VAR_INST` — see codesys-compatibility.md; not yet: PROPERTY, `ARRAY[*]` (known-issues.md) |

## Standard Library

**150+ functions** callable from ST code:

| Category | Functions |
|----------|-----------|
| Math | ABS, SQRT, SIN, COS, TAN, ASIN, ACOS, ATAN, ATAN2, EXP, LN, LOG, EXPT |
| Rounding | TRUNC, FLOOR, CEIL, ROUND |
| Selection | MIN, MAX (extensible), LIMIT, SEL, MUX, MOVE |
| Operators as functions | ADD, MUL (extensible), SUB, DIV, GT, GE, EQ, LE, LT (extensible, monotonic), NE |
| Bit ops | SHL, SHR, ROL, ROR |
| Memory | ADR, SIZEOF |
| String | LEN, CONCAT (2 or more inputs), LEFT, RIGHT, MID, INSERT, DELETE, FIND, REPLACE, and `=` `<>` `<` `<=` `>` `>=` on STRING — usable anywhere in an expression, nested, with literal arguments; results are truncated to the destination's length. WSTRING values are supported in declarations and assignments only |
| Time | ADD_TIME, SUB_TIME, MUL_TIME, DIV_TIME (and the L- variants), ADD_TOD_TIME, ADD_DT_TIME, SUB_DATE_DATE, SUB_TOD_TIME, SUB_TOD_TOD, SUB_DT_TIME, SUB_DT_DT, CONCAT_DATE_TOD, CONCAT_DATE, CONCAT_TOD, CONCAT_DT, DAY_OF_WEEK, CODESYS `TIME()`; operators `DT - DT`, `DT + TIME`, `TOD + TIME`, `DATE - DATE` |
| STRING conversions | `<int/bit/BOOL>_TO_STRING`, `STRING_TO_<int/BOOL>`, `REAL_TO_STRING`, `LREAL_TO_STRING`, `STRING_TO_REAL`, `STRING_TO_LREAL`, `TIME_TO_STRING`, `LTIME_TO_STRING`, `DATE_TO_STRING`, `TOD_TO_STRING`, `DT_TO_STRING` (the REAL and TIME/date ones are written in ST and compiled in only when used) |
| Type conversion | `<SRC>_TO_<DST>` between every pair of non-string elementary types (BOOL, bit strings, integers, REAL/LREAL, TIME/LTIME, DATE/TOD/DT and their L- forms, CHAR/WCHAR), and the overloaded `TO_<DST>`. REAL → integer rounds (halves away from zero) and saturates. TIME and TOD convert as milliseconds, DATE and DT as seconds since 1970-01-01, LTIME/LTOD/LDATE/LDT as nanoseconds (CODESYS units) |

**10 standard function blocks**, per IEC 61131-3 section 2.5.2:

SR, RS, R_TRIG, F_TRIG, CTU, CTD, CTUD, TON, TOF, TP

plus **RTO**, a retentive on-delay timer (IN, R, PT → Q, ET) that is not IEC
standard: the IEC counterpart of the Logix RTO instruction, used when ladder is
translated from Logix (ladder-translation.md).

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
integrators supply their own. See [runtime-symbols.md](runtime-symbols.md).

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

