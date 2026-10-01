# Running compiled programs

How a compiled module plugs into a runtime: the hardware abstraction layer and the program-independent runtime contract.

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
C runtime loop are in [docs/process-image.md](process-image.md). In Rust,
`plcc_hal::scan::ScanCycle` runs a compiled module on any `plcc_hal::Platform`:

```bash
cargo run --example linux_sim -p plcc-hal -- plc.st --input 0=1 --scans 10
```

