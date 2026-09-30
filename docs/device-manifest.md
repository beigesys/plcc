<!-- SPDX-License-Identifier: MPL-2.0 -->

# Device manifests

A device manifest is one TOML file that describes a device: the board, what
its runtime exposes (target, process image, console, Modbus, I/O terminals),
and how to build, flash and talk to it. The compiler, the browser front end,
plcc studio and the WebUSB flasher all read the same file.

```
devices/arduino-opta.toml            the catalog (repository root; a git submodule)
<project>/devices/arduino-opta.toml  a project's copy; project.toml lists it
crates/plcc-device/builtin/*.toml    built-in fallback copies of the Opta and Simulator
```

- **The catalog** is the directory `devices/` at the root of the plcc
  repository. It holds manifests only (and a README) and is maintained as its
  own repository, mounted as a git submodule. Nothing in plcc imports from it
  except by reading the `.toml` files; when the submodule is not checked out,
  plcc falls back to the built-in copies of the Opta and Simulator manifests
  compiled into `crates/plcc-device` (a test keeps them identical to the
  catalog files when both are present).
- **A project** copies the manifest of each of its devices into
  `devices/<id>.toml` at the project root, so the project builds the same way
  later even if the catalog moves on. Updating a project's copy from a newer
  catalog version is an explicit step (studio shows the diff and asks).

The Rust implementation is `crates/plcc-device` (pure Rust, builds for
wasm32); the JSON Schema for editors is
[`docs/device-manifest.schema.json`](device-manifest.schema.json), generated
from the Rust types.

## Trust

**A manifest is untrusted data.** Treat a downloaded or imported manifest like
any other file from the internet: it may be wrong or malicious. In particular:

- **Flashing.** `[flash]` can only *narrow* what the flasher allows, never
  widen it. `packages/webdfu` has built-in, code-level limits per bootloader
  USB id (for the Opta, 2341:0364 / 35D1:0364: never below `0x08040000`, never
  a sector the device's own DfuSe layout marks read-only, no mass erase). A
  manifest whose address, size or alternate setting falls outside those limits
  is rejected with an error; a USB id with no built-in entry cannot be flashed
  at all until one is added to webdfu's code. The limits are
  `packages/webdfu/src/floors.ts`; `profileFromManifest(device.flash)` turns a
  manifest into a flashing profile (narrowed to the manifest's application
  area) or throws `ManifestError`, and `DfuseDevice` re-checks every erase and
  write against the floor whatever profile it was given.
- **Compiling.** A manifest selects the LLVM triple, CPU, features and image
  sizes. It never supplies code, paths or shell commands.
- **Provenance.** `device.source` (where the file came from) and
  `device.sha256` (the hash of the file as published there) are reserved for a
  future registry. plcc validates their format but does not yet fetch or
  verify anything.

## An example

The catalog's Opta manifest, abridged (the full file is
`devices/arduino-opta.toml`):

```toml
[device]
id = "arduino-opta"
name = "Arduino Opta"
vendor = "Arduino"
description = "STM32H747 (Cortex-M7) PLC running the plcc generic runtime..."
version = 1          # this file's version; bump on every change
schema = 1           # the manifest format (this document)

[target]
triple = "thumbv7em-none-eabi"
cpu = "cortex-m7"
features = ["+fp-armv8d16"]
float_abi = "softfp"
image = { I = 18, Q = 1, M = 64 }

[target.runtime]
kind = "plcc-arduino"
abi = 1

[flash]
method = "dfuse"
usb = [{ vid = 0x2341, pid = 0x0364 }, { vid = 0x35D1, pid = 0x0364 }]
alt = 0
layout = "Internal Flash"
address = 0x08040000
max_size = 0x1C0000
reboot = "1200-baud-touch"
runtime_usb = [{ vid = 0x2341, pid = 0x0064 }, ...]
protected = [{ start = 0x08000000, size = 0x40000, reason = "Arduino bootloader (sectors 0 and 1)" }]

[console]
transport = "webserial"
baud = 115200
commands = ["info", "img", "mw"]
img_format = "hex-areas"
img_m_bytes = 64

[modbus.rtu]
interface = "RS485"
unit = 1
baud = 19200
parity = "even"

[[modbus.map]]
table = "holding"    # holding register n <-> %MWn
count = 32
address = "%MW0"

[[io]]
repeat = 8           # I1..I8
id = "I{n}"
terminal = "I{n}"
label = "Input I{n}"
group = "Digital inputs"
dir = "in"
kind = "digital"
type = "BOOL"
address = "%IX0.{n-1}"
```

## `[device]`

| Key | Type | | |
|---|---|---|---|
| `id` | string | required | Lowercase letters, digits and single `-` (`arduino-opta`). The runtime's `info` reports the same id. |
| `name` | string | required | Display name. |
| `vendor` | string | required | |
| `description` | string | | |
| `version` | integer ≥ 1 | required | This manifest's version. Bump it on every change; studio compares a project's copy with the catalog by it. |
| `schema` | integer | required | The manifest format: `1`. A file with another value is rejected. |
| `source` | string | | Reserved: where the file was downloaded from. |
| `sha256` | string | | Reserved: 64 hex digits, the SHA-256 of the file at `source`. |

## `[target]`

| Key | Type | | |
|---|---|---|---|
| `triple` | string | required | LLVM target triple (`thumbv7em-none-eabi`, `wasm32-unknown-unknown`). |
| `cpu` | string | | LLVM CPU (`cortex-m7`). Absent: the triple's generic CPU. |
| `features` | array of strings | | LLVM target features, one per string, each `+name` or `-name`. |
| `float_abi` | `soft` / `softfp` / `hard` | | ARM only. `soft`: no FPU instructions. `softfp`: FPU instructions, floats passed in integer registers (needs an `eabi` triple). `hard`: floats in FPU registers (needs `eabihf`). |
| `image` | `{ I, Q, M }` | required | Process-image area sizes in bytes (docs/process-image.md). Every program for the device is built with exactly these sizes, so one runtime build fits all of them. |
| `runtime.kind` | string | required | The runtime (`plcc-arduino`). |
| `runtime.abi` | integer | required | The runtime-contract version it implements (`PLCC_ABI_VERSION`, docs/process-image.md). |

`plcc compile --device` passes `triple`, `cpu`, `features` and `float_abi` to
LLVM and fixes the image sizes; see [Compiling for a device](#compiling-for-a-device).

## `[flash]` (optional)

| Key | Type | | |
|---|---|---|---|
| `method` | `dfuse` | required | USB DFU 1.1 with ST's DfuSe extensions. |
| `usb` | array of `{ vid, pid }` | required | The bootloader's USB ids. |
| `alt` | integer | default 0 | DFU alternate setting. |
| `layout` | string | | The alternate's DfuSe layout name must start with this (`Internal Flash`). |
| `address` | integer | required | Where the application goes. |
| `max_size` | integer | required | Largest application, bytes. |
| `leave` | bool | default true | Start the application after the download. |
| `reboot` | `none` / `1200-baud-touch` | default `none` | How to get from the running application into the bootloader. |
| `runtime_usb` | array of `{ vid, pid }` | | The running application's USB ids (its serial port), for `reboot`. Required for `1200-baud-touch`. |
| `protected` | array of `{ start, size, reason }` | | Flash that must never be erased or written. The application area may not overlap it. |

TOML integers may be written in hex (`0x08040000`). See [Trust](#trust) for
how the flasher bounds these values.

## `[console]` (optional)

The runtime's line-oriented console on a USB serial port.

| Key | Type | | |
|---|---|---|---|
| `transport` | `webserial` | required | A USB CDC serial port (WebSerial in a browser). |
| `baud` | integer | required | |
| `commands` | array of `info` / `img` / `mw` | required | The commands the runtime answers. |
| `img_format` | `hex-areas` | required | Format of the `img` reply. |
| `img_m_bytes` | integer | required | `%M` bytes the `img` reply includes, from byte 0; at most `image.M`. |

### Console protocol

Commands and replies are single lines ending in `\n` (the runtime also
accepts `\r\n`). Other lines may appear at any time (program output from
`plcc_print`, boot messages) and are ignored by clients, except a fault line
starting with `PLC STOP:`.

| Command | Reply | |
|---|---|---|
| `info` | `{"device":"arduino-opta","manifest":1,"runtime":"plcc-arduino","abi":1,"image":{"I":18,"Q":1,"M":64}}` | One line of JSON: the device id and manifest version the runtime was built for, `target.runtime.kind`, `target.runtime.abi`, and the image sizes. A client compares it with the project's manifest. |
| `img` | `I: 1 0 0 0 D2 7 ...  Q: 11  M: 1 0 ...` | `hex-areas`: `I:`, `Q:`, `M:` each followed by the area's bytes in upper-case hex without zero padding, separated by single spaces; two spaces before `Q:` and `M:`. `M` holds the first `img_m_bytes` bytes. |
| `mw <n> <value>` | `ok %MW<n> := <value>` | Write `%MWn` (bytes 2n, 2n+1, little-endian). `value` is taken modulo 2^16. |
| anything else | `? commands: ...` | |

## `[modbus]` (optional)

| Key | Type | | |
|---|---|---|---|
| `rtu.interface` | string | | `RS485`. |
| `rtu.unit` | integer 1..247 | required | Server address. |
| `rtu.baud` | integer | required | |
| `rtu.data_bits` | 7 / 8 | default 8 | |
| `rtu.parity` | `none` / `even` / `odd` | required | |
| `rtu.stop_bits` | 1 / 2 | default 1 | |
| `[[modbus.map]]` | | | One entry per table range. |
| `map.table` | `coils` / `discrete` / `input` / `holding` | required | |
| `map.start` | integer | default 0 | First item number (0-based protocol address). |
| `map.count` | integer | required | |
| `map.address` | string | required | Image address of item `start`. Coils and discrete inputs map to consecutive **bits** (a `%_X` address: coil n ↔ `%QX(n/8).(n%8)`); registers to consecutive **words** (a `%_W` address: holding register n ↔ `%MWn`). |

Coils and holding registers are written by the master and may not alias `%I`;
discrete inputs and input registers are read-only and normally alias `%I`.
Ranges of one table may not overlap, and every range must fit its area.

## `[[io]]`: I/O points

Each entry is one terminal-level point, or with `repeat`, a numbered group.

| Key | Type | | |
|---|---|---|---|
| `id` | string | required | Unique in the manifest. |
| `terminal` | string | required | The name printed on the device (`I1`, `R2`). Several points may share a terminal (a digital and an analog reading of `I1`); studio remaps tags by terminal when a project changes device. |
| `label` | string | required | |
| `group` | string | required | Heading the point is listed under. |
| `dir` | `in` / `out` / `mem` | required | Must match the address area: `%I`, `%Q`, `%M`. |
| `kind` | `digital` / `analog` / `register` | required | Digital points need a bit address; analog points a byte or larger. |
| `type` | string | required | IEC type; must fit the address size: `X` BOOL; `B` BYTE, SINT, USINT, CHAR; `W` WORD, INT, UINT, WCHAR; `D` DWORD, DINT, UDINT, REAL; `L` LWORD, LINT, ULINT, LREAL. |
| `address` | string | required | Direct address (`%IX0.3`, `%IW1`), inside `target.image`. Written back in canonical form (`%I0.3` → `%IX0.3`). |
| `range` | `[min, max]` | | Raw value range. |
| `eng` | `[at_min, at_max]` | | Engineering value at the ends of `range` (linear scaling); needs `range`. |
| `units` | string | | Engineering units. |
| `description` | string | | |
| `repeat` | integer 1..4096 | | Number of points this entry stands for. |
| `from` | integer | default 1 | First value of `n`. |

### `repeat` and expressions

In an entry with `repeat = N`, the variable `n` runs from `from` (default 1)
through `from + N - 1`, and every string field may contain `{expr}`: integer
arithmetic on `n` with literals, `+ - * / %`, unary `-` and parentheses. `/`
and `%` truncate toward zero. `{{` and `}}` are literal braces.

```toml
[[io]]
repeat = 16                          # n = 1..16
id = "DI{n}"
terminal = "DI{n}"
label = "Digital input {n}"
group = "Inputs, byte {(n-1)/8}"
dir = "in"
kind = "digital"
type = "BOOL"
address = "%IX{(n-1)/8}.{(n-1)%8}"   # DI1 = %IX0.0 ... DI9 = %IX1.0
```

A repeated entry's `id` must contain an expression (so the ids differ), and an
entry without `repeat` may not contain one. Two points on the same address are
a warning.

## Diagnostics

Every problem is reported with the file, line and column of the offending
value and its path in the document:

```
devices/x.toml:26:11: error: io[0].address: `%IX2.0` (n = 2) ends at byte 3, past the 2-byte %I area (target.image.I)
devices/x.toml:5:1: error: unknown field `colour`, expected one of `id`, `name`, ...
```

Syntax errors, wrong value types and unknown keys come from the TOML parser;
everything else (addresses, sizes, overlaps, expressions) from the validator.

## The expanded form

Consumers work with the **expanded** manifest: the same sections with every
`repeat` unrolled, `from` gone, types upper-cased and addresses canonical,
as JSON (`plcc device check --json`, `expandDevice` in plcc-wasm):

```json
{
  "device": { "id": "arduino-opta", "name": "Arduino Opta", "version": 1, "schema": 1, ... },
  "target": { "triple": "thumbv7em-none-eabi", "cpu": "cortex-m7", "image": { "I": 18, "Q": 1, "M": 64 }, ... },
  "io": [
    { "id": "I1", "terminal": "I1", "label": "Input I1", "group": "Digital inputs",
      "dir": "in", "kind": "digital", "type": "BOOL", "address": "%IX0.0" },
    ...
  ]
}
```

`crates/plcc-device/tests/data/*.expanded.json` are the expansions of the
built-in and test manifests; the Rust validator and studio's TypeScript loader
are both tested against them.

## Compiling for a device

```bash
plcc compile prog.st -o prog.o --device arduino-opta --emit-header prog.h
plcc compile prog.st -o prog.o --device ./my-board.toml
plcc device check my-board.toml           # diagnostics; --json prints the expansion
plcc device list                          # the catalog
```

`--device` takes a manifest file or a catalog id. It sets:

| From the manifest | Flag it stands for | Opta |
|---|---|---|
| `target.triple` | `--target` | `thumbv7em-none-eabi` |
| `target.cpu` | `--cpu` | `cortex-m7` |
| `target.features` | `--features` | `+fp-armv8d16` |
| `target.float_abi` | `--float-abi` | `softfp` |
| `target.image` | `--image-size I=… Q=… M=…` | 18, 1, 64 |

An explicit flag overrides the manifest (`--image-size` per area; `--features`
replaces the list). plcc refuses a manifest whose `target.runtime.abi` is not
its own runtime-contract version. `--emit-header` records the manifest as
`PLCC_DEVICE_ID` and `PLCC_DEVICE_MANIFEST_VERSION` (and `PLCC_TARGET_CPU`,
`PLCC_TARGET_FEATURES`); `--emit-symbols` as `"device": {"id", "version"}`, so a
runtime can refuse a program built for another device at compile time.

The float ABI: LLVM takes the calling convention from the triple (`eabi`
passes floats in integer registers, `eabihf` in FPU registers). `softfp` and
`hard` check the triple against it; `soft` also turns the FPU off. The
Opta's Arduino core builds with `-mcpu=cortex-m7 -mfloat-abi=softfp
-mfpu=fpv5-d16`, which is exactly `cortex-m7`, `+fp-armv8d16` and `softfp`:
REAL arithmetic is FPU instructions, and a program's object still links with
the core's code.

Catalog lookup: `$PLCC_DEVICES` if set, else the `devices/` directory of the
plcc checkout the binary was built from, else the built-in copies.

## Versioning

- `device.schema` is the format. This document is format 1; a later format
  gets a new number, and plcc says which one it reads.
- `device.version` is the manifest's own version: a new I/O point, a changed
  flash limit or a runtime update that changes the console bumps it. A
  runtime's `info` reports the version it was built for.
- `target.runtime.abi` is the process-image contract (docs/process-image.md).
  `plcc compile --device` refuses a manifest whose ABI is not the compiler's.
