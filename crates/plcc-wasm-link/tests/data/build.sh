#!/bin/sh
# SPDX-License-Identifier: MPL-2.0
# Rebuild the test objects and, for each, wasm-ld's output (`<name>.ld.wasm`,
# the reference the tests compare against). Run after a codegen change that
# affects the wasm object format:
#   PLCC=path/to/plcc WASM_LD=wasm-ld-21 LLC=llc-21 sh build.sh
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../../.." && pwd)
plcc=${PLCC:-$root/target/debug/plcc}
ld=${WASM_LD:-$(command -v wasm-ld-21 || command -v wasm-ld || echo /usr/lib/llvm-21/bin/wasm-ld)}
llc=${LLC:-$(command -v llc-21 || echo /usr/lib/llvm-21/bin/llc)}
cd "$here"
rm -f ./*.o ./*.wasm

compile() { # name, then plcc compile arguments
  name=$1
  shift
  ( ulimit -v $((4 * 1024 * 1024))
    "$plcc" compile "$@" -o "$name.o" --target wasm32-unknown-unknown >/dev/null )
}

# Paths relative to the repository root: fault sites name the file as given.
cd "$root"
for b in seal_in ton div_zero; do
  for o in 0 2; do
    compile "$here/${b}_O$o" "packages/plc-wasm/test/fixtures/$b.st" -O$o
  done
done
compile "$here/stdlib_math_O2" tests/fixtures/codegen/stdlib_math.st -O2
compile "$here/opta_io_O2" tests/fixtures/l5x/opta_io.L5X --io-map tests/fixtures/l5x/opta_io.toml -O2
cd "$here"
for ll in indirect ctor undef_data; do
  "$llc" -filetype=obj "$ll.ll" -o "$ll.o"
done

for o in *.o; do
  b=${o%.o}
  case $b in ctor | undef_data) continue ;; esac
  "$ld" --no-entry --export-dynamic --allow-undefined --export-table -o "$b.ld.wasm" "$o"
  echo "$b"
done
