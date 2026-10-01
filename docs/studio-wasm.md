<!-- SPDX-License-Identifier: MPL-2.0 -->

# plcc studio in the browser: what runs where

plcc studio (the ladder/ST editor in `studio/`) runs entirely in the browser.
There is no server of any kind: projects live in the browser's origin-private
file system, plcc itself — front end, LLVM code generation, linkers — runs as
WebAssembly in Web Workers, the simulator runs plcc's own wasm32 build of the
project, and a board is flashed over WebUSB. This page describes the pieces,
what each costs, how they are built and shipped, and what is left.

```
 browser tab                                         Web Workers
 ┌────────────────────────────────┐  postMessage  ┌──────────────────────────────────┐
 │ editor (studio/)               │ ────────────▶ │ front end: @plcc/plcc-wasm       │
 │  - model: plcc-ladder JSON     │ ◀──────────── │  check (diagnostics on rungs),   │
 │  - diagnostics, Problems,      │               │  convert (ST view, import,       │
 │    ST view, import / export    │               │  export), device manifests       │
 │                                │               ├──────────────────────────────────┤
 │                                │ ────────────▶ │ compiler: @plcc/plcc-compiler-   │
 │                                │ ◀── objects ─ │  wasm (plcc-build + LLVM + wasm  │
 │                                │               │  linker), loaded on first use    │
 │                                │               ├──────────────────────────────────┤
 │                                │ ────────────▶ │ simulator: @plcc/plc-wasm runs   │
 │                                │ ◀─ snapshots  │  the wasm32 build (TS preview    │
 │                                │               │  engine until it is ready)       │
 │ @plcc/plc-image: program image │               └──────────────────────────────────┘
 │ @plcc/webdfu ── WebUSB ────────┼──▶ Opta DFU, program slot only
 │ WebSerial: console (Online),   │
 │  1200-baud touch               │
 └────────────────────────────────┘
```

| Piece | Where | Download | Status |
|---|---|---|---|
| Front end: ST / PLCopen XML / L5X / TwinCAT / ladder model → diagnostics (placed on rungs and elements for a ladder model), tag outline; `plcc convert`; the ladder catalog; device manifests | `crates/plcc-driver`, `crates/plcc-device`, `crates/plcc-wasm`, `packages/plcc-wasm` | 2.2 MB wasm, 765 KB gzip | done; the studio loads it when it opens |
| Compiler: the front end, LLVM 21 code generation (ARM and WebAssembly backends) and a single-object wasm linker | `crates/plcc-build`, `crates/plcc-wasm-link`, `packages/plcc-compiler-wasm` | 32.3 MB wasm, **10.2 MB gzip**, fetched on the first Simulate or Download, then cached | done; same object as `plcc compile` (tested) |
| Simulator runtime for plcc's wasm32 output | `packages/plc-wasm`, `studio/src/runtime/plcHost.ts` | ~20 KB TS; programs a few KB | done |
| Program images for the Opta's program slot | `crates/plcc-image`, `packages/plc-image` | 88 KB wasm, 38 KB gzip | done; the loader runtime's first hardware flash is pending |
| Flashing (WebUSB DFU 1.1 + DfuSe), 1200-baud touch | `packages/webdfu` | ~15 KB TS | done, tested against a fake bootloader; not yet run from a browser against hardware |

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

Device manifests (docs/device-manifest.md), through `crates/plcc-device`:

```ts
const r = await loadDevice(tomlText, "devices/arduino-opta.toml");
// { ok, manifest, device, diagnostics }: `device` is the expansion (every `repeat`
// unrolled), `diagnostics` [{ file, severity, message, path: "io[3].address", line, col, span }]
await parseDevice(tomlText);      // TOML, value types, unknown keys only
await validateDevice(tomlText);   // plus every semantic check, no expansion
await builtinDevices();           // [{ file, text }]: the Opta and Simulator built into plcc
await deviceSchema();             // the JSON Schema (docs/device-manifest.schema.json)
```

Studio has its own TypeScript loader for manifests (`studio/src/devices/manifest.ts`,
so the editor does not load the 2 MB front end to show a device); both are
tested against the same golden expansions in `crates/plcc-device/tests/data`.

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
  or runs past the profile's area: the runtime 0x08040000-0x0817FFFF for
  `OPTA`, the program slot 0x08180000-0x081FFFFF for `OPTA_PROGRAM` /
  `programProfileFromManifest` (whose `minAddress` is the slot, so a program
  download cannot erase or write the runtime). The `boards.txt` maximum of
  1966080 bytes would run 128 KiB past the end of flash from 0x08040000.
- Every erase command and every data chunk is re-checked right before it is sent.
- No mass erase exists in the code. `leave()` refuses a start address below
  `minAddress`, except the profile's `entry` (a program-slot profile starts the
  runtime at 0x08040000: an image is not a vector table), and never below the
  floor.
- `DfuseDevice.open` refuses a device whose VID:PID, alternate setting or layout
  name the profile does not describe.
- Under every profile sit **built-in floors per bootloader USB id**
  (`src/floors.ts`; for 2341:0364 and 35D1:0364: nothing below 0x08040000,
  nothing past 0x08200000, alt 0, "Internal Flash"). `DfuseDevice` checks every
  erase, write and `leave()` against the floor as well as the profile, so a
  hand-made profile cannot widen it, and it refuses a bootloader with no floor.
- Device manifests (docs/device-manifest.md) are untrusted data:
  `profileFromManifest(device.flash)` builds a profile only when the manifest
  narrows its bootloader's floor (a higher address, a smaller area, known
  runtime ids) and throws `ManifestError` naming the problem otherwise — e.g.
  an application address of 0x08000000, a `max_size` past the end of flash,
  another alternate setting, or an unknown VID:PID.

The 1200-baud touch is implemented by the Arduino core the runtime is built with
(`cores/arduino/USB/USBSerial.cpp`: a 1200-baud line coding followed by DTR low
for 200 ms calls `_ontouch1200bps_()`, which sets an RTC backup-register magic
and resets into the bootloader). The plcc Opta runtime calls `Serial.begin()`,
so a running program can always be touched; a board stuck in a crash loop can
still be put in DFU mode with a double press of its reset button.

## The compiler in the browser

`packages/plcc-compiler-wasm` is plcc's compiler as one WASI command:
`compiler/` (its own Cargo workspace) reads a JSON request on stdin and writes
a JSON response on stdout — no files, no arguments:

```ts
import { compile, loadCompiler } from "@plcc/plcc-compiler-wasm";
const { module } = await loadCompiler({ baseUrl: "/plcc-compiler/", onProgress });
const r = await compile(module, {
  files: { "project.json": model },   // any input check takes: .st, .xml, .L5X, TwinCAT, a ladder model
  entry: ["project.json"],
  target: "wasm32-unknown-unknown",   // or `device: manifestToml` (triple, CPU, float ABI, image sizes)
  image: { I: 18, Q: 1, M: 64 },
  opt_level: 2,
  link: true,                         // wasm32: also link a module for @plcc/plc-wasm
});
// { ok, diagnostics, symbols, target, timings, object, module }
```

- **`crates/plcc-build`** is `plcc compile` over in-memory files: plcc-driver's
  front end (so every input format and the ladder model), LLVM codegen, the
  symbol table, the object as bytes, codegen errors as diagnostics. A parity
  test (`crates/plcc-cli/tests/build_parity.rs`) builds fixtures with both and
  requires the same object and symbol table, wasm32 and Arm, with and without
  a device manifest. It found that LLVM keeps state between builds in one
  process (a wasm32 build changes how a later Arm build lowers `CASE`), so the
  browser runs a **fresh instance per build** (instantiating is cheap; the
  compiled module is reused).
- **Linking for the simulator** (`crates/plcc-wasm-link`, wasmparser +
  wasm-encoder, ~750 lines): LLVM writes relocatable wasm objects only, and
  `wasm-ld` (lld) is not in the browser. YoWASP's lld for WASI would add tens
  of MB; plcc's objects are one object with a handful of relocation types, so
  a single-object linker doing what `wasm-ld --no-entry --export-dynamic
  --allow-undefined --export-table` does is cheaper and was the
  recommendation here. Tested against wasm-ld's output for 46 objects (the
  fixture programs at -O0 and -O2, an L5X program, hand-written IR for the
  cases plcc does not emit yet): same imports and exports, and the same
  process image, globals, prints and faults scan after scan when both run
  side by side (wasmi). It refuses what plcc never emits (constructors,
  undefined data symbols) with a clear error.
- **The WASI host** is 150 lines of TypeScript (`src/wasi.ts`): stdin, stdout,
  stderr, clocks, random; no file system.
- **LLVM 21.1.8 for `wasm32-wasip1`** (`llvm/build.sh`, from the spike):
  ARM and WebAssembly backends only, built with wasi-sdk 34 and YoWASP's WASI
  patch rebased onto 21.1.8 (`llvm-21.1.8-wasi.patch`, Apache-2.0 WITH
  LLVM-exception); threads off, no zlib/zstd/libxml2, MinSizeRel. 71 static
  libraries, 110 MB; packed as a 17 MB `llvm-21.1.8-wasi.tar.xz` (the
  libraries plus the headers llvm-sys compiles against). About an hour to
  build on 3-4 cores.
- **`build.sh`** links the compiler against that prefix (llvm-sys reads a
  stand-in `llvm-config`; an empty `libffi.a`; wasi-sdk's libc++abi and WASI
  emulation libraries; 8 MiB stack, 2 GiB maximum memory) and writes
  `dist/plcc-compiler.wasm.gz` (gzip -9) and `dist/plcc-compiler.json`
  (version, sizes, SHA-256). About a minute with the LLVM prefix in place.

### Size, loading, caching

| | |
|---|---|
| `plcc-compiler.wasm` (release, LTO, `opt-level = "s"`, stripped) | 32.3 MB |
| shipped as `plcc-compiler.wasm.gz` | 10.2 MB |
| gunzip in the browser (DecompressionStream) | ~0.2 s |
| `WebAssembly.compile` (V8, lazy tier-up) | ~0.1 s |
| first Simulate on a local server: download, check, compile, link, run | 1.6-1.7 s |
| first Simulate after a reload (from Cache Storage) | 1.4 s |
| Arm build of the demo for the Opta, compiler already loaded | 0.8-0.9 s |

Nothing loads the compiler until Simulate or Download needs it; typing only
ever waits for the 765 KB front end. The page fetches
`plcc-compiler.json` (no-cache), then the `.gz` (progress in the status bar
and the Download dialog), decompresses it with `DecompressionStream`
(a server that sends `Content-Encoding: gzip` has already done it: the gzip
magic decides), checks its SHA-256 against the manifest, stores it in Cache
Storage under its hash (old versions are deleted), and compiles it in the
compiler's worker. A private window or a full quota just skips the cache.

Brotli would be 6.8 MB, but browsers only decompress brotli when the server
sends `Content-Encoding: br`, which GitHub Pages does not do for `.wasm`;
gzip through `DecompressionStream` works on every static host. `wasm-opt -Oz`
saves 7 MB raw but less than 1 MB gzipped and makes code generation ~40 %
slower, so it is not used.

### Shipping it: built in CI, deployed with the Pages artifact

The 32 MB module is not committed. `.github/workflows/studio-pages.yml`
builds every browser package and deploys them with the studio as the Pages
artifact (GitHub Pages: 100 MB per file, 1 GB per site; the whole site is
about 14 MB):

1. LLVM for WASI comes from `actions/cache` (keyed on `llvm/`); on a miss,
   from this repository's release **`llvm-wasi-21.1.8-r1`**
   (`llvm-21.1.8-wasi.tar.xz`, checked against `llvm/llvm-21.1.8-wasi.sha256`);
   only if that is missing too is it built from source (about an hour, within
   the job's 150-minute limit). The release asset is built locally from the
   spike's build: `WORK=<scratch> sh packages/plcc-compiler-wasm/llvm/build.sh
   package`, then `gh release create llvm-wasi-21.1.8-r1
   <scratch>/llvm-21.1.8-wasi.tar.xz` (update the `.sha256` file with it).
2. wasi-sdk 34, `wasm-bindgen` 0.2.129 and binaryen are installed.
3. `packages/plcc-wasm`, `packages/plc-image` and
   `packages/plcc-compiler-wasm` are built and tested, then `plc-wasm` and
   `webdfu` are tested.
4. The studio is linted, tested (its tests use the real compiler, front end
   and image linker) and built with `STUDIO_BASE=/<repo>/`; the build copies
   the compiler into `plcc-compiler/`.

Without the compiler (a local build that skipped `build.sh`) the studio still
builds and works: Simulate runs the preview engine and says why, Download
says the compiler is not part of the build.

### Browser support

| | Chrome / Edge | Firefox, Safari |
|---|---|---|
| Editing, check, Problems, ST view, import / export | yes (tested: headless Chromium) | expected (Workers, wasm); untested |
| Simulate with plcc's build (Workers, `DecompressionStream`, Cache Storage) | yes (tested) | expected (Firefox 113+, Safari 16.4+); untested |
| Projects in OPFS | yes | Firefox 111+, Safari: in memory where OPFS writes are missing (the status bar says so) |
| Online, Detect (WebSerial) and Download (WebSerial + WebUSB) | yes | no: Chromium-only APIs |
| TwinCAT project folder import (File System Access) | yes | no: import the project as a .zip |

The compiler runs in its own worker with its own linear memory (allowed to
grow to 2 GiB) and a fresh instance per build.

### Alternatives that were not taken

- **A second backend for the simulator** (ST → WebAssembly with
  `wasm-encoder`): 6-10 weeks to parity with plcc-codegen's ~15 000 lines, two
  backends to keep in lockstep forever, and a simulator that no longer runs
  the code the PLC runs. Cranelift has no 32-bit ARM and does not emit wasm.
- **An HIR interpreter**: the same divergence problem, slower.
- **lld in the browser** (YoWASP, ~105 MB unpacked): far larger than the
  problem (above).
- **A native compile service**: ruled out; nothing runs outside the browser.

### Download to the Opta without the Arduino toolchain: program images

Implemented; the format and the loader are [program-image.md](program-image.md).
The runtime (firmware) is flashed once; each download replaces only the
program, which the runtime validates and loads from a fixed slot, the way
real PLC runtimes work:

```
plcc compile (LLVM, wasm in the browser) ─▶ prog.o ─▶ @plcc/plc-image ─▶ prog.img ─▶ @plcc/webdfu ─▶ slot 0x08180000
                                                      (crates/plcc-image,           (programProfileFromManifest:
                                                       88 KB wasm)                   only the slot, leave → runtime)
```

```ts
import { buildDeviceImage } from "@plcc/plc-image";
import { DfuseDevice, programProfileFromManifest, touch1200, waitForDfuDevice } from "@plcc/webdfu";

const img = await buildDeviceImage(device, object);   // device: the expanded manifest (loadDevice)
//   { bytes, address: 0x08180000, size, buildId, header, imports, sections, layout }
const profile = programProfileFromManifest(device.flash, device.device.name);
await touch1200(port);
const dev = await DfuseDevice.open(await waitForDfuDevice(navigator.usb, profile), { profile });
await dev.flash(img.bytes);                           // erases 1-4 sectors of the slot, starts the runtime
// then `info` on the console: "program": { "build": img.buildId, ... }
```

- **Memory map** (manifest version 2, `[flash.program]`): runtime
  0x08040000-0x0817FFFF (1.25 MiB, ~206 KiB used), program slot
  0x08180000-0x081FFFFF (512 KiB, bank 2 sectors 4-7), the program's
  `.data`/`.bss` in DTCM 0x20010000-0x2001FFFF (the Arduino core leaves DTCM
  unused above its vector table).
- **Image**: 128-byte header (magic, format, ABI, device id + manifest
  version, layout, sizes, `plcc_get_app`, a 16-byte build id, two CRC-32s),
  code and constants executed in place, a 16-byte veneer per imported runtime
  service (`movw/movt ip; ldr ip, [ip]; ldr pc, [ip, #8+4k]` through a table
  whose address the runtime stores at the start of the window — no runtime
  address is baked into a program), the `.data` initial values.
- **Linker** (`crates/plcc-image`, std-only Rust + sha2): every relocation
  type plcc emits for Cortex-M plus the other Thumb ones defensively; all
  fixture objects byte-identical to `ld.lld`; images run under QEMU through
  the runtime's own loader code.
- **Runtime** (`runtimes/arduino-opta/loader`): USB first, the slot checked
  (both CRCs, device, versions, layout, ranges) before anything in it runs,
  every call into the program guarded (plcc_fault, CPU faults, a 500 ms
  watchdog → STOP with outputs off, USB and Modbus alive), `info`/`prog`/
  `stop`/`run` on the console. Built and statically verified; **its first
  flash to hardware is pending** (runtimes/arduino-opta/README.md, "First
  flash").
- **Studio**: its manifest loader reads `[flash.program]` and the
  `prog`/`stop`/`run` commands; Download builds the image and writes the slot
  (after checking over the console that the board runs a program-image
  runtime of the same device); Online shows the program state from `info` and
  offers Run / Stop.

The remaining codegen change worth making: drop `.ARM.exidx` at the source
(`nounwind`, no unwind tables); the image linker discards it today.

## Status and what is left

- Done: the studio's model is plcc's ladder model; plcc check, convert and the
  compiler run in the browser; Simulate runs plcc's wasm32 build; Download
  compiles for the Opta, links a program image and writes the program slot
  over WebUSB (checked end to end against webdfu's fake bootloader).
- Left: the first flash of the loader runtime to an Opta at the bench
  (runtimes/arduino-opta/README.md), then a Download and an Online session
  against it from Chrome; publishing the `llvm-wasi-21.1.8-r1` release asset
  (until then the first CI run builds LLVM, then caches it).
- Worth doing: compile Cortex-M code for the real CPU (cortex-m7 + FPU,
  softfp) — the Opta manifest's `cpu`/`features` already do this for builds
  with a device; drop `.ARM.exidx` at the source (`nounwind`).

## Development

```bash
# front end → packages/plcc-wasm/pkg (needs wasm-bindgen-cli 0.2.129, optional wasm-opt)
cd packages/plcc-wasm && npm install && npm run build && npm test
cd packages/plc-wasm  && npm install && npm test      # fixtures: npm run fixtures (plcc + wasm-ld)
cd packages/webdfu    && npm install && npm test
cd packages/plc-image && npm install && npm test      # builds pkg/plcc_image.wasm (cargo, wasm32) if missing
# the compiler → packages/plcc-compiler-wasm/dist (rustup target add wasm32-wasip1)
WORK=~/.cache/plcc/llvm-build sh packages/plcc-compiler-wasm/llvm/build.sh package   # once, ~1 h
LLVM_WASI=… WASI_SDK=… sh packages/plcc-compiler-wasm/build.sh                       # ~1 min
cd packages/plcc-compiler-wasm && npm install && npm test
```

The packages are plain TypeScript ES modules with no framework and no runtime
dependencies; `main`/`types` point at `src/index.ts` for a Vite build to
consume directly. The studio links them (`studio/.npmrc`: `install-links=false`).
