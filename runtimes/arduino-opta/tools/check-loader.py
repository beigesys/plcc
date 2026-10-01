#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0
"""Static checks on a linked Opta program-image runtime (loader.ino.elf),
run by build-loader.sh before anyone flashes it. Needs llvm-readelf,
llvm-objdump and llvm-nm (LLVM 21 by default; LLVM_BIN=dir to override).

  check-loader.py <loader.ino.elf> <plcc_services.h>

Exit status 0 when every check passes; each check prints one line.
"""
import os
import re
import struct
import subprocess
import sys

LLVM = os.environ.get("LLVM_BIN", "/usr/lib/llvm-21/bin")
SLOT, SLOT_END = 0x08180000, 0x08200000
FLASH, APP = 0x08000000, 0x08040000
WINDOW, WINDOW_END = 0x20010000, 0x20020000

elf, services_h = sys.argv[1], sys.argv[2]
failures = 0


def run(*args):
    return subprocess.run([f"{LLVM}/{args[0]}", *args[1:]], capture_output=True, text=True, check=True).stdout


def check(ok, what):
    global failures
    print(("ok   " if ok else "FAIL ") + what)
    if not ok:
        failures += 1


# 1. Memory: the firmware stays out of the program slot and the RAM window.
segs = []
for line in run("llvm-readelf", "-l", "-W", elf).splitlines():
    p = line.split()
    if p and p[0] == "LOAD":
        segs.append((int(p[2], 16), int(p[3], 16), int(p[4], 16), int(p[5], 16)))  # vaddr, paddr, filesz, memsz
flash_end = max(pa + fs for va, pa, fs, ms in segs if FLASH <= pa < SLOT_END and fs)
check(flash_end <= SLOT, f"firmware flash ends at {flash_end:#010x}, below the program slot {SLOT:#010x}")
check(min(pa for va, pa, fs, ms in segs if fs) >= APP, "nothing is loaded below the application start 0x08040000")
secs = []
for line in run("llvm-readelf", "-S", "-W", elf).splitlines():
    m = re.match(r"\s*\[\s*\d+\]\s+(\S+)\s+(\S+)\s+([0-9a-f]+)\s+[0-9a-f]+\s+([0-9a-f]+)\s+\S+\s+(\S*)", line)
    if m and "A" in m.group(5):
        secs.append((m.group(1), int(m.group(3), 16), int(m.group(4), 16)))
bad = [n for n, a, s in secs if s and a < WINDOW_END and WINDOW < a + s]
check(not bad, f"no section in the program RAM window {WINDOW:#x}-{WINDOW_END:#x} {bad if bad else ''}")
bad = [n for n, a, s in secs if s and a < SLOT_END and SLOT < a + s]
check(not bad, f"no section in the program slot {bad if bad else ''}")

# 2. The service table: every entry is the Thumb address of the symbol it names.
syms = {}
for line in run("llvm-nm", elf).splitlines():
    p = line.split()
    if len(p) == 3:
        syms.setdefault(p[2], int(p[0], 16))
names = re.findall(r"PLCC_SERVICE\((\d+), (\w+)\)", open(services_h).read())
count = int(re.search(r"#define PLCC_SERVICE_COUNT (\d+)u", open(services_h).read()).group(1))
table = syms["plcc_service_table"]
data = open(elf, "rb").read()
off = None
for line in run("llvm-readelf", "-S", "-W", elf).splitlines():
    m = re.search(r"\]\s+(\S+)\s+\S+\s+([0-9a-f]+)\s+([0-9a-f]+)\s+([0-9a-f]+)", line)
    if m and int(m.group(2), 16) <= table < int(m.group(2), 16) + int(m.group(4), 16):
        off = int(m.group(3), 16) + table - int(m.group(2), 16)
magic, n = struct.unpack_from("<II", data, off)
bad = []
for idx, name in names:
    v = struct.unpack_from("<I", data, off + 8 + 4 * int(idx))[0]
    s = syms.get(name)
    if not (v & 1 and s is not None and (s & ~1) == (v & ~1)):
        bad.append(f"{idx}:{name}")
check(magic == 0x53434C50 and n == count == len(names), f"service table at {table:#010x}: magic PLCS, {n} entries")
check(not bad, f"every service entry is its symbol's Thumb address {bad if bad else ''}")

# 3. Boot order and the only way into the slot, from the disassembly.
dis = run("llvm-objdump", "-d", "-C", "--no-show-raw-insn", elf)
funcs = {}
cur = None
for line in dis.splitlines():
    m = re.match(r"^([0-9a-f]+) <(.+)>:$", line)
    if m:
        cur = m.group(2)
        funcs[cur] = []
    elif cur and "\t" in line:
        funcs[cur].append(line)


def calls(fn):
    out = []
    for l in funcs.get(fn, []):
        m = re.search(r"\bbl\s+0x[0-9a-f]+ <([^>+]+)", l) or re.search(r"\bb(?:\.w)?\s+0x[0-9a-f]+ <([^>+]+)>", l)
        if m:
            out.append(m.group(1))
    return out


main_calls = calls("main")
def first(lst, pred):
    return next((i for i, c in enumerate(lst) if pred(c)), None)
usb = first(main_calls, lambda c: "USBSerial::begin" in c)
setup = first(main_calls, lambda c: c == "setup")
check(usb is not None and setup is not None and usb < setup,
      f"main(): USB CDC begins before setup() ({' -> '.join(main_calls[: (setup or 0) + 1])})")

callers_prepare = [f for f in funcs if "plcc_image_prepare" in calls(f)]
check(len(callers_prepare) == 1, f"plcc_image_prepare is called from one function only ({callers_prepare})")
# The slot is read only inside a guarded call (an ECC error on a half-written
# flash word is a bus fault): plcc_image_check is called from call_check only,
# which check_slot passes to plcc_guard_call.
callers_check = sorted(f for f in funcs if "plcc_image_check" in calls(f))
check(callers_check == ["call_check"], f"plcc_image_check is called only from call_check ({callers_check})")
call_check = syms.get("call_check")


def literals(fn):
    return [int(m.group(1), 16) for l in funcs.get(fn, []) if (m := re.search(r":\s+[0-9a-f ]+\s+\.word\s+0x([0-9a-f]+)", l))]


check(call_check is not None and (call_check | 1) in literals("check_slot") and "plcc_guard_call" in calls("check_slot"),
      "check_slot runs call_check through plcc_guard_call")
for f in callers_prepare:
    seq = calls(f)
    i_check = first(seq, lambda c: c == "check_slot")
    i_prep = first(seq, lambda c: c == "plcc_image_prepare")
    check(i_check is not None and i_prep is not None and i_check < i_prep,
          f"{f}: check_slot (the guarded plcc_image_check) is called before plcc_image_prepare")
    # The result is tested right after the call.
    body = funcs[f]
    k = next(i for i, l in enumerate(body) if re.search(r"\bbl\s+\S+ <check_slot>", l))
    nxt = " ".join(body[k + 1:k + 4])
    check(re.search(r"\bcbz\s+r0|\bcbnz\s+r0|\bcmp\s+r0, #0", nxt) is not None,
          f"{f}: the result of check_slot is tested before prepare")
    i_guard = first(seq, lambda c: c == "plcc_guard_call")
    check(i_guard is not None and i_prep < i_guard, f"{f}: get_app is called through plcc_guard_call after prepare")

setup_calls = calls("setup")
i_install = first(setup_calls, lambda c: c == "plcc_guard_install")
i_start = first(setup_calls, lambda c: c in callers_prepare or c == "plcc_image_prepare" or c == "plcc_guard_call")
check(i_install is not None and i_start is not None and i_install < i_start,
      "setup(): the fault handlers are installed before the program is started")


# No direct branch into the slot anywhere in the firmware.
direct = [l.strip() for f in funcs for l in funcs[f]
          if (m := re.search(r"\bb(?:l|lx)?(?:\.w)?\s+0x([0-9a-f]+)", l)) and SLOT <= int(m.group(1), 16) < SLOT_END]
check(not direct, f"no direct branch into the program slot {direct[:3] if direct else ''}")

# Indirect calls into program code happen in the guarded call wrappers only.
wrappers = [f for f in funcs if re.match(r"call_(get_app|init|task|single)", f)]
check(len(wrappers) >= 3, f"the program is entered through the guarded wrappers {wrappers}")
guard_users = sorted({f for f in funcs if "plcc_guard_call" in calls(f)})
print(f"info functions calling plcc_guard_call: {guard_users}")

print(f"{failures} failure(s)")
sys.exit(1 if failures else 0)
