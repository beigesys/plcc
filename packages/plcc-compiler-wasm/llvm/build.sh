#!/bin/sh
# SPDX-License-Identifier: MPL-2.0
# LLVM 21.1.8 libraries for wasm32-wasip1 with only the ARM and WebAssembly
# backends: what inkwell / plcc-codegen link against in the browser compiler
# (docs/studio-wasm.md).
#
#   WORK=/some/scratch sh build.sh            # build into $WORK
#   WORK=/some/scratch sh build.sh package    # and pack $WORK/llvm-21.1.8-wasi.tar.xz
#
# Downloads wasi-sdk 34 and the LLVM 21.1.8 source into $WORK, applies
# llvm-21.1.8-wasi.patch (YoWASP's "Conditionalize use of POSIX features missing
# on WASI/WebAssembly", Apache-2.0 WITH LLVM-exception like LLVM itself,
# rebased onto 21.1.8 plus two fixes), configures with the host's llvm-tblgen
# (LLVM 21) and builds the libraries. ~1.2k compile steps; ~60 min on 3 cores.
#
# The package is the prefix ../build.sh wants (LLVM_WASI): lib/libLLVM*.a and
# the headers llvm-sys compiles against (include/llvm-c, include/llvm/Config).
set -e
here=$(cd "$(dirname "$0")" && pwd)
WORK=${WORK:?set WORK to a scratch directory}
TBLGEN=${TBLGEN:-/usr/lib/llvm-21/bin/llvm-tblgen}
mkdir -p "$WORK" && cd "$WORK"
W=$WORK/wasi-sdk-34.0-x86_64-linux
[ -d "$W" ] || curl -sSL https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-34/wasi-sdk-34.0-x86_64-linux.tar.gz | tar xzf -
if [ ! -d llvm-project-21.1.8.src ]; then
  curl -sSL https://github.com/llvm/llvm-project/releases/download/llvmorg-21.1.8/llvm-project-21.1.8.src.tar.xz \
    | tar xJf - llvm-project-21.1.8.src/llvm llvm-project-21.1.8.src/cmake llvm-project-21.1.8.src/third-party
  patch -d llvm-project-21.1.8.src -p1 < "$here/llvm-21.1.8-wasi.patch"
fi
EMU="-D_WASI_EMULATED_MMAN -D_WASI_EMULATED_SIGNAL -D_WASI_EMULATED_PROCESS_CLOCKS -D_WASI_EMULATED_GETPID"
cat > Toolchain-WASI.cmake <<END
set(CMAKE_SYSTEM_NAME WASI)
set(CMAKE_SYSTEM_VERSION 1)
set(CMAKE_SYSTEM_PROCESSOR wasm32)
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)
set(CMAKE_C_COMPILER $W/bin/clang)
set(CMAKE_C_COMPILER_TARGET wasm32-wasip1)
set(CMAKE_CXX_COMPILER $W/bin/clang++)
set(CMAKE_CXX_COMPILER_TARGET wasm32-wasip1)
set(CMAKE_LINKER $W/bin/wasm-ld)
set(CMAKE_AR $W/bin/ar)
set(CMAKE_RANLIB $W/bin/ranlib)
set(CMAKE_C_FLAGS "--sysroot $W/share/wasi-sysroot $EMU")
set(CMAKE_CXX_FLAGS "--sysroot $W/share/wasi-sysroot $EMU")
END
if [ ! -f llvm-build/build.ninja ]; then
cmake -G Ninja -B llvm-build -S llvm-project-21.1.8.src/llvm \
  -DCMAKE_TOOLCHAIN_FILE="$WORK/Toolchain-WASI.cmake" \
  -DLLVM_TABLEGEN="$TBLGEN" -DLLVM_NATIVE_TOOL_DIR="$(dirname "$TBLGEN")" \
  -DCMAKE_BUILD_TYPE=MinSizeRel -DLLVM_ENABLE_ASSERTIONS=OFF \
  -DLLVM_BUILD_SHARED_LIBS=OFF -DLLVM_ENABLE_PIC=OFF -DLLVM_BUILD_STATIC=ON -DLLVM_ENABLE_THREADS=OFF \
  -DLLVM_BUILD_TOOLS=OFF -DLLVM_BUILD_UTILS=OFF -DLLVM_INCLUDE_UTILS=OFF -DLLVM_INCLUDE_RUNTIMES=OFF \
  -DLLVM_INCLUDE_EXAMPLES=OFF -DLLVM_INCLUDE_TESTS=OFF -DLLVM_INCLUDE_BENCHMARKS=OFF -DLLVM_INCLUDE_DOCS=OFF \
  -DLLVM_ENABLE_ZLIB=OFF -DLLVM_ENABLE_ZSTD=OFF -DLLVM_ENABLE_LIBXML2=OFF -DLLVM_ENABLE_LIBEDIT=OFF \
  -DLLVM_ENABLE_TERMINFO=OFF -DLLVM_ENABLE_LIBPFM=OFF -DLLVM_ENABLE_BACKTRACES=OFF \
  -DLLVM_ENABLE_CRASH_OVERRIDES=OFF -DLLVM_ENABLE_UNWIND_TABLES=OFF \
  -DLLVM_TARGETS_TO_BUILD="ARM;WebAssembly" \
  -DLLVM_HOST_TRIPLE=wasm32-wasip1 -DLLVM_DEFAULT_TARGET_TRIPLE=thumbv7em-none-eabi
fi
# A cap per compiler process (CLAUDE.md, "Memory discipline").
( ulimit -v $((4 * 1024 * 1024))
  ninja -C llvm-build -j"${JOBS:-3}" \
    LLVMARMCodeGen LLVMARMAsmParser LLVMARMDisassembler LLVMARMDesc LLVMARMInfo LLVMARMUtils \
    LLVMWebAssemblyCodeGen LLVMWebAssemblyAsmParser LLVMWebAssemblyDisassembler LLVMWebAssemblyDesc \
    LLVMWebAssemblyInfo LLVMWebAssemblyUtils \
    LLVMPasses LLVMBitWriter LLVMIRReader LLVMLinker LLVMExecutionEngine LLVMMCJIT LLVMInterpreter )

[ "$1" = package ] || exit 0
P=$WORK/llvm-21.1.8-wasi
rm -rf "$P" && mkdir -p "$P/lib" "$P/include/llvm"
cp llvm-build/lib/libLLVM*.a "$P/lib/"
cp -r llvm-project-21.1.8.src/llvm/include/llvm-c "$P/include/"
cp -r llvm-project-21.1.8.src/llvm/include/llvm/Config "$P/include/llvm/"
cp -r llvm-build/include/llvm/Config/. "$P/include/llvm/Config/"
cp "$here/llvm-21.1.8-wasi.patch" "$P/"
tar -C "$WORK" -cJf "$WORK/llvm-21.1.8-wasi.tar.xz" llvm-21.1.8-wasi
ls -l "$WORK/llvm-21.1.8-wasi.tar.xz"
