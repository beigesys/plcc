// SPDX-License-Identifier: MPL-2.0
//
// The compiler in a Web Worker, so a 30 MB module compiles and LLVM runs off
// the UI thread. In the worker:
//
//   import { serveCompiler } from "@plcc/plcc-compiler-wasm/worker";
//   serveCompiler(self);
//
// Requests: `load` (fetch / cache / compile the module, with `progress`
// events), `compile` (one build; loads first if needed). Replies carry the
// request's `id`.

import { type CompileRequest, type CompileResult, type LoadedCompiler, type LoadProgress, compile, loadCompiler } from "./index";

export type CompilerRequest =
  | { type: "load"; id: number; baseUrl: string }
  | { type: "compile"; id: number; baseUrl: string; request: CompileRequest };

export type CompilerEvent =
  | { type: "progress"; progress: LoadProgress }
  | { type: "loaded"; id: number; source: LoadedCompiler["source"]; ms: number; bytes: number; version: string }
  | { type: "result"; id: number; result: CompileResult; ms: number }
  | { type: "error"; id: number; message: string };

export interface CompilerPort {
  postMessage(message: CompilerEvent, transfer?: Transferable[]): void;
  addEventListener(type: "message", listener: (e: { data: CompilerRequest }) => void): void;
}

export function serveCompiler(port: CompilerPort): (req: CompilerRequest) => Promise<void> {
  let loading: Promise<LoadedCompiler> | null = null;
  const load = (baseUrl: string) => {
    loading ??= loadCompiler({ baseUrl, onProgress: (progress) => port.postMessage({ type: "progress", progress }) }).catch(
      (e: unknown) => {
        loading = null;
        throw e;
      },
    );
    return loading;
  };
  const handle = async (req: CompilerRequest) => {
    try {
      const loaded = await load(req.baseUrl);
      if (req.type === "load") {
        port.postMessage({
          type: "loaded",
          id: req.id,
          source: loaded.source,
          ms: loaded.ms,
          bytes: loaded.manifest.bytes,
          version: loaded.manifest.version,
        });
        return;
      }
      const t0 = performance.now();
      const result = await compile(loaded.module, req.request);
      port.postMessage({ type: "result", id: req.id, result, ms: performance.now() - t0 });
    } catch (e) {
      port.postMessage({ type: "error", id: req.id, message: e instanceof Error ? e.message : String(e) });
    }
  };
  port.addEventListener("message", (e) => void handle(e.data));
  return handle;
}
