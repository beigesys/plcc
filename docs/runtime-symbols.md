<!-- SPDX-License-Identifier: MPL-2.0 -->

# Runtime symbols a platform must supply

plcc compiles ST to a freestanding object file. Everything the generated code
needs from the outside world is imported as a plain C symbol. There are only two.

Link them in, and a plcc-compiled object runs on bare metal with no Rust runtime,
no allocator and no OS.

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

## Everything else

Math is emitted as LLVM intrinsics (`llvm.sqrt`, `llvm.sin`, …). On most targets
LLVM lowers those to inline instructions; on targets without hardware support it
lowers them to libm calls (`sqrtf`, `sinf`, …), so a freestanding build that uses
`SQRT`/`SIN`/`COS`/`EXP`/`LN` needs a libm — `compiler-rt` or `newlib` both work.

The standard function blocks are **not** runtime symbols. They are compiled from
bundled ST source into the user's module (see `--stdlib`), so they inline and
optimize alongside user code and need no ABI bridge.
