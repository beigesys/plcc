#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0
"""Modbus RTU client for plcc water treatment PLC demo.

Connects to the Renode-emulated STM32F4 via UART PTY and reads/writes
PLC registers. Prints a live dashboard.

Usage:
    pip install pymodbus
    python3 tests/qemu/modbus_client.py /tmp/plcc_modbus_pty

Or with socat bridge:
    socat -d -d pty,raw,echo=0 pty,raw,echo=0
    # Use one PTY for Renode, the other for this client
"""

import sys
import time
import struct

try:
    from pymodbus.client import ModbusSerialClient
    from pymodbus.exceptions import ModbusException
except ImportError:
    print("Install pymodbus: pip install pymodbus")
    sys.exit(1)

PORT = sys.argv[1] if len(sys.argv) > 1 else "/tmp/plcc_modbus_pty"
SLAVE_ID = 1

REG_NAMES = {
    0: "mode",
    1: "raw_tank_sp",
    2: "clean_tank_sp",
    3: "inlet_speed",
    4: "transfer_speed",
    5: "outlet_speed",
}

INPUT_NAMES = {
    100: "cycle_count",
    101: "raw_level_%",
    102: "clean_level_%",
    103: "inlet_pump",
    104: "xfer_pump",
    105: "outlet_pump",
    106: "system_ok",
    107: "alarm_count",
    108: "total_flow",
}

def main():
    print(f"Connecting to Modbus RTU at {PORT} (slave {SLAVE_ID})...")
    client = ModbusSerialClient(PORT, baudrate=9600, timeout=2, stopbits=1, bytesize=8, parity='N')

    if not client.connect():
        print(f"Failed to connect to {PORT}")
        sys.exit(1)

    print("Connected!\n")

    # Set mode to RUN
    print("Setting mode = 1 (RUN)...")
    client.write_register(0, 1, slave=SLAVE_ID)
    time.sleep(0.5)

    # Set setpoints
    print("Setting raw_tank_sp=50, clean_tank_sp=70")
    client.write_register(1, 50, slave=SLAVE_ID)
    client.write_register(2, 70, slave=SLAVE_ID)
    print("Setting speeds: inlet=60, transfer=40, outlet=30")
    client.write_register(3, 60, slave=SLAVE_ID)
    client.write_register(4, 40, slave=SLAVE_ID)
    client.write_register(5, 30, slave=SLAVE_ID)
    print()

    # Poll loop
    try:
        while True:
            print("\033[2J\033[H")  # Clear screen
            print("=" * 60)
            print("  plcc Water Treatment PLC — Modbus RTU Dashboard")
            print("=" * 60)

            # Read holding registers
            hr = client.read_holding_registers(0, 6, slave=SLAVE_ID)
            if not hr.isError():
                print("\n  SETPOINTS (holding registers):")
                for i, val in enumerate(hr.registers):
                    name = REG_NAMES.get(i, f"reg_{i}")
                    if i == 0:
                        modes = {0: "STOP", 1: "RUN", 2: "FLUSH", 3: "ALARM"}
                        print(f"    {name:20s} = {modes.get(val, str(val))}")
                    else:
                        print(f"    {name:20s} = {val}")

            # Read input registers
            ir = client.read_holding_registers(100, 9, slave=SLAVE_ID)
            if not ir.isError():
                print("\n  PROCESS VALUES (input registers):")
                for i, val in enumerate(ir.registers):
                    addr = 100 + i
                    name = INPUT_NAMES.get(addr, f"reg_{addr}")
                    if addr in (103, 104, 105, 106):
                        print(f"    {name:20s} = {'ON' if val else 'OFF'}")
                    else:
                        print(f"    {name:20s} = {val}")

            print("\n" + "-" * 60)
            print("  Press Ctrl+C to exit")
            time.sleep(1)

    except KeyboardInterrupt:
        print("\n\nStopping...")
    finally:
        client.close()

if __name__ == "__main__":
    main()
