#!/bin/sh
# SPDX-License-Identifier: MPL-2.0
# Rebuild the committed test modules: plcc (wasm32) + wasm-ld, and the symbol
# tables. Run after a codegen change that affects the wasm ABI.
#   PLCC=path/to/plcc WASM_LD=wasm-ld-21 sh build.sh
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../../.." && pwd)
plcc=${PLCC:-$root/target/debug/plcc}
ld=${WASM_LD:-$(command -v wasm-ld || command -v wasm-ld-21 || echo /usr/lib/llvm-21/bin/wasm-ld)}
cd "$here"   # fault sites name the file as given: keep it relative
for st in *.st; do
  b=${st%.st}
  n=$b
  ( ulimit -v $((4 * 1024 * 1024))
    "$plcc" compile "$st" -o "$b.o" --target wasm32-unknown-unknown -O2 \
      --emit-symbols "$b.symbols.json" )
  # The runner's link flags (docs/studio-wasm.md): no entry point, every
  # symbol exported (the runtime contract and the tag bases), runtime
  # symbols imported from `env`, and the function table exported (SINGLE
  # task triggers are function pointers).
  "$ld" --no-entry --export-dynamic --allow-undefined --export-table \
    -o "$b.wasm" "$b.o"
  rm -f "$b.o"
  echo "$n.wasm"
done
