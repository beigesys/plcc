#!/bin/sh
# SPDX-License-Identifier: MPL-2.0
# Rebuild the committed fixtures: a plcc object for the Opta and the image the
# native `plcc image` makes of it (the WebAssembly build must match it byte
# for byte). Run after a codegen or image-format change.
#   PLCC=path/to/plcc sh build.sh
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../../.." && pwd)
plcc=${PLCC:-$root/target/debug/plcc}
cd "$here"
cp "$root/crates/plcc-cli/tests/data/image/chase.st" chase.st
( ulimit -v $((4 * 1024 * 1024))
  "$plcc" compile chase.st -o chase.o --device arduino-opta -O2
  "$plcc" image chase.o --device arduino-opta -o chase.img )
rm chase.st
ls -l chase.o chase.img
