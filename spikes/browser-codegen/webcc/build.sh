#!/bin/sh
# SPDX-License-Identifier: MPL-2.0
# Spike: build plcc's LLVM code generator as a WASI program (wasm32-wasip1)
# against the libraries llvm-wasi/build.sh produced.
#   WORK=/same/scratch sh build.sh [check|build]
# Needs: rustup target add wasm32-wasip1. Run:
#   node ../wasi-run.mjs target/wasm32-wasip1/release/webcc.wasm <dir> <dir>/prog.st <dir>/prog.o thumbv7em-none-eabi
here=$(cd "$(dirname "$0")" && pwd)
WORK=${WORK:?set WORK to the llvm-wasi scratch directory}
W=$WORK/wasi-sdk-34.0-x86_64-linux
SYS=$W/share/wasi-sysroot/lib/wasm32-wasip1
# llvm-sys runs `llvm-config` on the host; this stand-in describes the wasm build.
mkdir -p "$WORK/prefix/bin"
cat > "$WORK/prefix/bin/llvm-config" <<END
#!/bin/sh
for a in "\$@"; do
  case "\$a" in
    --version) echo 21.1.8 ;;
    --libdir) echo "$WORK/llvm-build/lib" ;;
    --cflags) echo "-I$WORK/llvm-project-21.1.8.src/llvm/include -I$WORK/llvm-build/include" ;;
    --build-mode) echo MinSizeRel ;;
    --libnames) (cd "$WORK/llvm-build/lib" && ls libLLVM*.a | tr '\n' ' '); echo ;;
    --system-libs) echo ;;
    --shared-mode) echo static ;;
  esac
done
END
chmod +x "$WORK/prefix/bin/llvm-config"
# llvm-sys asks for -lffi (LLVM is built without it): an empty archive.
[ -f "$WORK/llvm-build/lib/libffi.a" ] || "$W/bin/ar" rc "$WORK/llvm-build/lib/libffi.a"
# The WASI emulation libraries, in a directory of their own: putting the whole
# wasi-sdk sysroot on the search path would replace Rust's own libc.
mkdir -p "$WORK/emu" && cp "$SYS"/libwasi-emulated-*.a "$WORK/emu/"
export LLVM_SYS_211_PREFIX=$WORK/prefix
export LLVM_SYS_LIBCPP=c++
export CC_wasm32_wasip1="$W/bin/clang"
export CFLAGS_wasm32_wasip1="--target=wasm32-wasip1 --sysroot=$W/share/wasi-sysroot"
export AR_wasm32_wasip1="$W/bin/ar"
export CARGO_TARGET_WASM32_WASIP1_RUSTFLAGS="-L native=$SYS/noeh -L native=$WORK/emu -l static=c++abi \
 -l static=wasi-emulated-mman -l static=wasi-emulated-signal -l static=wasi-emulated-process-clocks \
 -l static=wasi-emulated-getpid -C link-arg=-zstack-size=8388608 -C link-arg=--max-memory=1073741824"
ulimit -v $((6 * 1024 * 1024))
cd "$here" && exec cargo "${1:-build}" --release --target wasm32-wasip1
