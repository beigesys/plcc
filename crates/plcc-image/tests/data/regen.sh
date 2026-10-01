#!/bin/sh
# SPDX-License-Identifier: MPL-2.0
# Reassemble the test objects in this directory (llvm-mc from LLVM 21):
#   relocs.o        relocs.s: every supported relocation type
#   errors-<n>.o    errors.s with CASE=n: objects the linker must refuse (10: hard-float)
# The ld.lld golden outputs (relocs.lld.*.bin) are written by the test:
#   PLCC_UPDATE_GOLDEN=1 cargo test -p plcc-image --test link
set -e
here=$(cd "$(dirname "$0")" && pwd)
LLVM=${LLVM:-/usr/lib/llvm-21/bin}
cd "$here"
"$LLVM/llvm-mc" -triple=thumbv7em-none-eabi -filetype=obj relocs.s -o relocs.o
for n in 1 2 3 4 5 6 7 8 9 10; do
  "$LLVM/llvm-mc" -triple=thumbv7em-none-eabi -filetype=obj --defsym CASE=$n errors.s -o errors-$n.o
done
