<!-- SPDX-License-Identifier: MPL-2.0 -->

# The runtime contract: process image, tasks, retain

Every module plcc compiles exports the same small set of symbols. A runtime
written once against them — a Linux daemon, an Arduino sketch, a Rust
`plcc-hal` platform — runs **any** ST program without knowing its name, its
variables or its state layout. No per-program C struct, no hand-written I/O
copying.

```
          runtime (yours)                       compiled ST module (plcc)
 ┌────────────────────────────────┐     ┌────────────────────────────────────┐
 │ read field inputs ─────────────┼────▶│ plcc_image_i[]   %I   (latched)    │
 │ for each due task:             │     │                                    │
 │     plcc_run_task(i) ──────────┼────▶│ task i: prog_scan(&plcc_inst_…)    │
 │ write field outputs ◀──────────┼─────│ plcc_image_q[]   %Q                │
 │ persist plcc_retain_regions ◀──┼─────│ plcc_image_m[]   %M, RETAIN vars   │
 └────────────────────────────────┘     └────────────────────────────────────┘
```

`plcc compile prog.st -o prog.o --target <triple> --emit-header prog.h` writes the
matching C header; `--emit-symbols prog.json` writes the same information as JSON.

## Symbols

### Exported by every module (ABI version 1)

| Symbol | C type | What |
|---|---|---|
| `plcc_image_i`, `plcc_image_q`, `plcc_image_m` | `uint8_t[]` | The `%I`, `%Q`, `%M` areas. 8-byte aligned, zero-initialized, in `.bss`. |
| `plcc_process_image` | `const plcc_process_image_t` | `{input, input_size, output, output_size, memory, memory_size}` — the layout of `plcc_hal::ProcessImageLayout`. |
| `plcc_init` | `void (void)` | Initialize every program instance (and VAR_GLOBALs). Call once before any task. |
| `plcc_run_task` | `void (uint32_t task)` | Run every program instance of task `task` once, in declaration order. Out-of-range: no-op. |
| `plcc_tasks` | `const plcc_task_t[]` | The task table (below). |
| `plcc_retain_regions` | `const plcc_retain_region_t[]` | Every RETAIN variable: `{name, data, size}`. |
| `plcc_app` / `plcc_get_app()` | `const plcc_app_t` | Root descriptor pointing at all of the above. The function form is for loaders that resolve functions more easily than data (JIT, some `dlopen` shims). |
| `plcc_inst_<…>` | `plcc_prog_<name>_t` | Statically allocated state of each program instance. |
| `plcc_globals` | `plcc_globals_t` | VAR_GLOBAL storage (when there are any). |
| `<program>_init`, `<program>_scan` | `void (void *state)` | Per-PROGRAM entry points, unchanged. Hosts that allocate their own state keep working. |

```c
typedef struct plcc_program_instance {
    const char *name;          /* "RESOURCE.instance", or the PROGRAM name */
    const char *program_type;  /* PROGRAM POU name */
    void (*init)(void *state);
    void (*scan)(void *state);
    void *state;               /* the plcc_inst_… object */
    uint64_t state_size;
} plcc_program_instance_t;

typedef struct plcc_task {
    const char *name;
    int64_t interval_ns;       /* 0: not cyclic (event or free-running) */
    uint32_t priority;         /* IEC: 0 is the highest priority */
    uint32_t program_count;
    uint8_t (*single)(void);   /* current value of SINGLE, or NULL */
    const plcc_program_instance_t *programs;
} plcc_task_t;

typedef struct plcc_retain_region { const char *name; void *data; uint64_t size; } plcc_retain_region_t;

typedef struct plcc_app {
    uint32_t abi_version;      /* PLCC_ABI_VERSION */
    uint32_t task_count;
    const plcc_task_t *tasks;
    const plcc_process_image_t *image;
    void (*init)(void);
    void (*run_task)(uint32_t task);
    const plcc_retain_region_t *retain;
    uint32_t retain_count;
    uint32_t retain_signature; /* changes whenever the RETAIN layout changes */
} plcc_app_t;
```

All of these are plain data and function pointers with natural C layout, the same
on every target. Because they are exported, only one plcc module can be linked into
an image: compile all `.st` files of a PLC into one object (`plcc compile a.st b.st
-o plc.o`).

### Imported (the runtime supplies them)

See [runtime-symbols.md](runtime-symbols.md): `plcc_monotonic_ns()`,
`plcc_print()`, and `plcc_fault()` (the module carries a weak default that traps,
so defining it is optional but strongly advised — see
[Runtime faults](#runtime-faults)).

## Process image

### Storage: compiler-owned static arrays

The three areas are **defined by the compiled module** as fixed-size arrays, not
passed in by the runtime.

- A directly represented variable compiles to an access at a link-time constant
  address — one load or store, no pointer chase. That matters on a Cortex-M.
- Nothing is allocated at run time; it works bare-metal with no heap.
- The runtime still reaches everything through pointers (`plcc_process_image`,
  `plcc_app.image`), so a Rust or C runtime handles any program generically — which
  is what "inputs/outputs via pointers from the host" is for.
- The cost: the areas cannot be placed on top of a DMA buffer. A runtime copies its
  field buffers into `%I` before a task and out of `%Q` after. That copy *is* the
  IEC input latch / output flush, so the program sees inputs that do not change
  mid-scan, which is the semantics you want anyway.

### Addressing (CODESYS convention)

IEC 61131-3 §6.5.5 defines the syntax (`%` + area + size + number) and leaves the
mapping to memory implementation-defined. plcc follows CODESYS: the number after a
size prefix counts **units of that size**.

| Address | Location | Type it holds |
|---|---|---|
| `%IX3.5` | bit 5 (0 = LSB) of byte 3 | BOOL |
| `%I3.5` | same (no size prefix means X) | BOOL |
| `%IX29` | IEC flat bit number: bit 29 = byte 3, bit 5 | BOOL |
| `%IB7` | byte 7 | BYTE, SINT, USINT, CHAR |
| `%IW3` | bytes 6–7 | WORD, INT, UINT, WCHAR |
| `%ID3` | bytes 12–15 | DWORD, DINT, UDINT, REAL |
| `%IL3` | bytes 24–31 | LWORD, LINT, ULINT, LREAL, TIME, LTIME, DATE… |

`%Q…` and `%M…` work the same in their own areas. Consequences:

- **Overlaps alias.** `%IX0.3`, `%IB0`, `%IW0`, `%ID0` and `%IL0` all include byte 0
  bit 3. Writing one is visible through the others.
- **Native byte order.** `%IW0` is a `uint16_t` in the target's byte order; on a
  little-endian target (x86, ARM, RISC-V, wasm) `%IB0` is its low byte and
  `%IX1.7` its top bit. `PLCC_BIG_ENDIAN` in the header says which.
- **Natural alignment for free.** `%xWn` is at a multiple of 2, `%xDn` of 4, `%xLn`
  of 8, and each area is 8-aligned, so every elementary access is aligned — cores
  that fault on unaligned loads (Cortex-M0, some RISC-V) are safe. (Siemens and
  Beckhoff number by *byte*, `%IW1` = bytes 1–2; that is not what plcc does.)
- An ARRAY, STRUCT or STRING can be placed at any `B/W/D/L` address whose byte
  offset is a multiple of its alignment (`arr AT %IB2 : ARRAY[0..3] OF WORD`); it
  occupies consecutive bytes with the target's normal layout.

### Sizes

Each area is as large as the highest byte any address in the program touches
(`AT` declarations and `%…` written in statements) — possibly 0. Fix a size with
`plcc compile --image-size I=64 --image-size Q=64 --image-size M=256` (or
`Compiler::set_image_size`), e.g. so one runtime build fits every program; a size
smaller than the program needs is an error. The header has `PLCC_IMAGE_I_SIZE` etc.

### AT variables

```iec
VAR_GLOBAL
    estop AT %IX0.7 : BOOL;
END_VAR
PROGRAM Main
VAR
    start AT %IX0.0 : BOOL;
    speed AT %QW1   : INT := 0;
    lamp  AT %QX0.0 : BOOL;
END_VAR
```

- The variable **is** the image location — reads and writes go straight to
  `plcc_image_*`. There is no copy to keep in sync.
- Allowed in `VAR` of a PROGRAM, FUNCTION_BLOCK or CLASS, and in `VAR_GLOBAL`
  (including CONFIGURATION/RESOURCE VAR_GLOBAL). In a FUNCTION_BLOCK every
  instance shares the one location. Not allowed in VAR_INPUT / VAR_OUTPUT /
  VAR_IN_OUT / VAR_TEMP, FUNCTIONs or METHODs.
- A BOOL needs an X address; anything else needs a size prefix whose width equals
  the type's (`INT` at `%xW`, `REAL` at `%xD`, …). `a AT %IX0.1 : INT` or
  `a AT %IW0 : DINT` is an error.
- A bit variable is read with a shift and mask and written with a read-modify-write
  of its byte. That RMW is not atomic: if an interrupt or another thread writes
  the same `%Q`/`%M` byte while a task runs, one write can be lost. Keep the image
  private to the scan (latch/flush around tasks), as the scan cycle below does.
- A bit variable has no byte address: passing it to a VAR_IN_OUT, `ADR` of it, or
  reaching it as `fbinstance.bitvar` from outside is an error. Copy it to a BOOL.
- An initializer (`lamp AT %QX0.0 : BOOL := TRUE`) is applied by `plcc_init` (and
  by `<program>_init`), like any other.
- The PROGRAM/FB state struct still reserves a slot for an AT variable so field
  indices do not shift; the slot is never used (the header says so).
- `%QX0.1 := %IX0.3;` — direct addresses can be used in statements and
  expressions too; they are typed BOOL/BYTE/WORD/DWORD/LWORD by size prefix.
- Errors carry a source span: a bit above 7, a type that does not fit, misaligned
  aggregates, `%IW1.2`, hierarchical addresses (`%IX1.2.3`).
- **Not yet supported:** partially specified addresses (`AT %I*`, `%QW*`) and the
  VAR_CONFIG block that completes them. Both are reported as errors, not ignored.

## Tasks and program instances

### With a CONFIGURATION

```iec
CONFIGURATION Plant
    VAR_GLOBAL trigger AT %IX1.0 : BOOL; END_VAR
    RESOURCE Cpu ON Opta
        TASK Fast  (INTERVAL := T#5ms,  PRIORITY := 0);
        TASK Slow  (INTERVAL := T#100ms, PRIORITY := 2);
        TASK OnHit (SINGLE := trigger,  PRIORITY := 1);
        PROGRAM loop1 WITH Fast : Motor (setpoint := g_sp, speed => g_speed);
        PROGRAM log1  WITH Slow : Logger;
        PROGRAM idle  : Housekeeping;
    END_RESOURCE
END_CONFIGURATION
```

- One task table entry per TASK, in declaration order, followed by
  `__background` if any instance has no `WITH` (interval 0, priority
  `UINT32_MAX`, i.e. free-running at the lowest priority).
- Each `PROGRAM name WITH task : Type` is one statically allocated instance,
  `plcc_inst_<resource>_<name>`, named `RESOURCE.name` in the table. Two instances
  of the same PROGRAM have separate state.
- `INTERVAL` must be a constant TIME, `PRIORITY` a constant non-negative integer
  (IEC: 0 is the most urgent). `SINGLE` may be any BOOL expression over globals; the
  table holds a function returning its current value.
- Connections `(in := expr, out => target)` are compiled into `plcc_run_task`:
  inputs are assigned before the instance's scan, outputs after.
- TASK and PROGRAM may sit directly in the CONFIGURATION (one implicit resource).
- CONFIGURATION and RESOURCE VAR_GLOBALs become ordinary globals. Resource scoping
  is flattened: every POU sees them.
- One CONFIGURATION per compiled module.

### Without a CONFIGURATION

Every PROGRAM gets one instance, `plcc_inst_<program>`, in a single task named
`MainTask` — cyclic, `T#20ms` (the CODESYS standard-project default; change it with
`--task-interval`), priority 1 — in declaration order. The per-program
`<program>_init/_scan(state)` functions are still exported, so hosts that allocate
state themselves are unaffected (they then simply do not use `plcc_inst_*`).

### Scheduling (the runtime's job)

The task table only *describes* the schedule. The reference implementation is
`plcc_hal::scan::ScanCycle`, which follows IEC 61131-3 §6.8.2:

- `interval_ns > 0`: due every interval, while SINGLE (if present) is FALSE. A late
  task runs once and skips the missed activations (no burst of catch-up runs).
- `single != NULL`: due once on each FALSE→TRUE transition of `single()`.
- neither: free-running — due every pass of the loop.
- Due tasks run highest priority (lowest number) first, non-preemptively; ties in
  table order.
- Inputs are latched before readiness is evaluated (so an input edge can trigger a
  SINGLE task in the same pass) and outputs flushed after the last task.

A preemptive RTOS runtime may instead give each task its own thread and call
`plcc_run_task(i)` from it; it must then latch/flush per task and serialize access
to the image and globals.

## Start-up and RETAIN

1. Zeroed `.bss` is the only precondition.
2. `plcc_init()` applies every declared initial value (instances, VAR_GLOBALs,
   AT initializers). That is a **cold start**.
3. For a **warm start**, then copy saved RETAIN data back into
   `plcc_retain_regions[i].data` (sizes must match and `retain_signature` must equal
   the saved one; otherwise do a cold start).

`plcc_retain_regions` lists every RETAIN variable reachable from a static instance:
`VAR RETAIN` in a PROGRAM, `VAR RETAIN` inside FUNCTION_BLOCK instances declared in
those (IEC: per instance), and `VAR_GLOBAL RETAIN`. Each entry points at the live
variable, so saving is a loop of `memcpy`s — no separate retain copy exists or needs
syncing. Regions are named (`Cpu.loop1.starts`, `Main.odo.km`, `GLOBAL.g_hours`).
Not covered yet: RETAIN inside arrays of FB instances, and `PERSISTENT`.

`plcc_hal::Application::retain_snapshot` serializes them as `"PLCR"`, the
signature, then each region as `u32 size` + bytes (little-endian integers); any
format works as long as save and restore agree.

## Runtime faults

An integer division or `MOD` by zero does what it does in CODESYS: the task stops
with an exception, the PLC goes to STOP. Compiled code calls

```c
void plcc_fault(uint32_t code, const char *where);   /* must not return */
```

with `code` = `PLCC_FAULT_DIV_BY_ZERO` and `where` = `"plc.st:42:13: Mixer"`
(file:line:column and POU; `FB.METHOD` for a method) — see
[runtime-symbols.md](runtime-symbols.md#plcc_fault) for the full contract. The
fault happens in the middle of `plcc_run_task`: the statements after the division
have not run, and the instance state is part-way through a scan. The runtime must:

1. **Put the outputs in their safe state.** Clear `plcc_image_q` and write it to the
   hardware *from inside the hook* — the scan that faulted never reaches the flush.
   (All-zero is the CODESYS default; a runtime may apply its own per-channel safe
   values.)
2. **Stop every task.** Nothing may call `plcc_run_task` again until the PLC is
   restarted. After the restart, call `plcc_init()` (cold) or restore RETAIN
   (warm) — the interrupted instance state is not trustworthy.
3. **Report** `code` and `where` (log, LED blink code, HMI).
4. **Not return.** Park, reset the MCU, or `longjmp` back to the scan loop. If the
   hook returns, the compiled code executes a trap instruction (HardFault on a
   Cortex-M).

A minimal bare-metal handler:

```c
#include "plc.h"

void plcc_fault(uint32_t code, const char *where) {
    memset(plcc_image_q, 0, PLCC_IMAGE_Q_SIZE);           /* 1. safe outputs */
    board_write_outputs(plcc_image_q, PLCC_IMAGE_Q_SIZE);
    plc_state = PLC_FAULTED;                               /* 2. no more tasks */
    log_printf("PLC fault %lu at %s\n", (unsigned long)code, where);   /* 3. */
    for (;;) board_blink_error_led(code);                  /* 4. never return */
}
```

A runtime that wants to keep serving (Modbus, a web UI) after a fault can
`longjmp` from the hook to a `setjmp` taken around its `plcc_run_task` calls
instead of parking, then refuse to run tasks until a restart.

`plcc_hal::scan::ScanCycle` does exactly this on a host: the JIT maps `plcc_fault`
onto `plcc_hal::fault::unwinding_fault_handler`, which unwinds back to
`Application::try_run_task`; `ScanCycle::step` then stops all tasks in the platform
scheduler, clears `%Q` and flushes it, reports a `DiagLevel::Fatal` /
`DiagCode::DivisionByZero` diagnostic through the platform's `DiagnosticSink`, and
returns `ScanError::Fault { task, fault }` — and keeps returning it until
`ScanCycle::start` restarts the program. `plcc sim` prints the fault and exits with
status 3.

A runtime that does not define `plcc_fault` still links: the module's weak default
traps, which on a PLC is a crash rather than a controlled stop — define the hook.

## Header and symbol table

`--emit-header prog.h` (C11 and C++11 — an Arduino sketch includes it directly):

- `PLCC_ABI_VERSION`, `PLCC_TARGET_TRIPLE`, `PLCC_POINTER_SIZE`, `PLCC_BIG_ENDIAN`
- `PLCC_TARGET_CPU` and `PLCC_TARGET_FEATURES` (`--cpu`, `--features`; `"generic"`
  and `""` by default), and with `--device` also `PLCC_DEVICE_ID` and
  `PLCC_DEVICE_MANIFEST_VERSION` (docs/device-manifest.md) — a runtime can
  `static_assert` that the program was built for it
- `PLCC_IMAGE_{I,Q,M}_SIZE` and the image, task, retain and app types above,
  plus prototypes for `plcc_init`, `plcc_run_task`, `plcc_get_app`
- `PLCC_TASK_COUNT`, and per task `PLCC_TASK_<NAME>` (index),
  `…_INTERVAL_NS`, `…_PRIORITY`
- `PLCC_RETAIN_COUNT`, `PLCC_RETAIN_SIZE`, `PLCC_RETAIN_SIGNATURE`
- `PLCC_FAULT_DIV_BY_ZERO` (and the reserved `PLCC_FAULT_*` codes,
  `PLCC_FAULT_USER_BASE`) and the `plcc_fault` prototype
- every AT binding as macros — `PLCC_AT_MAIN_START_AREA` (`'I'`), `…_OFFSET`
  (byte), `…_BIT` (0–7 or -1), `…_PTR` (`&plcc_image_i[0]`) — and as a table
  `plcc_at_bindings[]` of `{name, area, size, bit, byte_offset, byte_size, iec_type}`
- a typedef per PROGRAM (`plcc_prog_main_t`), FUNCTION_BLOCK (`plcc_fb_ton_t`) and
  STRUCT (`plcc_struct_recipe_t`) reachable from program state, each followed by
  `_Static_assert(sizeof …)` and `_Static_assert(offsetof …)` for every field
  against the layout LLVM computed **for the `--target` triple**; the per-program
  prototypes; `extern` declarations of every `plcc_inst_*` and `plcc_globals`.

If the header is used with a compiler or target it was not generated for, the
asserts fail at compile time instead of silently misreading state.

`--emit-symbols prog.json` has the same data plus a flattened list of every
program-instance and VAR_GLOBAL variable — `{path, symbol, offset, size, iec_type,
retain, bit}` — for HMI, OPC UA or Modbus mapping. `symbol` is the object the offset
is relative to (`plcc_inst_cpu_loop1`, `plcc_globals`, or an image area). Its
`fault` object lists the fault codes.

## A complete runtime in C

This is everything a bare-metal or Arduino runtime needs; nothing in it depends on
the program.

```c
#include <string.h>
#include "plc.h"          /* plcc compile plc.st -o plc.o --target thumbv7em-none-eabihf --emit-header plc.h */

int64_t plcc_monotonic_ns(void) { return board_micros64() * 1000; }
void plcc_print(const char *msg) { (void)msg; }

static volatile uint8_t faulted;
void plcc_fault(uint32_t code, const char *where) {    /* see "Runtime faults" */
    (void)code; (void)where;
    memset(plcc_image_q, 0, PLCC_IMAGE_Q_SIZE);
    board_write_outputs(plcc_image_q, PLCC_IMAGE_Q_SIZE);
    faulted = 1;
    for (;;) { }
}

static int64_t next_due[PLCC_TASK_COUNT];
static uint8_t prev_single[PLCC_TASK_COUNT];

void plc_setup(void) {
    plcc_init();
    if (retain_load_ok(PLCC_RETAIN_SIGNATURE))        /* warm start */
        for (uint32_t r = 0; r < PLCC_RETAIN_COUNT; r++)
            retain_read(r, plcc_retain_regions[r].data, plcc_retain_regions[r].size);
}

void plc_loop(void) {
    int64_t now = plcc_monotonic_ns();
    board_read_inputs(plcc_image_i, PLCC_IMAGE_I_SIZE);       /* latch %I */

    uint8_t due[PLCC_TASK_COUNT] = {0};
    for (uint32_t t = 0; t < PLCC_TASK_COUNT; t++) {
        const plcc_task_t *task = &plcc_tasks[t];
        uint8_t s = task->single ? task->single() : 0;
        if (task->single && s && !prev_single[t]) due[t] = 1;
        prev_single[t] = s;
        if (task->interval_ns > 0 && !s && now >= next_due[t]) {
            due[t] = 1;
            next_due[t] = (next_due[t] + task->interval_ns > now)
                        ? next_due[t] + task->interval_ns : now + task->interval_ns;
        }
        if (task->interval_ns == 0 && !task->single) due[t] = 1;
    }
    for (uint32_t prio_pass = 0; prio_pass < PLCC_TASK_COUNT; prio_pass++) {
        uint32_t best = PLCC_TASK_COUNT;       /* highest priority still due */
        for (uint32_t t = 0; t < PLCC_TASK_COUNT; t++)
            if (due[t] && (best == PLCC_TASK_COUNT || plcc_tasks[t].priority < plcc_tasks[best].priority))
                best = t;
        if (best == PLCC_TASK_COUNT) break;
        due[best] = 0;
        plcc_run_task(best);
    }

    board_write_outputs(plcc_image_q, PLCC_IMAGE_Q_SIZE);     /* flush %Q */
}
```

`board_read_inputs` / `board_write_outputs` are the only board-specific parts: on an
Arduino Opta, pack `digitalRead(I1..I8)` into `plcc_image_i[0]` bit by bit and
drive the relays from `plcc_image_q[0]`; which bit is which terminal is a
convention of that runtime, documented once, not per program. (With `PLCC_TASK_COUNT`
0 — a module with no PROGRAM — guard the arrays.)

In Rust, `plcc_hal::scan::ScanCycle` is the same loop over any `plcc_hal::Platform`.

## Implementation-defined choices

| Topic | IEC 61131-3 3rd ed. | plcc | Why |
|---|---|---|---|
| Unit of `%IW`, `%ID`, `%IL` numbers | implementation-defined | size-indexed (`%IW1` = byte 2) | CODESYS; keeps every access naturally aligned |
| `%IXn` (one number) | bit n | flat bit n (byte n/8, bit n%8) | the standard's reading |
| Byte order of multi-byte addresses | implementation-defined | target native | zero-cost access; matches CODESYS on the same CPU |
| Bit numbering | — | 0 = least significant | CODESYS |
| Image storage | implementation-defined | static arrays exported by the module | constant addresses, no heap |
| Image size | implementation-defined | highest address used, or `--image-size` | |
| AT in FUNCTION_BLOCK | full addresses discouraged, `*` intended | allowed, shared by all instances | CODESYS permits it |
| Partial `%I*` + VAR_CONFIG | defined | not yet supported (error) | |
| Program with no task | lowest priority | free-running `__background` task, priority `UINT32_MAX` | |
| No CONFIGURATION | not defined | one `MainTask`, cyclic T#20ms, priority 1 | CODESYS standard project |
| Priority numbering | 0 = highest | 0 = highest | |
| INTERVAL with SINGLE | periodic while SINGLE is FALSE | same (in `ScanCycle`) | |
| Overrun | implementation-defined | run once, skip missed activations | CODESYS default behaviour |
| Resource-scoped VAR_GLOBAL | visible in its resource | visible everywhere | one module per PLC |
| Task-level I/O images | implementation-defined | one image, latched/flushed per scan pass | single-core runtimes |
| Integer division / MOD by zero | error, handling implementation-defined | `plcc_fault(PLCC_FAULT_DIV_BY_ZERO, where)`; the runtime stops the PLC | CODESYS raises an exception and stops the task |
