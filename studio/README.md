<!-- SPDX-License-Identifier: MPL-2.0 -->

# plcc studio

A web ladder editor for PLC programs, built on plcc. Everything runs in the
browser, with no server of any kind: projects are stored in the browser's
Origin Private File System (OPFS), plcc itself (its front end, its LLVM code
generator and its linkers) runs as WebAssembly in Web Workers, the simulator
runs plcc's own build of the project, and an Arduino Opta is watched over
WebSerial and flashed over WebUSB.

![Simulate: plcc's wasm32 build of the demo running in the browser](docs/screenshots/simulate.png)

## Run it

```bash
cd studio
npm i
npm run dev          # http://localhost:5173
```

The browser packages it uses are siblings in `../packages` and are linked, not
copied (`.npmrc`). Two of them carry WebAssembly that is built, not committed:

| Package | Build | Without it |
|---|---|---|
| `@plcc/plcc-wasm` (front end, 2.2 MB) | `cd ../packages/plcc-wasm && npm run build` (wasm-bindgen-cli 0.2.129) | the studio does not build |
| `@plcc/plc-image` (program image linker, 88 KB) | `cd ../packages/plc-image && sh build.sh` | Download cannot link |
| `@plcc/plcc-compiler-wasm` (the compiler, 10 MB gzip) | `../packages/plcc-compiler-wasm/build.sh` (LLVM for WASI; docs/studio-wasm.md) | Simulate runs the preview engine, Download cannot compile |

| Script | What it does |
|---|---|
| `npm test` | vitest: model and rung text, golden tests against plcc-wasm (the model round-trips through plcc, rung text reads as plcc reads it, the catalog is plcc's), migration of old projects, the command registry, the preview engine, the simulator worker running plcc's real build, Download against a fake bootloader, the serial protocol and the WebSerial transport, the store. Tests that need a built wasm package skip without it. |
| `npm run lint` | ESLint (typescript-eslint, react-hooks) |
| `npm run build` | type check (`tsc -b`, strict), then the production bundle in `dist/` (with the compiler in `dist/plcc-compiler/` when it is built) |
| `npm run screenshots` | builds the app, drives it in headless Chromium, checks the flows end to end (below) and writes `docs/screenshots/*.png` and `perf.json`. `STUDIO_BASE=/plcc/` runs it as GitHub Pages serves it. Run `npx playwright install chromium` once first |

Online, Detect and Download need Chrome or Edge (WebSerial, WebUSB), served
from `localhost` or `https`. GitHub Pages builds it all in CI
(`.github/workflows/studio-pages.yml`).

## Screens

| | |
|---|---|
| Offline editing | ![offline](docs/screenshots/offline.png) |
| Simulate (plcc's wasm32 build) | ![simulate](docs/screenshots/simulate.png) |
| plcc's diagnostics on a broken rung, Problems | ![diagnostics](docs/screenshots/diagnostics.png) |
| ST view | ![st view](docs/screenshots/st-view.png) |
| Right-click menu | ![context menu](docs/screenshots/context-menu.png) |
| Import (L5X) | ![import](docs/screenshots/import-l5x.png) |
| Download | ![download](docs/screenshots/download.png) |
| I/O mapping | ![io](docs/screenshots/io-mapping.png) |
| Online (demo device), serial log | ![online](docs/screenshots/online-demo.png) |
| Control Room theme | ![control room](docs/screenshots/theme-control-room.png) |
| Blueprint theme | ![blueprint](docs/screenshots/theme-blueprint.png) |

The layout has five parts:

- **Top bar (52 px):** logo, File (projects, import, export), a project /
  program / routine breadcrumb, the Offline | Simulate | Online switch, a
  device status chip, the theme menu, and Download.
- **Left nav (232 px):** devices, programs and their routines (with their
  problem counts), then the project views: Tags, I/O mapping and Tasks.
- **Center:** the rung list. Each rung is a card with a number gutter, a
  comment, the SVG ladder, and an editable rung-text bar in monospace with
  plcc's diagnostics for the rung under it. The ST view opens beside it.
- **Right inspector (300 px):** the selected tag (type, value, address,
  terminal, comment), Force ON/OFF, the selected instruction's problems, and
  the device's I/O list with state dots. In Simulate this list becomes the
  virtual I/O panel.
- **Problems panel** (from the status bar's counter) and the **status bar
  (30 px):** what runs the simulation, scan time, period, jitter, the
  problem counts, the cursor position, and the save state.

## Editing

The editor works from the keyboard first. Click a rung or press Tab to focus
the rung list, then:

| Key | Action |
|---|---|
| Arrow keys | Left/right move through the instructions of a rung. Up/down move to the next leg of a branch, or to the next rung |
| `c` / `C` | insert XIC / XIO after the selection |
| `o`, `l`, `u` | insert OTE, OTL, OTU |
| `b` | command palette, to pick a box instruction (TON, GRT, ...) |
| `p` | add a branch around the selection (on a branch: add another leg) |
| `n` / `Shift+N` | new rung below / above |
| Enter | edit the selected instruction's operands, with tag autocomplete |
| Delete / Backspace | delete the instruction, or the rung if only the rung is selected |
| Ctrl+C / Ctrl+X / Ctrl+V / Ctrl+D | copy, cut, paste, duplicate (as rung text, also on the system clipboard) |
| Alt + Up/Down | move the rung up or down |
| Alt + Left/Right | move the instruction within its branch |
| `t` | edit the rung's text |
| `/` | quick entry |
| Space | Simulate/Online: toggle the selected contact's BOOL tag |
| Shift+F10, the Menu key | the selection's context menu |
| Ctrl+Shift+S | ST view |
| Ctrl+K | command palette |
| Ctrl+Z / Ctrl+Y | undo / redo |

**Right-click menus** on instructions (edit, change type XIC ↔ XIO, OTE ↔
OTL ↔ OTU, TON ↔ TOF ↔ RTO, ...; insert before / after; add a branch; cut,
copy, paste, duplicate, delete; go to tag; show in ST view; toggle and force
in Simulate and Online), rungs (insert above / below, duplicate, move, edit
comment, copy and paste as rung text, delete), tags in the tag table, the
inspector and the I/O list (go to usages, rename, map to a terminal, force,
copy name), routines (open, rename, duplicate, export as ST / PLCopen /
L5X, delete) and devices (detect, change, view manifest, update). Menus,
keys and the palette all run the same commands (`src/state/registry.ts`),
and every menu item shows its shortcut.

Other ways to edit:

- **Quick entry** is the box under the rungs and also the command palette.
  Typing `XIC Start XIO Stop OTE Motor` and pressing Enter adds a rung.
  Branches are written as `BST ... NXB ... BND` or `[ ... , ... ]`. Any text
  containing `(` is read as rung text instead.
- **Rung text** is the bar under each rung, in the canonical form Studio 5000
  exports and plcc writes into L5X: `[XIC(StartPB) ,XIC(Motor) ]XIO(StopPB)OTE(Motor);`.
  Edit it and press Enter, and the drawing is rebuilt. Instruction ids are
  kept wherever the rung's shape still matches.
- **The toolbar** has chips (XIC, OTE, TON, ...). Click one to insert it after
  the selection, or drag it onto an instruction or a rung.
- **New tags** are created the first time a tag is used, with a type inferred
  from the instruction: contacts, coils and ONS/OSR/OSF bits get BOOL;
  TON/TOF/RTO get TIMER; CTU/CTD get COUNTER; math and compare operands get
  DINT.

The editor speaks Logix ladder (plcc-ladder's Logix dialect): XIC, XIO, OTE,
OTL, OTU, ONS, OSR, OSF, TON, TOF, RTO, CTU, CTD, RES, EQU, NEQ, GRT, GEQ,
LES, LEQ, LIM, ADD, SUB, MUL, DIV, MOV, CPT, JSR, JMP / LBL, RET, and an
`ST("...")` box of Structured Text (plcc writes it to L5X as an ST routine
called by JSR). Any other of plcc's 91 Logix instructions (from an import, or
typed as rung text) is drawn as a box with its operands named from plcc's
catalog and compiled by plcc.

## Architecture

```
src/
  model/      plcc-ladder's model as TypeScript (types.ts), its JSON (json.ts),
              rung text, the instruction catalog (logix-catalog.json, from
              plcc), tree edits, ids, addresses and the process image, and the
              migration of the first studio's projects
  plcc/       the workers: plcc's front end (frontend.ts, check / convert /
              manifests) and its compiler (compiler.ts, loaded on demand)
  engine/     the preview engine (a Logix-like interpreter of the model) and
              the pure power-flow trace used by Simulate and Online
  runtime/    the simulator worker: SimWorker runs the preview engine, then
              plcc's build (PlcHost, @plcc/plc-wasm); fixed-period loop
  devices/    device manifests: TOML loader and validator, the catalog, a
              project's devices
  serial/     console protocol, OnlineSession (info, img polling, mw, prog /
              stop / run, the traffic log), WebSerial transport, fake console
  store/      FileStore (OPFS or memory), project <-> files, ProjectRepo,
              zip export / import, autosave
  state/      zustand stores: editor, live values, problems (plcc check),
              simulate (builds), download, convert (import / export), the
              command registry and its commands, persistence, theme
  ui/         React components; ladder/ has the SVG layout and renderer
```

What runs where (docs/studio-wasm.md has the details and the numbers):

| Thread | What |
|---|---|
| page | the editor, OPFS, WebSerial (Online), WebUSB (Download) |
| front-end worker | `@plcc/plcc-wasm` (765 KB gzip): check after edits (debounced 400 ms), the ST view and import / export (convert), manifest validation, fault sites |
| compiler worker | `@plcc/plcc-compiler-wasm` (10.2 MB gzip, fetched on the first Simulate or Download, cached in Cache Storage by its SHA-256): a fresh instance per build |
| simulator worker | the preview engine, then plcc's wasm32 build run by `@plcc/plc-wasm` |

- **The UI thread is never blocked.** Checks, builds and scans run in workers.
  The simulator posts a snapshot of power flow and *changed* tag values at
  most about 30 times a second; the page merges snapshots once per animation
  frame. On the production build in headless Chromium the page holds 60 fps
  (p99 frame 16.8 ms, no long tasks) with plcc's program running
  (`docs/screenshots/perf.json`).
- **Themes:** *Graphite* (dark, default), *Control Room* (ISA-101 HMI, IBM
  Plex) and *Blueprint* (light). Until you pick one, the theme follows
  `prefers-color-scheme`. Color is used only for state: `--power` energized,
  `--alarm` (amber) abnormal (warnings, unassigned operands, the preview
  engine), `--fault` (red) errors (plcc's errors, a faulted PLC).

## Model

A project is plcc's ladder model — crate `plcc-ladder`,
`crates/plcc-ladder/src/model.rs`, documented in
[docs/ladder-translation.md](../docs/ladder-translation.md) — in the Logix
dialect, plus the project's devices. `src/model/types.ts` mirrors the Rust
types field for field:

```ts
Project  { dialect: 'logix', name, globals: Variable[], pous: Pou[], declarations, tasks: Task[],
           devices: DeviceRef[], deviceFiles }             // the last two are the studio's
Pou      { id, name, kind: 'program', variables: Variable[], routines: Routine[] }  // routines[0] is the main routine
Routine  { id, name, rungs: Rung[] }                       // one rung holding one ST box: an ST routine
Rung     { id, comment?, label?, elements: Element[] }
Element  = { type: 'contact', id, operand, kind: 'no' | 'nc' }
         | { type: 'coil', id, operand, kind: 'normal' | 'set' | 'reset' }
         | { type: 'branch', id, legs: Element[][] }
         | { type: 'block', id, name, pins: { name, dir, value }[] }   // TON(Timer, Preset, Accum), ...
         | { type: 'jump', id, label } | { type: 'return', id } | { type: 'st', id, code }
Variable { name, data_type, section, initial?, address?, comment?, constant?, retain? }
Task     { name, interval_ms?, priority?, programs }
```

Ids are numbers unique in the project; plcc's diagnostics and fault sites
name them, so they land on the rung and element. Golden tests
(`src/model/plcc.test.ts`) round-trip projects through plcc-wasm's `convert`,
check that the rung text reads the way plcc reads it, that
`logix-catalog.json` is plcc's catalog (`node scripts/gen-catalog.mjs`
regenerates it), and that the demo checks clean.

plcc compiles the model as it compiles an L5X export: each variable's
`address` binds it to the process image (the L5X I/O map, docs/l5x.md), and
each task is a periodic (or continuous) Logix task.

### On disk (OPFS)

```
projects/<id>/project.toml             format = 2, name, [[devices]] (name, manifest)
projects/<id>/project.json             the plcc-ladder model: `plcc convert project.json --to l5x` works on it
projects/<id>/devices/<device-id>.toml the project's copy of each device manifest (docs/device-manifest.md)
```

Projects saved by the first studio (`tags.toml`, `routines/*.ladder.json` in
its draft model) are migrated when opened and saved in format 2. Its
extensions that Logix has no instruction for become their Logix equivalents,
each reported: an edge contact becomes `XIC(x)OSR(edgeN_sb,edgeN)` in a rung
before and `XIC(edgeN)` in place; an edge coil `OSR` / `OSF`; a negated coil
an `OTE` of a helper and `XIO(helper)OTE(x)` in a rung after.

Export and import produce the same files inside a `.zip`. TOML is handled by
smol-toml and zip by fflate, both MIT. If OPFS is unavailable (a private
window, an old browser), projects are kept in memory and the status bar says
so.

## What works

- **Editing:** rungs, branches, every instruction above, comments, rung text
  both ways, undo/redo, drag and drop, the palette, quick entry, context
  menus, tag autocomplete and creation; ST routines.
- **Check (plcc):** after every edit, in a worker. Errors and warnings mark
  their element (and the operand's position in the rung text), list under the
  rung, count per routine in the left nav, and fill the Problems panel; ST
  routines mark their lines. Device manifests are validated by plcc too.
- **ST view:** the Structured Text plcc compiles a routine's program to (Logix
  semantics, calls into its Logix prelude), or its translation to IEC
  61131-3, with the translation's notes.
- **Import / export:** File > Import reads Rockwell `.L5X`, PLCopen `.xml`
  (CODESYS, OpenPLC), `.st` and TwinCAT projects (a `.zip`, or a folder in
  Chrome) through plcc's converter, translates IEC ladder and ST to Logix,
  lists every translation note, and adds the programs to the project or opens
  them as a new one. File > Export (and a routine's menu) writes L5X, PLCopen
  XML or IEC ST, with plcc's notes about what behaves differently.
- **Tags, I/O mapping, devices, tasks:** as before; tasks are periodic
  (interval) or continuous (empty), and each is compiled as such.
- **Simulate:** plcc compiles the project to wasm32 in the browser, links it,
  and the simulator worker runs it with `@plcc/plc-wasm`: plcc's task
  scheduler, the device's image sizes, every instruction plcc compiles. The
  first time, the compiler downloads (progress in the status bar); until the
  first build is ready, and whenever the project cannot be built, the
  TypeScript **preview** engine runs instead, labelled as such. Edits rebuild
  the program (a cold restart). Virtual I/O, force, write and toggle work as
  before; power flow is computed from the model and the program's live
  values. A PLC fault stops the program and is marked on its element.
- **Download:** compiles the project for its device (the manifest's triple,
  CPU, float ABI and image sizes), links a program image for the device's
  program slot (`@plcc/plc-image`), then, on a click: checks over the console
  that the board runs a program-image runtime of the same device (otherwise
  nothing is touched), reboots it into its bootloader (1200-baud touch), and
  writes and verifies only the program slot with `@plcc/webdfu`'s slot
  profile; leaving DFU starts the runtime. Tested end to end against webdfu's
  fake bootloader; not yet run against hardware (the program-image runtime
  has not been flashed to an Opta yet).
- **Online:** WebSerial to the device's USB console at the manifest's baud
  rate (115200 8N1, DTR asserted, commands ending in a single `\n`).
  - `info` first: the device id, runtime, ABI and image sizes are compared
    with the project's manifest. A program-image runtime also reports its
    program's state (running, stopped, empty, or the fault and its site),
    shown in the device chip, the banner and the inspector, with **Run** and
    **Stop**; `info` is re-read every second.
  - `img` is polled every 100 ms; %I, %Q and the manifest's `img_m_bytes`
    of %M are decoded; tags with an address show live values and power flow.
  - Force ON/OFF on %M BOOL tags is a read-modify-write of the containing
    word with `mw`; `%MW` tags can be written.
  - The **serial log** shows every line sent and received, timeouts and
    disconnects (img polls hidden by default), and saves it as text.
  - **Demo device** runs the same console protocol against a fake Opta.

### Checked end to end (`npm run screenshots`)

The demo opens and checks clean; right-click StartPB → Change type → XIO
changes it; Shift+F10 opens the menu; a broken rung gets plcc's diagnostics
on two elements and in Problems; the ST view shows the generated ST and the
IEC translation; Simulate runs plcc's build (1.6-1.7 s on a local server,
1.4 s from the cache after a reload), StartPB seals Motor in and RunTimer
accumulates about 1000 ms per second; Download builds the Opta image
(10.7 KB at 0x08180000); an L5X fixture and an ST file import; Online against
the demo device shows the console traffic and the program state. With
`STUDIO_BASE=/plcc/` too.

## What is left

- Run Download and an Online session against a real Opta once the
  program-image runtime is on it (runtimes/arduino-opta/README.md).
- A project can list several devices, but Simulate, Online and Download use
  the first (the controller).
- ST boxes and ST routines are Logix ST (plcc compiles them through its L5X
  path); the preview engine does not run them (plcc's build does).
