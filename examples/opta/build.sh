#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# Build a standalone Arduino Opta firmware image from a Structured Text program.
#
#   ./build.sh [program.st]
#
# Produces opta.bin, ready to flash at 0x08040000 with dfu-util.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
OUT="$HERE/build"

ST="${1:-$HERE/blink.st}"
PLCC="${PLCC:-$ROOT/target/release/plcc}"
CC="${CC:-arm-none-eabi-gcc}"

# Must match what plcc emits for --target thumbv7em-none-eabihf --cpu cortex-m7.
# A mismatch here is silent: floats would pass in core registers on one side of
# the call and in s0-s15 on the other, and nothing would fail at link time.
ARCH=(-mcpu=cortex-m7 -mthumb -mfpu=fpv5-d16 -mfloat-abi=hard)

mkdir -p "$OUT"

echo "==> compiling $(basename "$ST")"
"$PLCC" compile "$ST" -o "$OUT/plc.o" \
    --target thumbv7em-none-eabihf \
    --cpu cortex-m7 \
    --stdlib none

echo "==> building shim"
"$CC" "${ARCH[@]}" -c "$HERE/firmware.c" -o "$OUT/firmware.o" \
    -Os -ffreestanding -fno-builtin -Wall -Wextra \
    -ffunction-sections -fdata-sections

echo "==> linking"
"$CC" "${ARCH[@]}" -T "$HERE/opta.ld" -o "$OUT/opta.elf" \
    "$OUT/firmware.o" "$OUT/plc.o" \
    -nostdlib -nostartfiles -Wl,--gc-sections -Wl,-Map="$OUT/opta.map"

arm-none-eabi-objcopy -O binary "$OUT/opta.elf" "$OUT/opta.bin"

echo
arm-none-eabi-size "$OUT/opta.elf"
echo
echo "image: $OUT/opta.bin ($(stat -c%s "$OUT/opta.bin") bytes)"
echo "flash: dfu-util -a 0 -s 0x08040000:leave -D $OUT/opta.bin"
