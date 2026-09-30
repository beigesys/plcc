#!/bin/sh
# SPDX-License-Identifier: MPL-2.0
# Link plcc Cortex-M objects with minild and with ld.lld at the same addresses
# and compare the flat text images byte for byte (docs/studio-wasm.md).
#   sh compare.sh <dir of .o files>
set -e
here=$(cd "$(dirname "$0")" && pwd)
objs=${1:?directory of plcc thumbv7em objects}
LLD=${LLD:-/usr/lib/llvm-21/bin/ld.lld}
OBJCOPY=${OBJCOPY:-/usr/lib/llvm-21/bin/llvm-objcopy}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
(cd "$here" && cargo build --release -q)
TEXT=0x08180040
DATA=0x24070000
IMPORTS="plcc_monotonic_ns=0x08040101 plcc_print=0x08040111 plcc_fault=0x08040121 __aeabi_unwind_cpp_pr0=0x08040131"
n=0
for f in __gtsf2 __subsf3 __mulsf3 __ltsf2 __floatsisf __divsf3 __addsf3 sqrt sin roundf log exp cos __unordsf2 __truncdfsf2 __gesf2 __fixsfsi; do
  n=$((n + 1)); IMPORTS="$IMPORTS $f=$(printf 0x%08x $((0x08040201 + 16 * n)))"
done
cat > "$work/blob.ld" <<END
SECTIONS {
  . = $TEXT;
  .text : { *(.text*) *(.rodata*) }
  . = $DATA;
  .data : { *(.data*) }
  .bss : { *(.bss*) *(COMMON) }
  /DISCARD/ : { *(.ARM.exidx*) *(.ARM.attributes) *(.comment) *(.note*) }
}
END
defsyms=$(for kv in $IMPORTS; do printf -- "--defsym=%s " "$kv"; done)
ok=0; bad=0
for o in "$objs"/*.o; do
  "$here/target/release/minild" "$o" "$work/mine.bin" $TEXT $DATA $IMPORTS > /dev/null || { bad=$((bad + 1)); continue; }
  # -O0: no string merging, so both place .rodata.str* the same way.
  "$LLD" -O0 -T "$work/blob.ld" $defsyms -o "$work/ref.elf" "$o" 2>/dev/null
  "$OBJCOPY" -O binary -j .text "$work/ref.elf" "$work/ref.bin"
  size=$(stat -c %s "$work/ref.bin")
  if cmp -s -n "$size" "$work/ref.bin" "$work/mine.bin"; then ok=$((ok + 1)); else echo "DIFF: $o"; bad=$((bad + 1)); fi
done
echo "identical text images: $ok, different: $bad"
