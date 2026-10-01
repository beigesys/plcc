#!/bin/sh
# SPDX-License-Identifier: MPL-2.0
# Build plcc's browser compiler (compiler/, a WASI command) into dist/:
#
#   dist/plcc-compiler.wasm.gz   the module, gzip -9 (decompressed in the browser)
#   dist/plcc-compiler.json      { version, bytes, gzip_bytes, sha256 } (cache key)
#
#   LLVM_WASI=<prefix> WASI_SDK=<wasi-sdk dir> sh build.sh
#
# LLVM_WASI is the LLVM-for-WASI prefix llvm/build.sh packages (lib/libLLVM*.a,
# include/llvm-c, include/llvm/Config); WASI_SDK is wasi-sdk 34 (its clang
# compiles llvm-sys's C wrapper; its sysroot has libc++abi and the WASI
# emulation libraries). Defaults: ~/.cache/plcc/llvm-21.1.8-wasi and
# ~/.cache/plcc/wasi-sdk-34.0-x86_64-linux. Needs: rustup target add wasm32-wasip1.
set -e
here=$(cd "$(dirname "$0")" && pwd)
LLVM_WASI=${LLVM_WASI:-$HOME/.cache/plcc/llvm-21.1.8-wasi}
WASI_SDK=${WASI_SDK:-$HOME/.cache/plcc/wasi-sdk-34.0-x86_64-linux}
[ -f "$LLVM_WASI/lib/libLLVMCore.a" ] || { echo "no LLVM for WASI at $LLVM_WASI (llvm/build.sh)" >&2; exit 1; }
[ -x "$WASI_SDK/bin/clang" ] || { echo "no wasi-sdk at $WASI_SDK" >&2; exit 1; }
SYS=$WASI_SDK/share/wasi-sysroot/lib/wasm32-wasip1
TOOLS=${TOOLS:-$here/target/tools}
mkdir -p "$TOOLS/bin" "$TOOLS/emu"
# llvm-sys runs `llvm-config` on the host; this stand-in describes the wasm build.
cat > "$TOOLS/bin/llvm-config" <<END
#!/bin/sh
for a in "\$@"; do
  case "\$a" in
    --version) echo 21.1.8 ;;
    --libdir) echo "$LLVM_WASI/lib" ;;
    --includedir) echo "$LLVM_WASI/include" ;;
    --cflags) echo "-I$LLVM_WASI/include" ;;
    --build-mode) echo MinSizeRel ;;
    --libnames) (cd "$LLVM_WASI/lib" && ls libLLVM*.a | tr '\n' ' '); echo ;;
    --system-libs) echo ;;
    --shared-mode) echo static ;;
  esac
done
END
chmod +x "$TOOLS/bin/llvm-config"
# llvm-sys asks for -lffi (LLVM is built without it): an empty archive.
[ -f "$TOOLS/emu/libffi.a" ] || "$WASI_SDK/bin/ar" rc "$TOOLS/emu/libffi.a"
# The WASI emulation libraries, in a directory of their own: putting the whole
# wasi-sdk sysroot on the search path would replace Rust's own libc.
cp "$SYS"/libwasi-emulated-*.a "$TOOLS/emu/"
export LLVM_SYS_211_PREFIX=$TOOLS
export LLVM_SYS_LIBCPP=c++
export CC_wasm32_wasip1="$WASI_SDK/bin/clang"
export CFLAGS_wasm32_wasip1="--target=wasm32-wasip1 --sysroot=$WASI_SDK/share/wasi-sysroot"
export AR_wasm32_wasip1="$WASI_SDK/bin/ar"
export CARGO_TARGET_WASM32_WASIP1_RUSTFLAGS="-L native=$SYS/noeh -L native=$TOOLS/emu -L native=$LLVM_WASI/lib \
 -l static=c++abi -l static=wasi-emulated-mman -l static=wasi-emulated-signal \
 -l static=wasi-emulated-process-clocks -l static=wasi-emulated-getpid \
 -C link-arg=-zstack-size=8388608 -C link-arg=--max-memory=2147483648"
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$here/target}
# A cap, as for every cargo run in this repo (CLAUDE.md, "Memory discipline").
( ulimit -v $((${CAP_GB:-12} * 1024 * 1024))
  cd "$here/compiler" && cargo build --release --target wasm32-wasip1 --locked )
wasm=$CARGO_TARGET_DIR/wasm32-wasip1/release/plcc-compiler-wasm.wasm
mkdir -p "$here/dist"
gzip -9 -n -c "$wasm" > "$here/dist/plcc-compiler.wasm.gz"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$here/compiler/Cargo.toml" | head -1)
sha=$(sha256sum "$wasm" | cut -d' ' -f1)
printf '{ "version": "%s", "bytes": %s, "gzip_bytes": %s, "sha256": "%s" }\n' \
  "$version" "$(wc -c < "$wasm")" "$(wc -c < "$here/dist/plcc-compiler.wasm.gz")" "$sha" \
  > "$here/dist/plcc-compiler.json"
cat "$here/dist/plcc-compiler.json"
