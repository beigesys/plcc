<!-- SPDX-License-Identifier: MPL-2.0 -->

# Runtime symbols a platform must supply

plcc compiles ST to a freestanding object file. Everything the generated code
needs from the outside world is imported as a plain C symbol. There are only three,
and the third (`plcc_fault`) has a weak default in the object.

Link them in, and a plcc-compiled object runs on bare metal with no Rust runtime,
no allocator and no OS.

This page is about what the object *imports*. What it *exports* for the runtime
to drive it — the `%I`/`%Q`/`%M` process image, the task table, `plcc_init()` /
`plcc_run_task()`, the RETAIN regions and the `plcc_app` descriptor — is the
runtime contract in [process-image.md](process-image.md).

## `plcc_monotonic_ns`

```c
int64_t plcc_monotonic_ns(void);
```

The one time source. ST has no way to ask for the time, so every timer — the
`MONOTONIC_NS()` builtin, and the bundled ST standard library's `TON`, `TOF` and
`TP` underneath it — bottoms out here.

Contract:

* **Nanoseconds since an arbitrary but fixed epoch.** Only differences are
  meaningful. Booting the counter at zero is fine.
* **Monotonic non-decreasing.** It must never run backwards and must not wrap
  within a session. A timer that sees a negative delta will misbehave.
* **Signed 64-bit.** `TIME`, `LTIME` and `LINT` are all laid out as `i64`
  nanoseconds by codegen, and every arithmetic and comparison path treats them as
  signed. `i64` nanoseconds still spans about 292 years of uptime.
* **Cheap and callable from the scan context.** Timers call it every scan. It must
  not block, allocate, or take a lock that a higher-priority context might hold.

Typical implementations:

| Platform | Implementation |
| --- | --- |
| Linux / POSIX host | `clock_gettime(CLOCK_MONOTONIC)` |
| Cortex-M | a free-running 32-bit timer plus a SysTick rollover counter |
| Any host build | `plcc_runtime::host_clock::plcc_monotonic_ns` (already exported) |

`plcc sim` and the JIT map this onto `plcc-runtime`'s host clock, which is an
`std::time::Instant` captured once at process start.

## `plcc_print`

```c
void plcc_print(const char *msg);
```

Backs the `PRINT` statement. `msg` is a NUL-terminated string. Writing it to a
debug UART, a log ring buffer, or `stderr` are all reasonable. A no-op
implementation is acceptable if the target has nowhere to print.

`plcc sim` maps this onto a host implementation that writes `[PLC] <msg>` to
stderr.

## `plcc_fault`

```c
#define PLCC_FAULT_DIV_BY_ZERO    1u
#define PLCC_FAULT_ARRAY_BOUNDS   2u   /* reserved */
#define PLCC_FAULT_NULL_REFERENCE 3u   /* reserved */
#define PLCC_FAULT_USER_BASE 0x10000u  /* first code free for the runtime */

void plcc_fault(uint32_t code, const char *where);
```

Called when an ST operation cannot continue — today, an integer `/` or `MOD`
(also `DIV()`, `DIV_TIME`, TIME / integer) whose divisor is zero. CODESYS raises
an exception there and stops the task; plcc hands the same decision to the
runtime.

Contract:

* **Must not return.** If it does, the compiled code executes `llvm.trap`
  (`ud2` on x86, `udf` → HardFault on Cortex-M, `unreachable` on wasm). The call
  is not declared `noreturn`, so a returning hook is always caught by the trap.
* `code` is one of `PLCC_FAULT_*` (the header `--emit-header` writes has them;
  `plcc_runtime::fault::FaultCode` in Rust). Only `PLCC_FAULT_DIV_BY_ZERO` is
  raised today; the reserved codes are for an array-bounds check in a strict mode
  and for calls through an unbound interface reference / NULL pointer. Codes from
  `PLCC_FAULT_USER_BASE` up are never emitted by plcc.
* `where` is a static NUL-terminated string: `"<file>:<line>:<col>: <POU>"`
  (`plc.st:42:13: Mixer`, `FB_Valve.Open` for a method) when the compiler was given
  the source files — `plcc compile`, `plcc sim` and `plcc_hal::jit` always are —
  otherwise just the POU name. It lives in the object's read-only data; keep the
  pointer, no need to copy.
* Called from the scan context, in the middle of `plcc_run_task` (or
  `plcc_init`, for a zero divisor in an initial value).

What the runtime should do — clear `%Q`, stop the tasks, report, never return — and
a minimal handler are in [process-image.md](process-image.md#runtime-faults).

**Weak default.** Every module that contains a check also *defines* `plcc_fault`
with weak linkage (ELF `STB_WEAK`), with a body that just traps. So:

* a runtime that does not know about faults still links, on any target;
* a runtime's own (strong) definition replaces the default at link time — no
  flag needed. Put it in an object file that is linked directly: a linker does not
  pull an archive (`.a`) member to replace a symbol that is already defined, weak
  or not, so a handler inside a static library needs `--whole-archive` or an
  explicit reference;
* a host that JIT-compiles the module drops the default body first
  (`Compiler::use_external_fault_handler`) and maps the symbol to its handler.

Division by a constant non-zero divisor (`x / 10`, `x MOD 16#100`) has no check
and no call. `MIN / -1` (e.g. `INT#-32768 / -1`) does not fault: it wraps to
`MIN` (and `MIN MOD -1` is 0), as CODESYS gives on ARM targets. REAL/LREAL
division by zero does not fault either: it is IEEE 754 (±INF, NaN), as in CODESYS.

`plcc sim` prints `[PLC] FAULT <code> (<what>) at <where>: program stopped` and
exits with status 3. `plcc_hal` unwinds to `ScanCycle`, which stops the PLC (see
process-image.md).

## Everything else

Math is emitted as LLVM intrinsics (`llvm.sqrt`, `llvm.sin`, …). On most targets
LLVM lowers those to inline instructions; on targets without hardware support it
lowers them to libm calls (`sqrtf`, `sinf`, …), so a freestanding build that uses
`SQRT`/`SIN`/`COS`/`EXP`/`LN` needs a libm — `compiler-rt` or `newlib` both work.

Nothing else is imported (`plcc_fault` is imported only in the sense that a
runtime may replace it). In particular the process image is *defined* by the
object (`plcc_image_i/q/m`), not supplied by the runtime, and there is no
allocator call anywhere: program instances are static.

The standard function blocks are **not** runtime symbols. They are compiled from
bundled ST source into the user's module (see `--stdlib`), so they inline and
optimize alongside user code and need no ABI bridge.
