<!-- SPDX-License-Identifier: MPL-2.0 -->

# Program images

A **program image** is a plcc program linked for a fixed place in a device's
flash and RAM, so that the device's runtime (its firmware) can be flashed
**once** and every program after that is downloaded on its own, without
relinking the firmware and without any toolchain beyond plcc itself. It is
how a browser downloads a program to an Arduino Opta: plcc (compiled to
WebAssembly) produces the object, `plcc-image` (also WebAssembly) links it into
an image, and WebUSB DFU writes the image into the program slot.

```
   plcc compile --device arduino-opta     plcc image --device arduino-opta       DFU
 prog.st ─────────────────────────▶ prog.o ─────────────────────────▶ prog.img ────────▶ slot
                                   (ELF, relocatable)   crates/plcc-image      (header + code + data)
```

The runtime contract inside the program is unchanged
([process-image.md](process-image.md)): `plcc_get_app()` returns the
`plcc_app_t` descriptor (tasks, process image, init, run_task, retain), and the
runtime drives it exactly as it drives a linked-in program. What changes is
the glue around it:

- the image carries a **header** the runtime validates before it executes a
  single instruction of the program;
- the program reaches the runtime only through a **service table** whose
  address the runtime hands it at start-up — nothing in the image is an
  absolute address inside the firmware, so a program survives a runtime
  update that keeps the table;
- `.data` and `.bss` live in a **RAM window** the runtime reserves, and the
  runtime copies and zeroes them before `init`.

Implementations: the linker is `crates/plcc-image` (`plcc image`, and
`packages/plc-image` in the browser); the loader is
`runtimes/arduino-opta/loader` (`plcc_image.c` holds the checks, shared with
the emulator test harness).

## Memory map (Arduino Opta, manifest version 2)

The device manifest's `[flash.program]` (docs/device-manifest.md) fixes the
slot, the window and the number of services; this is `devices/arduino-opta.toml`:

```
flash (STM32H747, 2 MiB, 16 × 128 KiB sectors)
0x08000000 ┌──────────────────────────────┐
           │ Arduino bootloader (256 KiB) │ protected: webdfu never writes below 0x08040000
0x08040000 ├──────────────────────────────┤
           │ runtime (firmware), ≤ 1.25 MiB│ [flash] address / max_size = 0x140000
           │  (today ~170 KiB)            │ build-loader.sh refuses a firmware that ends past 0x08180000
0x08180000 ├──────────────────────────────┤
           │ program slot, 512 KiB        │ [flash.program] address / max_size = 0x80000
           │  header | text | veneers |   │ bank 2, sectors 4-7: a download erases only these
           │  .data image | 0xFF pad      │
0x08200000 └──────────────────────────────┘

DTCM (M7 data TCM, 128 KiB, not cached, not used by the Arduino core)
0x20000000 ┌──────────────────────────────┐
           │ vector table (166 × 4 bytes) │ mbed relocates the vectors here (SCB->VTOR)
           │ free (runtime .dtcm, if any) │ build-loader.sh refuses a firmware using DTCM ≥ 0x20010000
0x20010000 ├──────────────────────────────┤
           │ services pointer (4) + spare │ [flash.program] ram = { start, size = 0x10000 }
0x20010008 │ .data │ .bss │ COMMON       │ zeroed / copied by the loader before init
0x20020000 └──────────────────────────────┘

AXI SRAM 0x24000000-0x2407FFFF: the runtime's .data/.bss, mbed heap, MSP stack (unchanged)
```

Why there: the Arduino core's linker script (`variants/OPTA/linker_script.ld`)
places the firmware's `.data`/`.bss`, the heap and the main stack in AXI SRAM
and fills it to the end; DTCM holds only the relocated vector table and an
(empty) `.dtcm` section, so its upper half is free without changing the core.
The program's code runs from flash in place. The program's stack is the
runtime's thread stack (32 KiB, `MBED_CONF_RTOS_MAIN_THREAD_STACK_SIZE`).

## The image

Little-endian. The header is the slot's first 128 bytes; the body follows.

```c
#define PLCC_IMAGE_MAGIC 0x49434C50u   /* bytes "PLCI" */
#define PLCC_IMAGE_FORMAT 1u

typedef struct plcc_image_header {     /* 128 bytes */
  uint32_t magic;          /*   0 */
  uint16_t format;         /*   4  1 */
  uint16_t header_size;    /*   6  128 */
  uint32_t image_size;     /*   8  header + body, a multiple of 32 (an STM32H7 flash word) */
  uint32_t body_crc32;     /*  12  CRC-32 of bytes [header_size, image_size) */
  char     target_id[24];  /*  16  device manifest id, NUL-padded: "arduino-opta" */
  uint32_t target_version; /*  40  device manifest version the image was linked for */
  uint32_t abi;            /*  44  runtime-contract ABI, PLCC_ABI_VERSION (1) */
  uint32_t services;       /*  48  service-table entries used: highest index + 1 */
  uint32_t slot_addr;      /*  52  flash address linked at   ([flash.program] address) */
  uint32_t slot_size;      /*  56                           ([flash.program] max_size) */
  uint32_t ram_addr;       /*  60  RAM window linked for     ([flash.program] ram) */
  uint32_t ram_size;       /*  64 */
  uint32_t text_addr;      /*  68  slot_addr + 128: code, read-only data, veneers */
  uint32_t text_size;      /*  72 */
  uint32_t data_load;      /*  76  flash address of .data's initial values */
  uint32_t data_addr;      /*  80  RAM address .data is copied to */
  uint32_t data_size;      /*  84 */
  uint32_t bss_addr;       /*  88  RAM zeroed before init (.bss and COMMON) */
  uint32_t bss_size;       /*  92 */
  uint32_t services_slot;  /*  96  = ram_addr: the word the loader stores the table's address in */
  uint32_t get_app;        /* 100  plcc_get_app, Thumb (bit 0 set) */
  uint32_t flags;          /* 104  0; a loader refuses any bit it does not know */
  uint8_t  build_id[16];   /* 108  identifies the build */
  uint32_t header_crc32;   /* 124  CRC-32 of bytes [0, 124) */
} plcc_image_header_t;
```

```
slot_addr  ┌ header (128) ┬ text: .text* sections, .rodata* sections ┬ veneers (16 B each) ┬ .data image ┬ 0xFF ┐
           └──────────────┴──────────── text_size ───────────────────┴─────────────────────┴─ data_size ┴ pad ─┘
ram_addr   ┌ services ptr (4) | spare (4) ┬ .data ┬ .bss, COMMON ┬ free ┐
```

- **CRC-32** is CRC-32/ISO-HDLC (zlib's `crc32`, reflected polynomial
  0xEDB88320, initial value and final XOR 0xFFFFFFFF). Two of them: the header
  is checked before any of its fields is trusted; the body before anything in
  it runs. Padding to the 32-byte flash word is `0xFF` and covered by the body
  CRC.
- **Target id** = the manifest's `device.id` + `device.version`. The Opta
  runtime accepts `arduino-opta` versions from 2 (the first with a program
  slot) up to its own. Every layout field (`slot_*`, `ram_*`) must equal the
  runtime's exactly, `abi` must be its ABI, and `services` may not exceed its
  table.
- **Build id**: by default the first 16 bytes of the SHA-256 of the object
  file; a caller (studio) may pass its own, e.g. a hash of the project
  sources, so "is the program in the PLC the one open in the editor" is a
  16-byte comparison. `plcc image --build-id <32 hex digits>`.
- **Exports**: the header carries `plcc_get_app` (and through it the whole
  contract: `plcc_app.init`, `run_task`, the task table and the process
  image). The loader checks every pointer in the returned descriptor — code
  inside `text`, state inside the RAM window, the process-image sizes equal
  the device's `target.image` — before it calls any of them.

## Runtime services

A plcc object imports a few symbols: `plcc_monotonic_ns`, `plcc_print`,
`plcc_fault` (runtime-symbols.md), and whatever the compiler and libm leave as
calls: `memset`, 64-bit division (`__divdi3`, `__aeabi_ldivmod`), 64-bit ↔ float
conversions (`__floatdisf`, `__fixdfdi`, …), `sinf`, `pow`, … An image
resolves each import to a **veneer** that jumps through the runtime's service
table:

```c
typedef struct plcc_service_table {
  uint32_t magic;     /* 0x53434C50, "PLCS" */
  uint32_t count;     /* entries */
  const void *fn[];   /* fn[i]: the service with index i (Thumb address) */
} plcc_service_table_t;
```

```
veneer k (16 bytes, Thumb-2):
  f240 0c__   movw ip, #:lower16:services_slot
  f2c0 0c__   movt ip, #:upper16:services_slot
  f8dc c000   ldr.w ip, [ip]                 ; the table
  f8dc f___   ldr.w pc, [ip, #8 + 4*k]       ; fn[k]; interworking load, arguments untouched
```

`ip` (r12) is the AAPCS intra-procedure-call scratch register that veneers may
clobber, so a veneer is transparent to the caller and the callee. `BL`/`B.W`
to an import, its address in data (`R_ARM_ABS32`) or in a `MOVW`/`MOVT` pair
all resolve to the veneer.

The table is **append-only**: an index never changes meaning, so an image
linked against an older runtime runs on a newer one. The list is
`crates/plcc-image/src/services.rs` (a test keeps it identical to
`runtimes/arduino-opta/loader/plcc_services.h`): 0 `plcc_monotonic_ns`,
1 `plcc_print`, 2 `plcc_fault`, 3-18 `memcpy`/`memmove`/`memset`/`memcmp` and
their `__aeabi_mem*` forms, 19-48 integer helpers (`__aeabi_ldivmod`,
`__divdi3`, …), 49-64 64-bit ↔ float conversions, 65-128 soft-float (AEABI and
GNU names, for objects built without an FPU), 129-156 double libm (`sin` …
`nearbyint`), 157-184 their `float` forms. The Opta's runtime has all 185.
Importing anything else is a link error naming the symbol.

`plcc_fault` is special: plcc objects for ARM *define* it weakly (a trapping
default, runtime-symbols.md). The image linker treats a weak definition whose
name is a service like a weak definition in a normal link that meets a strong
one: references go to the runtime's service.

Data imports are not supported (there are none in plcc output).

## Linking (crates/plcc-image)

Input: one relocatable ELF32 little-endian ARM object, as `plcc compile`
writes for a Cortex-M target (EABI version 5; a hard-float object is refused
for a soft-float device). Output: the image above. The linker is std-only Rust
with no LLVM (it builds for `wasm32-unknown-unknown`), ~1 800 lines with the service list and unit tests.

- Sections: allocatable `PROGBITS` that are executable → text, read-only →
  text after the code, writable → `.data`; `NOBITS` → `.bss`; `COMMON`
  symbols are allocated after `.bss`. `.ARM.exidx*`/`.ARM.extab*` (unwind
  tables; nothing unwinds on a PLC) are dropped, which also drops their
  reference to `__aeabi_unwind_cpp_pr0`. Non-allocatable sections (symbols,
  attributes, debug info) are ignored. Refused with an error naming the
  section: TLS, `.init_array`/`.fini_array`/`.preinit_array` (constructors),
  writable code, any other allocatable section type, relocations against a
  `NOBITS` section.
- Section order follows the object, each at its own alignment; gaps inside
  text are filled with `0xd4` (what lld fills ARM code with), gaps in `.data`
  with zeros.
- Relocations (`SHT_REL` with implicit addends and `SHT_RELA`), computed as
  lld computes them (`S` symbol value with the Thumb bit, `A` addend, `P`
  place, `Pa` = `P & ~3`):

| Type | Value | Use in plcc output |
|---|---|---|
| `R_ARM_NONE` (0), `R_ARM_V4BX` (40) | — | markers |
| `R_ARM_ABS32` (2), `R_ARM_TARGET1` (38) | S + A | pointers in `.rodata` / `.data` (task table, `plcc_app`) |
| `R_ARM_REL32` (3) | S + A − P | |
| `R_ARM_PREL31` (42) | S + A − P, 31 bits | `.ARM.exidx` only (dropped) |
| `R_ARM_THM_CALL` (10) | S + A − P, ±16 MiB | `BL` (always BL: Cortex-M has no ARM state) |
| `R_ARM_THM_JUMP24` (30) | S + A − P, ±16 MiB | `B.W` tail calls |
| `R_ARM_THM_JUMP19` (51) | S + A − P, ±1 MiB | `B<cc>.W` |
| `R_ARM_THM_JUMP11` (102) | S + A − P, ±2 KiB | `B` (16-bit) |
| `R_ARM_THM_JUMP8` (103) | S + A − P, ±256 B | `B<cc>` (16-bit) |
| `R_ARM_THM_MOVW_ABS_NC` (47), `R_ARM_THM_MOVT_ABS` (48) | S + A | addresses of state, image, strings |
| `R_ARM_THM_MOVW_PREL_NC` (49), `R_ARM_THM_MOVT_PREL` (50) | S + A − P | |
| `R_ARM_THM_PC8` (11) | S + A − Pa, 0..1020, ×4 | `LDR`/`ADR` (16-bit) literal |
| `R_ARM_THM_PC12` (54) | S + A − Pa, ±4095 | `LDR.W` literal |
| `R_ARM_THM_ALU_PREL_11_0` (53) | S + A − Pa, ±4095 | `ADR.W` |

  plcc's Cortex-M output uses `ABS32`, `THM_CALL`, `THM_JUMP24`,
  `THM_MOVW_ABS_NC`, `THM_MOVT_ABS`, `NONE` and `PREL31` (every fixture
  program at `-O0` and `-O2`); the others are supported defensively and tested
  against `ld.lld` with hand-written assembly. Anything else (ARM-state
  relocations, GOT, TLS, `R_ARM_TARGET2`, …) is an error naming the type,
  section and offset. A value out of range is an error, never a silent wrap
  (there are no range-extension thunks: the slot is 512 KiB).
- Undefined symbols: a service → its veneer; an undefined weak symbol that is
  not a service → 0; anything else is an error listing every missing symbol.
- `plcc_get_app` must be defined (in text), else the object is not a plcc
  program.

Tests (`crates/plcc-image/tests/link.rs`, `crates/plcc-cli/tests/image.rs`,
`packages/plc-image`):

- every relocation type, from hand-written assembly, byte-identical to
  `ld.lld` with an equivalent linker script; all 44 fixture objects (22
  programs at `-O0` and `-O2`) byte-identical to `ld.lld` in text and `.data`;
- objects it must refuse (each error above), every loader check provoked one
  at a time, truncation and byte-flip sweeps without a panic, the Rust and C
  service lists compared, the WebAssembly build byte-identical to the native
  one;
- images **run** under `qemu-arm -cpu cortex-m7` in a harness built from the
  runtime's own `plcc_image.c` and service table (newlib/libgcc as on the
  board): 64-bit division, libm, conversions, `PRINT`, a division-by-zero
  fault with its source location, TON timing, a GCC-compiled object with
  `.data` and COMMON, corrupt images refused, every fixture for 30 scans;
- the runtime's fault guard runs bare-metal on `qemu-system-arm -M
  mps2-an500` (Cortex-M7) set up like the Opta (vectors in RAM at
  0x20000000, thread mode on the PSP, a 10 ms tick): a UsageFault, a
  BusFault, an endless loop and `plcc_fault` each stop the task with outputs
  off and the program runnable again; a fault outside the program goes to the
  original handler.

The emulator tests are skipped where qemu or `arm-none-eabi-gcc` is missing;
the `ld.lld` comparisons fall back to committed golden files.

```bash
plcc compile prog.st -o prog.o --device arduino-opta
plcc image prog.o --device arduino-opta -o prog.img [--build-id <hex>] [--map]
plcc image --info prog.img        # print and verify a header
```

## Loading (the runtime's side)

What the Opta loader does (`runtimes/arduino-opta/loader`), and what any
loader of this format must do:

1. **Bring up the console first.** USB serial is up before the slot is even
   read (the Arduino core starts USB CDC before `setup()`), so the 1200-baud
   touch that enters the bootloader works whatever the slot holds.
2. **Validate, without executing**: magic (an erased slot, `0xFFFFFFFF`, is
   "no program"), format, header size, header CRC, target id and version
   range, ABI, `services` ≤ its table, slot and RAM window equal its own,
   every range inside the slot / window (in 64-bit arithmetic), `get_app`
   Thumb code inside text, `flags` = 0, body CRC. Only an image that passes
   all of them is ever jumped into. The Opta reads the slot inside the fault
   guard: an interrupted download can leave a half-programmed flash word,
   whose ECC error is a bus fault on read, and that must be "no program", not
   a crash at boot.
3. **Prepare**: zero the RAM window, copy `.data`, store the service table's
   address at `services_slot`.
4. **Call `get_app()`** (guarded, see below) and validate the descriptor:
   inside the image, ABI, ≤ 32 tasks, process-image sizes equal the device's.
5. **`init()`, then run tasks** with the scan loop of process-image.md.
6. **Faults stop the program, never the runtime.** `plcc_fault` (division by
   zero) and any CPU fault (HardFault, BusFault, MemManage, UsageFault) or a
   scan longer than the watchdog *while program code runs* clear `%Q`, write
   the outputs off and put the PLC in STOP with the reason in `info`; USB and
   Modbus keep running. A restart (`run`) is a cold start: window zeroed,
   `.data` copied, `init` again.
7. **No program**: the runtime runs with outputs off (Modbus and the console
   serve a zero image) and reports `"program": null`.

## Versioning

- `format` changes only if the header layout or the meaning of a field
  changes; a loader refuses formats it does not know.
- New services are appended (the manifest's `services` grows with a new
  manifest version); an old image keeps working on a new runtime.
- Changing the slot, the window or the ABI is a new manifest version and makes
  older images invalid, which the loader reports instead of running them.
