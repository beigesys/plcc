#!/bin/sh
# SPDX-License-Identifier: MPL-2.0
# Build crates/plcc-image (its plain C ABI, no wasm-bindgen) into
# packages/plc-image/pkg/plcc_image.wasm. Needs the rustup target
# wasm32-unknown-unknown; wasm-opt (binaryen) is optional.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
# A cap, as for every cargo run in this repo (CLAUDE.md, "Memory discipline").
( ulimit -v $((6 * 1024 * 1024))
  cd "$root" && cargo build -p plcc-image --features wasm-abi --target wasm32-unknown-unknown --profile wasm-release )
mkdir -p "$here/pkg"
cp "$root/target/wasm32-unknown-unknown/wasm-release/plcc_image.wasm" "$here/pkg/plcc_image.wasm"
if command -v wasm-opt >/dev/null; then
  wasm-opt -Oz --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
    --enable-mutable-globals "$here/pkg/plcc_image.wasm" -o "$here/pkg/plcc_image.wasm"
fi
ls -l "$here/pkg/plcc_image.wasm"
