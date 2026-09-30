#!/bin/sh
# SPDX-License-Identifier: MPL-2.0
# Build crates/plcc-wasm into packages/plcc-wasm/pkg (wasm-bindgen, ES module).
# Needs: rustup target wasm32-unknown-unknown, wasm-bindgen-cli matching the
# wasm-bindgen crate version in Cargo.lock, and optionally wasm-opt (binaryen).
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
# A cap, as for every cargo run in this repo (CLAUDE.md, "Memory discipline").
( ulimit -v $((6 * 1024 * 1024))
  cd "$root" && cargo build -p plcc-wasm --target wasm32-unknown-unknown --profile wasm-release )
wasm=$root/target/wasm32-unknown-unknown/wasm-release/plcc_wasm.wasm
rm -rf "$here/pkg"
wasm-bindgen --target web --out-dir "$here/pkg" --out-name plcc_wasm "$wasm"
if command -v wasm-opt >/dev/null; then
  wasm-opt -Oz --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
    --enable-mutable-globals --enable-reference-types --enable-multivalue \
    "$here/pkg/plcc_wasm_bg.wasm" -o "$here/pkg/plcc_wasm_bg.wasm"
fi
ls -l "$here/pkg/plcc_wasm_bg.wasm"
