<!-- SPDX-License-Identifier: MPL-2.0 -->

# plcc runtimes for the Arduino Opta

Two Arduino sketches that run plcc-compiled IEC 61131-3 programs on the Opta
(STM32H747, Cortex-M7), built with `arduino-cli` and the Arduino `mbed_opta`
core (4.6.0). Both implement the device manifest `devices/arduino-opta.toml`:
the same process image, Modbus RTU map and USB console.

| | `loader/` — program-image runtime | `runtime/` — linked runtime |
|---|---|---|
| Programs | downloaded separately into the program slot, any number of times | linked into the firmware: one Arduino build and full flash per program |
| Build | `./build-loader.sh` (once) | `./build.sh program.st` (per program) |
| Manifest | version 2 (`[flash.program]`) | version 1 |
| Status | **built and statically verified; not yet run on hardware** | in use on the bench |

The licence of our code is MPL-2.0. The sketches link against the Arduino
core and its libraries (mbed OS, ArduinoModbus, ArduinoRS485; LGPL-2.1 and
others) as any Arduino sketch does; nothing from them is copied here.

## The program-image runtime

```
flash  0x08000000  Arduino bootloader (256 KiB)          never written
       0x08040000  this runtime (≤ 1.25 MiB; ~206 KiB)    flashed once (DFU, application area)
       0x08180000  program slot (512 KiB)                 one program image (docs/program-image.md)
DTCM   0x20010000  program RAM window (64 KiB)            the program's .data/.bss
```

### Build

```bash
./build-loader.sh            # arduino-cli compile + tools/check-loader.py
./build-loader.sh --upload   # ... and flash it (see "First flash" below)
```

`tools/check-loader.py` checks the linked ELF before anything is flashed:
the firmware ends below the slot and uses no RAM in the window; the service
table has all 185 entries, each the Thumb address of its symbol; the core's
`main()` begins USB CDC before `setup()`; the fault handlers are installed
before the first call into a program; the slot is read only inside a guarded
call (`check_slot` → `plcc_image_check`), and `plcc_image_prepare` (the only
way to obtain the program's entry point) is called once, after that check,
whose result is tested; there is no direct branch into the slot.

### Download a program

```bash
plcc compile plant.st -o plant.o --device arduino-opta -O2
plcc image plant.o --device arduino-opta -o plant.img
# 1200-baud touch: the board reboots into its DFU bootloader
stty -F /dev/ttyACM0 1200 && sleep 2
# Write the slot only. Do NOT use `:leave` here: it would start the board at
# the slot address, and an image is not a vector table.
dfu-util -d 2341:0364 -a 0 -s 0x08180000 -D plant.img
# then press RESET (or power-cycle): the runtime starts and loads the program
```

In the browser (plcc studio) the same steps are `buildDeviceImage` from
`@plcc/plc-image` and `@plcc/webdfu` with `programProfileFromManifest`,
which can erase and write only the slot and leaves DFU mode into the
runtime at 0x08040000.

### Boot and safety

1. The Arduino core starts USB CDC — and the thread that turns a 1200-baud
   touch into a reboot to the bootloader — in `main()`, before `setup()`. A
   download is always possible whatever the slot holds.
2. `setup()`: outputs off, Modbus RTU up, fault handlers installed (the CPU
   fault vectors in the RAM vector table mbed sets up at 0x20000000; if that
   is not where the vectors are, no program is ever run), 10 ms watchdog tick.
3. The slot is checked without executing anything in it: magic (an erased
   slot is "no program"), format, header CRC-32, device id and manifest
   version (2..2), ABI, service count, slot and window, every range, body
   CRC-32. The check itself runs guarded: an ECC error from a half-written
   flash word (an interrupted download) is reported, not a crash.
4. Only then: the RAM window is zeroed, `.data` copied, the service table's
   address stored; `plcc_get_app()` is called (guarded) and its descriptor
   checked (code in the image, state and process image in the window, sizes
   18/1/64) before `init()`.
5. Holding the **USER button during boot** leaves the program stopped (`run`
   starts it).

While running, a program fault — `plcc_fault` (division by zero), a
HardFault/BusFault/MemManage/UsageFault in program code, or a scan longer than
**500 ms** — stops the program: outputs off, red LED blinking,
`PLC STOP: fault <code> ... at <where>` on the console every 4 s, `"state":"fault"`
in `info`. USB, the console and Modbus keep running. `run` cold-starts the
program again (window cleared, `init`).

What the guard cannot contain (documented, not handled): a fault with a
broken exception frame (stack overflow past the thread stack) or a fault in
the runtime itself goes to mbed's crash handler after the outputs are turned
off; the board then needs a reset (or a double press of RESET for the
bootloader). A program that corrupts the runtime's memory is not possible
through plcc-generated code (it only writes its window), but nothing enforces
it in hardware (the MPU is not used).

### Console

`info`, `img`, `mw <n> <value>` as before (docs/device-manifest.md), plus:

```
> info
{"device":"arduino-opta","manifest":2,"runtime":"plcc-arduino","abi":1,"image":{"I":18,"Q":1,"M":64},"state":"run","program":{"build":"59f1c5d389c78996cab89552e10ad22f","size":1632,"crc":"9a3c…","version":2}}
> prog
{"valid":true,"why":"ok","format":1,"target":"arduino-opta","version":2,"abi":1,"services":1,"size":1632,"text":1476,"data":0,"bss":168,"build":"59f1…","header_crc":"…","body_crc":"…"}
> stop
ok stop
> run
ok run
```

With no program: `"state":"empty","program":null,"reason":"empty slot"`.

### First flash (needs someone at the bench)

This runtime changes the board's boot path, so its first flash is done with
someone who can press RESET:

1. Relays off and nothing connected that must not switch. Note the current
   firmware (`~/opta_plcc/build.sh ~/opta_plcc/chaser_io.st` restores it).
2. `./build-loader.sh --upload` — check-loader.py must report 0 failures.
3. Expect within ~2 s: the USB serial port back, `info` reports
   `"manifest":2,"state":"empty"` and `"reason":"empty slot"` (or, if the slot
   holds old data, a reason such as `"no program image (bad magic)"`), relays
   off. Touch at 1200 baud: the board must enter DFU (`dfu-util -l` shows
   2341:0364).
4. Download `chase` (crates/plcc-cli/tests/data/image/chase.st, I1 on: relays
   1-3 chase) or `chaser_io.st` as an image, reset, check `info` shows
   `"state":"run"` and the build id, and the outputs behave.
5. Fault path: download `crates/plcc-cli/tests/data/image/div_zero.st` (a
   division by zero on the third scan): `info` must show `"state":"fault"`
   with `div_zero.st:13:10: Div`, relays off, USB alive; `run` restarts it.
6. Recovery if anything hangs: double-press RESET (bootloader, green LED
   pulsing), then flash the known-good firmware.

## The linked runtime

`./build.sh program.st [--no-upload]` compiles the program with
`--device arduino-opta`, links it into `runtime/runtime.ino` and uploads the
whole firmware (`IO_MAP`, `DEVICE`, `PLCC`, `ARDUINO_CLI`, `BUILD_DIR` in the
script's header). For `chaser_io.st` it builds a firmware byte-identical to
`~/opta_plcc/build.sh`.

## Emulator tests

`loader/emu/` runs the loader's own C code off the board (both driven by
`cargo test -p plcc --test image`, skipped without the tools):

- `harness.c` under `qemu-arm -cpu cortex-m7`: program images run through
  `plcc_image.c`'s checks and the real service table (newlib/libgcc), with a
  fake clock.
- `guard_test.c` bare-metal on `qemu-system-arm -M mps2-an500` (Cortex-M7),
  set up like the Opta: the fault guard contains a UsageFault, a BusFault, an
  endless loop (watchdog) and `plcc_fault`, and chains other faults.
