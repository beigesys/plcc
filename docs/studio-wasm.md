<!-- SPDX-License-Identifier: MPL-2.0 -->

# plcc studio in the browser: what runs where

plcc studio (the ladder/ST editor in `studio/`) runs entirely in the browser.
There is no build server: projects live in the browser's origin-private file
system, the simulator runs in a Web Worker, and a board is flashed over WebUSB.
This page describes the browser-side building blocks, what each costs, the one
piece that is not in the browser yet (code generation), and the plan for it.

```
 browser tab                                     Web Worker(s)
 ┌──────────────────────────────┐   postMessage   ┌──────────────────────────────┐
 │ editor (studio/)             │ ──────────────▶ │ @plcc/plcc-wasm              │
 │  - diagnostics, tag browser  │ ◀────────────── │  parse + check, tag outline  │
 │  - I/O view, online mode     │                 │  convert, ladder catalog     │
 │                              │                 ├──────────────────────────────┤
 │                              │ ──────────────▶ │ @plcc/plc-wasm (worker.ts)   │
 │                              │ ◀── snapshots ─ │  runs the program's .wasm,   │
 │                              │                 │  scan scheduler, %I/%Q/%M    │
 │ @plcc/webdfu ── WebUSB ──────┼──▶ Opta DFU     └──────────────────────────────┘
 │ Web Serial (1200-baud touch, │
 │  USB console, online mode)   │     code generation: see "The compile gap"
 └──────────────────────────────┘
```

| Piece | Where | Size | Status |
|---|---|---|---|
| Front end: ST / PLCopen XML / L5X / TwinCAT → diagnostics, tag outline; `plcc convert` (ST, PLCopen LD, L5X RLL, ladder JSON, IEC↔Logix); ladder instruction catalog | `crates/plcc-driver`, `crates/plcc-wasm`, `packages/plcc-wasm` | 1.88 MB wasm, 658 KB gzip, 491 KB brotli | done, tested in Node |
| Simulator runtime for plcc's wasm32 output | `packages/plc-wasm` | ~20 KB TS; programs 3-4 KB each | done, tested in Node |
| Flashing (WebUSB DFU 1.1 + DfuSe), 1200-baud touch | `packages/webdfu` | ~15 KB TS | done, tested against a fake device; not yet run against hardware from a browser |
| Code generation (ST → wasm32 for the simulator, → Thumb for the Opta) | `crates/plcc-codegen` (LLVM) | 31.7 MB wasm, 6.8 MB brotli (spike) | **spike works**: LLVM in wasm, output byte-identical to native |
| Linking for the Opta | Arduino CLI + arm-gcc today | 140 KB (spike) | **spike works**: single-object linker, byte-identical to lld; needs a firmware that loads program images |

## `@plcc/plcc-wasm` — the front end

`crates/plcc-driver` is the `plcc check` pipeline over a map of `path → text`:
input dispatch (`.st`, PLCopen `.xml`, `.L5X` with an optional I/O map, TwinCAT
`.TcPOU/.TcDUT/.TcGVL/.TcIO/.TcTTO` and `.plcproj` projects resolved against the
in-memory files, case-insensitively), the Logix prelude, the bundled standard
library, the type checker. It has no file system access and no LLVM; the
front-end crates (`plcc-st`, `plcc-hir`, `plcc-plcopen`, `plcc-l5x`,
`plcc-twincat`, `plcc-stdlib`, `plcc-runtime`) all build for
`wasm32-unknown-unknown` unchanged (LLVM is only a dev-dependency of the readers'
tests). `crates/plcc-wasm` wraps it with wasm-bindgen; `packages/plcc-wasm`
builds it (`npm run build` → `build.sh`: `cargo build --profile wasm-release`,
`wasm-bindgen --target web`, `wasm-opt -Oz`) and adds typed wrappers.

```ts
import { load, check } from "@plcc/plcc-wasm";
await load();                       // fetches pkg/plcc_wasm_bg.wasm next to the glue
const r = await check({
  files: { "src/main.st": text, "io.L5X": l5x },
  entry: ["src/main.st"],           // optional; default: every .plcproj, else every source file
  io_map: toml,                     // optional, L5X I/O map (docs/l5x.md)
  stdlib: "bundled-st",             // or "none"
});
```

```json
{
  "ok": false,
  "declarations": 1,
  "diagnostics": [{
    "file": "src/main.st", "severity": "error", "stage": "typecheck", "code": null,
    "message": "type mismatch: ...", "help": null,
    "span":   { "start": { "line": 3, "col": 11, "offset": 60, "utf16": 60 },
                "end":   { "line": 3, "col": 16, "offset": 65, "utf16": 65 }, "message": "..." },
    "labels": [ ... ]
  }],
  "tags": {
    "image":    [{ "path": "Main.start", "address": "%IX0.0", "area": "I", "size_prefix": "X",
                   "byte_offset": 0, "bit": 0, "bits": 1, "iec_type": "BOOL", "file": "src/main.st" }],
    "programs": [{ "name": "Main", "kind": "program", "variables": [{ "name": "start", "kind": "var",
                   "iec_type": "BOOL", "retain": false, "constant": false, "at": "%IX0.0" }] }],
    "function_blocks": [], "globals": [],
    "tasks":    [{ "name": "Fast", "interval_ns": 10000000, "priority": 1, "single": null,
                   "instances": [{ "name": "Cpu.Main", "program": "Main" }], "implicit": false }]
  }
}
```

- `line`/`col` are 1-based, `col` in UTF-16 code units; `utf16` is a JavaScript
  string index (what CodeMirror/Monaco want); `offset` is the UTF-8 byte offset.
- `stage` says who reported it: `parse`, `plcopen`, `l5x`, `twincat`, `io-map`,
  `input` (bad path, missing entry), `typecheck`, `convert` (dialect translation
  and write warnings, ST-to-ladder notes).
- Paths must be relative and `/`-separated; `..`, `.`, empty components,
  absolute paths, backslashes and NUL are rejected (`validatePath`).
- The tag outline is source-level. `%I/%Q/%M` locations are exact (they follow
  from the address alone). Offsets of ordinary variables in program state depend
  on code generation: they come from the compiled module's symbol table
  (`--emit-symbols`), which `@plcc/plc-wasm` uses.
Conversion and the ladder palette (`plcc convert`, `plcc_ladder::catalog::all`):

```ts
const r = await convert({ files: { "p.st": st }, to: "ladder-json" });   // or "st" | "plcopen" | "l5x"
// { ok, output, diagnostics }; dialect?: "iec" | "logix" translates ladder-json/st output,
// prelude?: true appends the Logix prelude to ST converted from L5X.
const palette = await catalog("logix");   // [{ name: "XIC", category: "bit", role: "input", pins: [...] }, ...]
```

Ladder outputs take one input file (a `.json` input is a ladder model; `.st`
is converted, its drawable statements as rungs); translation warnings come back
as `stage: "convert"` warnings, reader errors keep their spans.

- A Rust panic aborts the instance (`panic = "abort"`) and wasm-bindgen keeps one
  instance per realm: run the package in a worker and restart the worker when
  `poisonedReason()` is non-null.

## `@plcc/plc-wasm` — the simulator runtime

Runs a program compiled with `plcc compile --target wasm32-unknown-unknown` and
linked with

```bash
wasm-ld --no-entry --export-dynamic --allow-undefined --export-table -o prog.wasm prog.o
```

(every symbol exported — the runtime contract and the bases of the tag offsets;
runtime symbols imported from `env`; the function table exported because a
SINGLE task trigger is a function pointer). `test/fixtures/build.sh` is the
reference.

What a module imports (inspected from real modules; nothing else occurs):

| Import | Supplied as |
|---|---|
| `env.plcc_monotonic_ns() -> i64` | `BigInt` from the caller's `Clock` (`FakeClock` for tests and single-stepping, `RealClock` = `performance.now()`) |
| `env.plcc_print(ptr)` | `onPrint(message)` |
| `env.plcc_fault(code, ptr)` | throws `PlcFault { code, where }`; the scan unwinds. On wasm the module *imports* it (codegen drops the weak trapping default for wasm targets). |
| libm: `sinf`, `cos`, `powf`, `fmod`, `roundf`, … | `Math.*` (f32 variants rounded with `Math.fround`, C `round`/`fmod` semantics) |

Anything else is refused at load time, naming the import.

What it exports and the runner uses: `memory`, `plcc_init`, `plcc_run_task`,
`plcc_get_app` (→ ABI version, task table, process image), the
`plcc_image_{i,q,m}` / `plcc_inst_*` / `plcc_globals` address globals,
`__indirect_function_table`.

```ts
import { PlcModule, ScanCycle, Runner, FakeClock } from "@plcc/plc-wasm";
const clock = new FakeClock();
const plc = await PlcModule.load(wasmBytes, { clock, symbols, onPrint: console.log });
plc.tasks;                          // from the module: name, intervalNs, priority, SINGLE, programs
plc.writeBit("I", 0, 0, true);      // %IX0.0
plc.write("Main.setpoint", 42);     // by symbol-table path
const cycle = new ScanCycle(plc, clock, { onFault: (f) => ... });
cycle.start(); cycle.simulate(clock, 500, 10);   // 500 ms in 10 ms steps
plc.read("Main.t.ET");              // 64-bit and TIME values are bigint
new Runner(cycle).start();          // real time, setTimeout-driven (works in a worker)
```

- `ScanCycle` is `plcc_hal::scan::ScanCycle` / the Arduino runtime loop:
  intervals, SINGLE edges, priorities, overrun skipping, latch/flush hooks.
- A fault (`plcc_fault`, or any wasm trap) clears `%Q`, stops every task and is
  reported with its code and site (`div_zero.st:11:6: Ratio`); `restart()`
  re-instantiates the module (fresh memory, `plcc_init`) — the faulted instance's
  state and stack pointer are not trusted.
- `worker.ts` (`serveWorker(self)`) is a message protocol for running it all in
  a Web Worker: `load`, `start`, `stop`, `restart`, `step` (with `advanceMs` on the
  fake clock), `writeImage`, `writeBit`, `writeTag`, `readTags`, `snapshot`; it
  posts throttled `scan` snapshots of `%I/%Q/%M`, `print` and `fault` events.

## `@plcc/webdfu` — flashing the Opta

WebUSB DFU 1.1 with ST's DfuSe extensions, written from the USB DFU 1.1 class
specification and ST AN3156 (devanlai/webdfu, ISC, was checked; it is an untyped
example without the safety rules, so nothing was vendored).

```ts
import { OPTA, DfuseDevice, touch1200, waitForDfuDevice } from "@plcc/webdfu";
// 1. Reboot the running board into its bootloader (Web Serial).
const [port] = await navigator.serial.getPorts();   // or requestPort({ filters: OPTA.runtimeFilters })
await touch1200(port);
// 2. The bootloader enumerates as 2341:0364; grant it once with
//    navigator.usb.requestDevice({ filters: OPTA.dfuFilters }).
const usb = await waitForDfuDevice(navigator.usb, OPTA);
const dev = await DfuseDevice.open(usb, { profile: OPTA });
await dev.flash(firmwareBin, { onProgress: ({ phase, done, total }) => ... });  // erase, write, leave
```

The Opta values were read from the installed Arduino core (mbed_opta 4.6.0), not
assumed: `boards.txt` `opta.upload.tool=dfu-util`, `upload.vid=0x2341`,
`upload.pid=0x0364`, `upload.interface=0`, `upload.address=0x08040000`,
`upload.use_1200bps_touch=true`; `platform.txt` runs
`dfu-util --device 2341:0364 -D <bin> -a0 --dfuse-address=0x08040000:leave`; the
bootloader's alt 0 name is `@Internal Flash  2MB   /0x08000000/01*128Ka,15*128Kg`.

**Safety — the bootloader cannot be written from this code:**

- The alternate setting's DfuSe layout string is read (WebUSB `interfaceName`, or
  the string descriptor via the configuration descriptor) and parsed; without it,
  nothing is written.
- Before anything is sent, a write is refused if it touches a sector the layout
  marks read-only (`a`) or not erasable / not writable, lies outside the layout,
  starts below the profile's `minAddress` (0x08040000 for the Opta: sector 0 is
  `a`, sector 1 is writable in the layout but holds the rest of the bootloader),
  or runs past the profile's application area (0x08040000-0x081FFFFF; the
  `boards.txt` maximum of 1966080 bytes would run 128 KiB past the end of flash
  from that address, so the profile uses the real 1.75 MiB).
- Every erase command and every data chunk is re-checked right before it is sent.
- No mass erase exists in the code. `leave()` refuses a start address below
  `minAddress`.
- `DfuseDevice.open` refuses a device whose VID:PID, alternate setting or layout
  name the profile does not describe.

The 1200-baud touch is implemented by the Arduino core the runtime is built with
(`cores/arduino/USB/USBSerial.cpp`: a 1200-baud line coding followed by DTR low
for 200 ms calls `_ontouch1200bps_()`, which sets an RTC backup-register magic
and resets into the bootloader). The plcc Opta runtime calls `Serial.begin()`,
so a running program can always be touched; a board stuck in a crash loop can
still be put in DFU mode with a double press of its reset button.

## The compile gap

Everything above runs in the browser. Two steps still need native tools:

1. **Code generation.** `plcc-codegen` is LLVM (inkwell → the LLVM C API). ST →
   wasm32 for the simulator and ST → Thumb-2 for the Opta both go through it.
2. **Linking.** For the simulator, the relocatable wasm object must become a
   module (`wasm-ld` today). For the Opta, the object is linked into the whole
   Arduino firmware (mbed OS core, ArduinoModbus, …) with arm-none-eabi-gcc via
   `arduino-cli` — hundreds of megabytes of toolchain and several minutes per
   build, not something to run in a tab.

The feasibility spike below looked at both.

### (a) LLVM compiled to WebAssembly — works

`spikes/browser-codegen/` holds the scripts. The result, in about three hours:

1. **LLVM 21.1.8 libraries for `wasm32-wasip1`**, ARM and WebAssembly backends
   only, built with wasi-sdk 34 (`llvm-wasi/build.sh`). Upstream LLVM 21 does not
   configure for a WASI host ("Unable to determine platform"); YoWASP's single
   WASI patch (the one behind `@yowasp/clang`, Apache-2.0 WITH LLVM-exception)
   applies to 21.1.8 with five hunks redone by hand plus two small fixes
   (`llvm-21.1.8-wasi.patch`, 18 files, all in `Support` and CMake). Threads
   off, no zlib/zstd/libxml2, MinSizeRel. 71 static libraries, 110 MB of
   archives; ~57 minutes on 3 cores, a one-time CI artifact.
2. **plcc-codegen + inkwell + plcc-driver linked against them** as a WASI
   program (`webcc/`): llvm-sys needs only a stand-in `llvm-config` script, an
   empty `libffi.a`, wasi-sdk's `libc++abi` and the WASI emulation libraries. No
   source change in plcc (the program calls `Compiler` exactly as the CLI does).
3. **It runs, and its output is byte-identical to native plcc.** Under Node's
   WASI, for `chaser_io.st`, `water_treatment.st`, `batch_process.st` and
   `pid_simple.st`: every `thumbv7em-none-eabi` object has the same `.text`,
   `.rodata` and relocations as `plcc compile -O2`, the symbols JSON is identical,
   and every `wasm32-unknown-unknown` object is identical byte for byte.
   Linking the web-built Opta object with the spike linker gives the same image
   as linking the native one.

| | |
|---|---|
| `webcc.wasm` (release, LTO, `opt-level = "s"`, stripped) | 31.7 MB; 10.1 MB gzip; **6.8 MB brotli** |
| after `wasm-opt -Oz` | 24.9 MB; 9.2 MB gzip; 6.7 MB brotli (and ~40 % slower) |
| module compile (V8, Node 22) | ~90 ms (lazy tier-up) |
| parse + check + codegen + `-O2` + emit, one program | 0.4-0.6 s |

A 6.8 MB download, cached by the service worker once, is acceptable for an IDE
(VS Code for the Web and StackBlitz ship more). It is what makes the simulator
and the Opta build share one compiler with the native CLI — no second backend,
no drift.

To productize (**~1-2 weeks**): a reactor-style entry point (`compile(request)
→ { object, symbols, diagnostics }` instead of `main` and files) or a small WASI
shim over in-memory files in the worker (`@bjorn3/browser_wasi_shim`, MIT/Apache,
or 150 lines of our own: the program only reads the source and writes the
outputs); CI that builds and caches the LLVM libraries; codegen errors as
structured diagnostics; memory limits (`--max-memory` is 1 GiB now); and a size
pass (drop MCJIT/Interpreter from the link, `-Oz`, fewer passes).

### (b) A second backend for the simulator only

Cranelift has no 32-bit ARM backend, so it could only ever serve the simulator —
and it does not emit WebAssembly either, so "Cranelift in the browser" would mean
running Cranelift-compiled *native* code, which a browser cannot. The real
option is a direct ST → WebAssembly backend with `wasm-encoder`
(Apache-2.0 WITH LLVM-exception / MIT), from the same AST/HIR the LLVM backend
uses.

- Size: `plcc-codegen` is ~15 000 lines of lowering (expressions and implicit
  conversions for every IEC type, strings and their functions, TIME/DATE
  arithmetic, FB instances and init, OOP dispatch, references and pointers, the
  runtime contract, faults, …). A wasm backend would re-implement nearly all of
  it: **6-10 weeks** to parity, then two backends to keep in lockstep forever.
- Risk: the simulator would no longer run the code the PLC runs. Every semantic
  divergence (integer wrap, division, REAL rounding, string truncation) becomes a
  "works in the simulator, not on the PLC" bug — the worst kind for this product.
- Output would be a finished module (no linker needed), and the backend would be
  small in the browser (~1 MB).

Not recommended while (a) is viable.

### (c) Other paths

- **An interpreter** of the HIR in Rust → wasm, for the simulator: the same
  duplication and divergence problem as (b), slower at run time; no.
- **lld in the browser** for the simulator's link step: YoWASP ships LLVM/Clang/LLD
  for WASI (`@yowasp/clang`, 105 MB unpacked) — usable, but far larger than the
  problem. plcc's wasm objects are as simple as its ARM ones: across the fixture
  programs the only relocations are `R_WASM_FUNCTION_INDEX_LEB`,
  `R_WASM_MEMORY_ADDR_{LEB,SLEB,I32}` and `R_WASM_TABLE_INDEX_I32` (plus
  `GLOBAL_INDEX_LEB` / `TYPE_INDEX_LEB` at `-O0`). A single-object wasm linker
  (lay out data segments, place the stack, build the table and exports, patch
  those relocations) with `wasmparser` + `wasm-encoder` is **~3-4 days**.
- **Emit a finished module from codegen**: LLVM cannot; its wasm backend only
  writes relocatable objects.
- **Keep a native compile service** (the original `plcc serve` design): ruled out
  by the browser-only requirement, but it remains the fallback if (a) proves too
  heavy for low-end machines.

### Linking for the Opta without the Arduino toolchain: a program image

Today every download relinks the whole firmware. Real PLC runtimes do not: the
runtime (firmware) is installed once, and a download replaces only the
application, which the runtime loads from a fixed place. plcc's runtime contract
already has the shape for this — `plcc_app` describes the program completely
(tasks, image, init, run, retain) — so the firmware can drive a program it was
not linked with. What plcc would produce is a position-fixed **program image**:
the object's code and data placed at addresses reserved for programs, with the
runtime's services reached through a table the firmware publishes.

**Flash and RAM.** On the Opta, reserve the last 4 sectors of the M7 flash
(0x08180000-0x081FFFFF, 512 KiB) as the program slot, and a RAM window for the
program's `.data`/`.bss` (e.g. 64 KiB at a fixed AXI-SRAM address the firmware's
linker script leaves out). The firmware keeps 1.25 MiB (the runtime is ~165 KiB
today). DfuSe can write the slot alone: the flasher's profile for "program" gets
`minAddress = 0x08180000`, so a program download cannot touch the firmware, let
alone the bootloader — and it erases 1-4 sectors instead of 14.

**Image layout** (little-endian, at the slot base):

```c
#define PLCC_IMAGE_MAGIC 0x42434C50u        /* "PLCB" */

typedef struct plcc_image_header {
    uint32_t magic;            /* PLCC_IMAGE_MAGIC */
    uint16_t abi_version;      /* PLCC_ABI_VERSION of plcc_app */
    uint16_t header_size;      /* sizeof(plcc_image_header_t) */
    uint32_t image_size;       /* header + text + data load image */
    uint32_t image_crc32;      /* CRC-32 of bytes [header_size, image_size) */
    uint32_t runtime_api;      /* version of the runtime API table the image was linked against */
    uint32_t target;           /* board/CPU/float-ABI id ('OPTA', cortex-m7, softfp) */
    uint32_t text_addr;        /* where text+rodata execute (in place): slot base + header_size */
    uint32_t text_size;
    uint32_t data_load;        /* offset of the .data initial image in the slot */
    uint32_t data_addr;        /* RAM address .data is copied to */
    uint32_t data_size;
    uint32_t bss_addr;         /* RAM address zeroed by the loader */
    uint32_t bss_size;
    uint32_t api_slot;         /* RAM address of `const plcc_runtime_api_t *` the loader fills */
    uint32_t get_app;          /* Thumb address of plcc_get_app() */
    uint8_t  build_id[16];     /* hash of the sources: "is the program in the PLC the one open in the editor?" */
    uint32_t header_crc32;     /* CRC-32 of the header up to here */
} plcc_image_header_t;
```

**Runtime services** are called through one table whose address the loader
stores in `api_slot`:

```c
typedef struct plcc_runtime_api {
    uint32_t version, count;
    int64_t (*monotonic_ns)(void);
    void    (*print)(const char *);
    void    (*fault)(uint32_t code, const char *where);
    void   *(*memcpy)(void *, const void *, size_t);
    void   *(*memset)(void *, int, size_t);
    /* compiler helpers the Cortex-M code calls: __aeabi_ldivmod, __aeabi_uldivmod,
       __aeabi_f2lz, ... and, while plcc emits soft-float, __addsf3, __mulsf3, ...;
       libm: sinf, cosf, powf, ... — appended, never reordered */
} plcc_runtime_api_t;
```

The image linker turns each call to an undefined symbol into a 16-byte veneer
(`movw/movt ip, api_slot; ldr ip, [ip]; ldr pc, [ip, #4*k]` — `ip`/r12 is the
AAPCS intra-procedure scratch register veneers may use). The firmware's loader:
check magic, CRCs, `abi_version`, `runtime_api` ≤ its own, `target`; copy
`.data`, zero `.bss`, store the API pointer, call `get_app()->init()`, then run
`get_app()->tasks` exactly as it runs the linked-in table today. A missing or
bad image leaves the runtime up (USB console, Modbus) with no program — the
"no application" state every PLC has.

Codegen changes needed: for an image build, keep `plcc_fault` external (as for
wasm), and drop `.ARM.exidx` (mark functions `nounwind` without unwind tables),
which removes the `__aeabi_unwind_cpp_pr0` reference. Worth doing regardless:
compile for the actual CPU (`cortex-m7`, `+fp-armv8d16sp`, softfp ABI to match
the Arduino core) — today plcc targets a generic ARMv7E-M without an FPU, so
every REAL operation is a library call (`__addsf3`, `__mulsf3`, ...).

### The image linker is small: a spike that matches lld byte for byte

The objects plcc emits for Cortex-M are simple. Across all 19 programs in
`tests/fixtures/{programs,codegen}` plus `chaser_io.st`, at `-O0` and `-O2`, the
only relocation types are `R_ARM_ABS32`, `R_ARM_THM_CALL`, `R_ARM_THM_JUMP24`,
`R_ARM_THM_MOVW_ABS_NC`, `R_ARM_THM_MOVT_ABS`, `R_ARM_NONE` and `R_ARM_PREL31`
(the last only in `.ARM.exidx`, which an image drops); sections are `.text`,
`.rodata`, `.rodata.str1.1`, `.bss` (`.data` appears with initialized globals).
Undefined symbols: `plcc_monotonic_ns`, `plcc_print`, `__aeabi_unwind_cpp_pr0`,
soft-float helpers and libm.

A ~250-line std-only Rust prototype (`spikes/browser-codegen/minild`) parses the
ELF32 object, lays text/rodata out at a fixed flash address and data/bss at a
fixed RAM address, resolves imports to given addresses and applies those
relocations. For all 38 objects its flat image is **byte-identical** to
`ld.lld -O0` with an equivalent linker script. It builds for `wasm32-wasip1`
(140 KB) and gives the same output under Node's WASI. A production version —
the header, CRCs, veneers, `.data`/COMMON handling, defensive support for
`R_ARM_THM_JUMP19/JUMP11/PC8/REL32`, errors instead of panics, tests against
`ld.lld` in CI and on the Unicorn/QEMU emulator — is roughly **3-5 days**. The
firmware side (reserved slot and RAM window in the linker script, loader, API
table, "no program" state, program-slot DFU profile) is another **3-5 days**,
plus hardware bring-up.

## Recommendation

1. **Ship option (a).** Build the LLVM-for-WASI libraries in CI (cached), turn
   the `webcc` spike into `crates/plcc-web` (reactor API, JSON request/response
   like `plcc-wasm`, same structured diagnostics), and run it in a worker next to
   `@plcc/plcc-wasm`. The front-end module stays separate and small (0.5 MB) so
   typing never waits for the 6.8 MB compiler; the compiler loads on first
   Simulate/Download. ~1-2 weeks.
2. **Simulator:** a single-object wasm linker in Rust (`wasmparser` +
   `wasm-encoder`, ~3-4 days) turns the compiler's wasm32 object into the module
   `@plcc/plc-wasm` already runs. (Until then the studio could fall back to
   plcc's native `plcc compile` + `wasm-ld` for development.)
3. **Opta:** adopt the program-image model — firmware (the plcc runtime) flashed
   once, programs downloaded into a fixed 512 KiB slot through the same WebUSB
   DFU path with a `minAddress` of the slot, linked by the image linker
   (~3-5 days) against a runtime API table; firmware loader and linker script
   ~3-5 days plus bring-up. This also makes downloads seconds instead of an
   Arduino build, and gives the "which program is in the PLC" build id online
   mode needs.
4. Not (b): a second backend would double the compiler and let the simulator
   drift from the PLC.
5. While there: compile Cortex-M code for the real CPU (cortex-m7 + FPU,
   softfp) instead of a generic ARMv7E-M — REAL math is library calls today.

## Development

```bash
# front end → packages/plcc-wasm/pkg (needs wasm-bindgen-cli 0.2.129, optional wasm-opt)
cd packages/plcc-wasm && npm install && npm run build && npm test
cd packages/plc-wasm  && npm install && npm test      # fixtures: npm run fixtures (plcc + wasm-ld)
cd packages/webdfu    && npm install && npm test
```

All three packages are plain TypeScript ES modules with no framework and no
runtime dependencies; `main`/`types` point at `src/index.ts` for a Vite build to
consume directly.
