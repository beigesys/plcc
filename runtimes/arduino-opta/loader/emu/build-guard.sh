#!/bin/sh
# SPDX-License-Identifier: MPL-2.0
# Build the bare-metal fault-guard test: ./build-guard.sh <out.elf>
# Run: qemu-system-arm -M mps2-an500 -nographic -semihosting-config enable=on,target=native \
#        -kernel <out.elf> -device loader,file=<image>,addr=0x00100000
set -e
here=$(cd "$(dirname "$0")" && pwd)
out=${1:?usage: build-guard.sh <out.elf>}
${CC:-arm-none-eabi-gcc} -mcpu=cortex-m7 -mthumb -mfloat-abi=softfp -mfpu=fpv5-d16 \
  --specs=nosys.specs -O1 -ffreestanding -fno-builtin -nostartfiles -static -Wall -Wextra \
  -T "$here/guard_test.ld" -o "$out" \
  "$here/guard_test.c" "$here/../plcc_guard.c" "$here/../plcc_image.c" "$here/../plcc_services.c" -lm -lc -lgcc
