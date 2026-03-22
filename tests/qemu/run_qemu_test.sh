#!/bin/bash
# SPDX-License-Identifier: MPL-2.0
# Compile ST program, link with ARM startup, run on QEMU Cortex-M3.
#
# Usage: ./tests/qemu/run_qemu_test.sh [program.st]
# Default: tests/fixtures/programs/blink.st

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
ST_FILE="${1:-$PROJECT_DIR/tests/fixtures/programs/blink.st}"
TMPDIR=$(mktemp -d)
trap "rm -rf $TMPDIR" EXIT

echo "=== plcc QEMU Cortex-M3 test ==="
echo "Source: $ST_FILE"
echo ""

# Step 1: Compile ST to ARM object
echo "[1/4] Compiling ST → ARM Cortex-M3 object..."
cargo run --quiet --manifest-path="$PROJECT_DIR/Cargo.toml" -- compile "$ST_FILE" \
    -o "$TMPDIR/program.o" --target thumbv7m-unknown-none-eabi

echo "      $(ls -la $TMPDIR/program.o | awk '{print $5}') bytes"

# Step 2: Compile startup.c for ARM
echo "[2/4] Compiling ARM startup code..."
arm-none-eabi-gcc -mcpu=cortex-m3 -mthumb -ffreestanding -nostdlib -c \
    "$SCRIPT_DIR/startup.c" -o "$TMPDIR/startup.o"

# Step 3: Link into ELF firmware
echo "[3/4] Linking firmware..."
arm-none-eabi-gcc -mcpu=cortex-m3 -mthumb -ffreestanding -nostdlib \
    -T "$SCRIPT_DIR/cortex_m3.ld" \
    "$TMPDIR/startup.o" "$TMPDIR/program.o" \
    -o "$TMPDIR/firmware.elf" \
    -lgcc

arm-none-eabi-size "$TMPDIR/firmware.elf"
echo ""

# Step 4: Run on QEMU with semihosting
echo "[4/4] Running on QEMU (lm3s6965evb, Cortex-M3)..."
echo "---"
timeout 10 qemu-system-arm \
    -machine lm3s6965evb \
    -nographic \
    -semihosting \
    -kernel "$TMPDIR/firmware.elf" > "$TMPDIR/output.txt" 2>&1 || true

cat "$TMPDIR/output.txt"
echo "---"

if grep -q "PASS" "$TMPDIR/output.txt"; then
    echo "QEMU test PASSED"
    exit 0
else
    echo "QEMU test FAILED"
    exit 1
fi
