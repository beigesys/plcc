<!-- SPDX-License-Identifier: MPL-2.0 -->

# Arduino Opta — bare-metal firmware from Structured Text

A complete standalone firmware image built around a plcc-compiled ST program.
No Arduino core, no mbed, no libc. The whole image is 332 bytes.

```bash
./build.sh                    # builds blink.st
./build.sh my_program.st      # or your own
```

## Flashing

Put the board in DFU mode — double-tap the reset button, or touch the serial
port at 1200 baud:

```bash
stty -F /dev/ttyACM0 1200 hupcl
dfu-util -a 0 -s 0x08040000:leave -D build/opta.bin
```

The image loads at `0x08040000`, which is where the factory bootloader hands
off. Staying above it keeps USB DFU recovery working. The bootloader's own
sector is marked read-only in the DFU descriptor anyway:

```
@Internal Flash /0x08000000/01*128Ka,15*128Kg
                            ^^^^^^^^ 'a' = read-only
```

so it cannot be erased through this interface even by mistake.

Under WSL the board must be forwarded with `usbipd`, and it re-enumerates under
a different PID when it enters DFU — so it needs re-attaching after the reset.

## What the LEDs mean

| | |
| --- | --- |
| LED_D2 solid | firmware reached `main()` |
| LED_D0 blinking | the ST program is scanning |
| all three solid | hard fault |

The relays are deliberately untouched. They are on the same GPIO port
(`PI_4`-`PI_7`) but are mechanical parts with a finite cycle count, and a
scan-rate blink would chew through them.

## Board facts

| | |
| --- | --- |
| MCU | STM32H747XI, Cortex-M7 (the M4 core is left unbooted) |
| Build target | `--target thumbv7em-none-eabihf --cpu cortex-m7` |
| Status LEDs | LED_D0 `PI_0`, LED_D1 `PI_1`, LED_D2 `PI_3` |
| Relays 1-4 | `PI_6` `PI_5` `PI_7` `PI_4` |
| Stack / data | DTCM at `0x20000000`, 128K |

`--cpu cortex-m7` is not optional. The default for `thumbv7em` is `cortex-m4`,
which is single-precision only, so every `LREAL` would fall back to
`__adddf3`/`__muldf3` soft-double calls. The M7 has an FPv5-D16 unit.

The float ABI in `build.sh` must match what plcc emits. A mismatch is silent:
floats would pass in core registers on one side of the call and in `s0`-`s15`
on the other, with nothing failing at link time on a linker that does not check
build attributes.

## No header is generated

plcc emits no header for the state struct, so `firmware.c` hard-codes the
offset of the variable it reads. `blink.st` lowers to `{ i8, i32 }`, putting
`led0` at offset 0 — confirmed by compiling to `.ll` and reading the
`getelementptr` in `<pou>_init`. Declaring the output variable first is a
deliberate habit until plcc can emit a header.

## Timers

`blink.st` counts scans rather than time, so it needs no external symbols at
all — the linked image has zero undefined symbols. Using `TON`, `TOF` or `TP`
requires supplying `plcc_monotonic_ns()`; see
[docs/runtime-symbols.md](../../docs/runtime-symbols.md). Note that a 32-bit
microsecond counter is not good enough: the contract requires a monotonic
64-bit nanosecond value that does not wrap within a session.
