// SPDX-License-Identifier: MPL-2.0
// plcc's compiler (LLVM in WebAssembly, ~10 MB to download, cached) in its own
// worker: loaded on the first Simulate or Download, one fresh instance per build.

import { serveCompiler, type CompilerPort } from '@plcc/plcc-compiler-wasm/worker'

serveCompiler(self as unknown as CompilerPort)
