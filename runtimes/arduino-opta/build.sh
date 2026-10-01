#!/bin/sh
# SPDX-License-Identifier: MPL-2.0
#
# Compile an ST program with plcc and flash it to the Opta LINKED INTO the
# generic runtime (runtime/runtime.ino): one Arduino build per program.
#   ./build.sh program.st [--no-upload]
# Environment:
#   IO_MAP=file.toml   L5X only: bind Logix tags to image addresses (docs/l5x.md)
#   DEVICE=id|file     device manifest (default arduino-opta; docs/device-manifest.md)
#   PLCC=path          the plcc binary (default: this checkout's target/debug/plcc)
#   ARDUINO_CLI=path   arduino-cli (default: ~/.local/bin/arduino-cli, else on PATH)
#   BUILD_DIR=dir      also write the build output (.elf, .bin, .map) there
#
# The program-image runtime (flashed once, programs downloaded separately)
# is built by build-loader.sh; see README.md.
set -e
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
plcc=${PLCC:-$repo/target/debug/plcc}
cli=${ARDUINO_CLI:-$HOME/.local/bin/arduino-cli}
[ -x "$cli" ] || cli=arduino-cli
fqbn=arduino:mbed_opta:opta
prog=${1:?usage: build.sh program.st [--no-upload]}
upload=$2

# The device manifest fixes the target (Cortex-M7 with its FPU, soft-float
# calling convention like the Arduino core) and the image sizes, so every
# program gets the full I/O and Modbus map.
( ulimit -v $((4 * 1024 * 1024)); "$plcc" compile "$prog" -o "$here/runtime/plc.o" \
    --emit-header "$here/runtime/plc.h" --device "${DEVICE:-arduino-opta}" ${IO_MAP:+--io-map "$IO_MAP"} )
"$cli" compile -b $fqbn --build-property "compiler.c.elf.extra_flags=$here/runtime/plc.o" \
    ${BUILD_DIR:+--output-dir "$BUILD_DIR"} "$here/runtime"
[ "$upload" = "--no-upload" ] && exit 0
port=$(ls -l /dev/serial/by-id/ | grep -i opta | grep -o 'ttyACM[0-9]*')
"$cli" upload -b $fqbn -p "/dev/$port" "$here/runtime"
