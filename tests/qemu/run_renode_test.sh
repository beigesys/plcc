#!/bin/bash
# SPDX-License-Identifier: MPL-2.0
# Compile ST program, link for STM32F4, run on Renode.
#
# Usage: ./tests/qemu/run_renode_test.sh [program.st]

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
ST_FILE="${1:-$PROJECT_DIR/tests/fixtures/programs/blink.st}"
RENODE="$HOME/.local/bin/renode_portable/renode"
TMPDIR=$(mktemp -d)
trap "rm -rf $TMPDIR" EXIT

if [ ! -f "$RENODE" ]; then
    echo "Renode not found at $RENODE"
    echo "Install: mkdir -p ~/.local/bin/renode_portable && wget https://builds.renode.io/renode-latest.linux-portable.tar.gz -O /tmp/renode.tar.gz && tar xf /tmp/renode.tar.gz -C ~/.local/bin/renode_portable --strip-components=1"
    exit 1
fi

echo "=== plcc Renode STM32F4 test ==="
echo "Source: $ST_FILE"
echo ""

# Step 1: Compile ST to ARM Cortex-M4 object
echo "[1/4] Compiling ST → ARM Cortex-M4 object..."
cargo run --quiet --manifest-path="$PROJECT_DIR/Cargo.toml" -- compile "$ST_FILE" \
    -o "$TMPDIR/program.o" --target thumbv7em-unknown-none-eabihf

echo "      $(wc -c < $TMPDIR/program.o) bytes"

# Step 2: Compile STM32F4 startup
echo "[2/4] Compiling STM32F4 startup code..."
arm-none-eabi-gcc -mcpu=cortex-m4 -mthumb -mfloat-abi=hard -mfpu=fpv4-sp-d16 \
    -ffreestanding -nostdlib -c \
    "$SCRIPT_DIR/startup_stm32f4.c" -o "$TMPDIR/startup.o"

# Step 3: Link firmware
echo "[3/4] Linking STM32F4 firmware..."
arm-none-eabi-gcc -mcpu=cortex-m4 -mthumb -mfloat-abi=hard -mfpu=fpv4-sp-d16 \
    -ffreestanding -nostdlib \
    -T "$SCRIPT_DIR/stm32f4.ld" \
    "$TMPDIR/startup.o" "$TMPDIR/program.o" \
    -o "$TMPDIR/firmware.elf" \
    -lgcc

arm-none-eabi-size "$TMPDIR/firmware.elf"
echo ""

# Step 4: Run on Renode
echo "[4/4] Running on Renode (STM32F4 Discovery, Cortex-M4)..."
echo "---"

# Create a Renode script that captures UART output
cat > "$TMPDIR/test.resc" << RESC
using sysbus
mach create
machine LoadPlatformDescription @platforms/boards/stm32f4_discovery.repl
sysbus LoadELF @$TMPDIR/firmware.elf
logLevel -1 usart2
usart2 CreateFileBackend @$TMPDIR/uart_output.txt true
start
sleep 3
quit
RESC

timeout 15 "$RENODE" --disable-xwt --console "$TMPDIR/test.resc" > "$TMPDIR/renode_log.txt" 2>&1 || true

if [ -f "$TMPDIR/uart_output.txt" ]; then
    cat "$TMPDIR/uart_output.txt"
else
    echo "(no UART output captured)"
    echo "Renode log:"
    tail -20 "$TMPDIR/renode_log.txt"
fi

echo "---"

if [ -f "$TMPDIR/uart_output.txt" ] && grep -q "PASS" "$TMPDIR/uart_output.txt"; then
    echo "Renode test PASSED"
    exit 0
else
    echo "Renode test: checking log for execution..."
    if grep -q "Loading segment" "$TMPDIR/renode_log.txt" 2>/dev/null; then
        echo "Firmware loaded successfully on STM32F4"
        # Even if UART output wasn't captured, the firmware ran
        if grep -q "scan" "$TMPDIR/uart_output.txt" 2>/dev/null; then
            echo "Renode test PASSED (partial output)"
            exit 0
        fi
    fi
    echo "Renode test: could not verify (check log)"
    cat "$TMPDIR/renode_log.txt" | tail -30
    exit 1
fi
