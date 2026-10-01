#!/bin/sh
# SPDX-License-Identifier: MPL-2.0
#
# Build the program-image runtime (loader/loader.ino) and verify the linked
# firmware before anyone flashes it (tools/check-loader.py):
#   - it ends below the program slot (0x08180000) and uses no RAM in the
#     program window (0x20010000-0x2001FFFF);
#   - the service table has every entry, each the Thumb address of its symbol;
#   - the core's main() starts USB CDC before setup(), and setup() installs
#     the fault handlers before it can call into a program;
#   - the slot is entered only through plcc_image_prepare, called only after
#     plcc_image_check, and program code is called only via plcc_guard_call.
#
#   ./build-loader.sh [--upload]
# Environment: ARDUINO_CLI=path, BUILD_DIR=dir (default: build/loader).
#
# Flashing it changes the board's boot path: do it the first time with
# someone at the bench (README.md, "First flash").
set -e
here=$(cd "$(dirname "$0")" && pwd)
cli=${ARDUINO_CLI:-$HOME/.local/bin/arduino-cli}
[ -x "$cli" ] || cli=arduino-cli
fqbn=arduino:mbed_opta:opta
out=${BUILD_DIR:-$here/build/loader}
"$cli" compile -b $fqbn --output-dir "$out" "$here/loader"
python3 "$here/tools/check-loader.py" "$out/loader.ino.elf" "$here/loader/plcc_services.h"
[ "$1" = "--upload" ] || exit 0
port=$(ls -l /dev/serial/by-id/ | grep -i opta | grep -o 'ttyACM[0-9]*')
"$cli" upload -b $fqbn -p "/dev/$port" --input-dir "$out"
