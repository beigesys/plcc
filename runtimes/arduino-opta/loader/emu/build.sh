#!/bin/sh
# SPDX-License-Identifier: MPL-2.0
# Build the emulator harness: ./build.sh <out-binary>
# Needs arm-none-eabi-gcc with newlib (the Arduino core's ABI: Cortex-M7,
# softfp, fpv5-d16). Run images with: qemu-arm -cpu cortex-m7 <out> <image> <scans> <step_ms>
set -e
here=$(cd "$(dirname "$0")" && pwd)
out=${1:?usage: build.sh <out-binary>}
${CC:-arm-none-eabi-gcc} -mcpu=cortex-m7 -mthumb -mfloat-abi=softfp -mfpu=fpv5-d16 \
  --specs=nosys.specs -O1 -ffreestanding -fno-builtin -nostartfiles -static -Wall -Wextra -Wno-unused-parameter \
  -Wl,-Ttext=0x00400000 -Wl,--entry=_start \
  -o "$out" "$here/harness.c" "$here/../plcc_image.c" "$here/../plcc_services.c" -lm -lc -lgcc
