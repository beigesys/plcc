// SPDX-License-Identifier: MPL-2.0
//
// plcc's compiler in the browser: the front end, LLVM code generation (Arm and
// WebAssembly backends) and a single-object wasm linker, compiled to one WASI
// module (../compiler, built by ../build.sh). It is large (≈30 MB, ≈10 MB
// gzip), so it is fetched only when a build is needed, cached by its SHA-256,
// and run in a Web Worker (./worker.ts).

import { runWasi } from "./wasi";

export { runWasi, WasiExit, type WasiRun } from "./wasi";

/** `plcc_driver::Diagnostic` (see @plcc/plcc-wasm for the full type). */
export interface Diagnostic {
  file: string | null;
  severity: "error" | "warning" | "advice";
  stage: string;
  code: string | null;
  message: string;
  help: string | null;
  span: { start: Position; end: Position; message: string | null } | null;
  labels: unknown[];
  /** For a ladder model input: where in the model. */
  ladder?: { pou?: string; routine?: string; rung?: number; element?: number; operand?: number; tag?: string };
}

export interface Position {
  line: number;
  col: number;
  offset: number;
  utf16: number;
}

export interface CompileRequest {
  /** path → text: .st, PLCopen .xml, .L5X, TwinCAT files, a ladder model .json. */
  files: Record<string, string>;
  entry?: string[];
  io_map?: string;
  stdlib?: "bundled-st" | "none";
  /** A device manifest (TOML text): target triple, CPU, features, float ABI, image sizes. */
  device?: string;
  /** LLVM triple; default: the device's, else wasm32-unknown-unknown. */
  target?: string;
  cpu?: string;
  features?: string[];
  float_abi?: "soft" | "softfp" | "hard";
  image?: Partial<Record<"I" | "Q" | "M", number>>;
  opt_level?: 0 | 1 | 2 | 3;
  task_interval?: string;
  /** wasm32: also link the object into a module for @plcc/plc-wasm. */
  link?: boolean;
}

export interface CompileResult {
  ok: boolean;
  diagnostics: Diagnostic[];
  /** `--emit-symbols` JSON (the runtime contract), for @plcc/plc-wasm and HMIs. */
  symbols: unknown | null;
  target: string;
  timings?: { check_ms: number; codegen_ms: number; emit_ms: number; link_ms?: number };
  /** The relocatable object (ELF for Arm, wasm for wasm32). */
  object: Uint8Array | null;
  /** wasm32 with `link`: the finished module. */
  module: Uint8Array | null;
}

function fromBase64(s: string | undefined): Uint8Array | null {
  if (!s) return null;
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/**
 * Compile with a fresh instance of the compiler module (one build per
 * instance: LLVM keeps state between builds, and a panic aborts).
 */
export async function compile(module: WebAssembly.Module, req: CompileRequest): Promise<CompileResult> {
  const run = await runWasi(module, new TextEncoder().encode(JSON.stringify(req)));
  const text = new TextDecoder().decode(run.stdout);
  let raw: Record<string, unknown>;
  try {
    raw = JSON.parse(text) as Record<string, unknown>;
  } catch {
    throw new Error(`the compiler exited with ${run.exitCode} without a response${run.stderr ? `: ${run.stderr.trim()}` : ""}`);
  }
  return {
    ok: Boolean(raw.ok),
    diagnostics: (raw.diagnostics as Diagnostic[]) ?? [],
    symbols: raw.symbols ?? null,
    target: String(raw.target ?? ""),
    timings: raw.timings as CompileResult["timings"],
    object: fromBase64(raw.object as string | undefined),
    module: fromBase64(raw.module as string | undefined),
  };
}

/** `dist/plcc-compiler.json`, written by build.sh. */
export interface CompilerManifest {
  version: string;
  bytes: number;
  gzip_bytes: number;
  sha256: string;
}

export interface LoadProgress {
  phase: "manifest" | "download" | "cache" | "compile";
  /** Bytes so far / in total (download: compressed bytes). */
  loaded: number;
  total: number;
}

export interface LoadOptions {
  /** Where `plcc-compiler.json` and `plcc-compiler.wasm.gz` are served. */
  baseUrl: string;
  onProgress?: (p: LoadProgress) => void;
  /** Cache Storage name (default "plcc-compiler"); null: no caching. */
  cacheName?: string | null;
  fetch?: typeof fetch;
}

export interface LoadedCompiler {
  module: WebAssembly.Module;
  manifest: CompilerManifest;
  /** Where the bytes came from. */
  source: "network" | "cache";
  ms: number;
}

async function readAll(res: Response, total: number, onChunk: (n: number) => void): Promise<Uint8Array> {
  if (!res.body) return new Uint8Array(await res.arrayBuffer());
  const reader = res.body.getReader();
  const chunks: Uint8Array[] = [];
  let n = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
    n += value.length;
    onChunk(n);
  }
  const out = new Uint8Array(total > 0 && total === n ? total : n);
  let at = 0;
  for (const c of chunks) {
    out.set(c, at);
    at += c.length;
  }
  return out;
}

async function gunzip(bytes: Uint8Array): Promise<Uint8Array> {
  const stream = new Blob([bytes as BlobPart]).stream().pipeThrough(new DecompressionStream("gzip"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

async function sha256(bytes: Uint8Array): Promise<string> {
  const d = await crypto.subtle.digest("SHA-256", bytes as BufferSource);
  return [...new Uint8Array(d)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/**
 * Fetch (or take from Cache Storage) and compile the compiler module. The
 * cache key is the module's SHA-256 from the manifest, so a new build is
 * fetched once and old ones are dropped.
 */
export async function loadCompiler(opts: LoadOptions): Promise<LoadedCompiler> {
  const t0 = performance.now();
  const f = opts.fetch ?? fetch;
  const base = opts.baseUrl.endsWith("/") ? opts.baseUrl : `${opts.baseUrl}/`;
  const progress = opts.onProgress ?? (() => {});
  progress({ phase: "manifest", loaded: 0, total: 0 });
  const mres = await f(`${base}plcc-compiler.json`, { cache: "no-cache" });
  if (!mres.ok) throw new Error(`the compiler is not available here (${mres.status} for plcc-compiler.json)`);
  const manifest = (await mres.json()) as CompilerManifest;
  const key = `${base}plcc-compiler-${manifest.sha256}.wasm`;
  const cacheName = opts.cacheName === undefined ? "plcc-compiler" : opts.cacheName;
  let cache: Cache | null = null;
  try {
    if (cacheName && typeof caches !== "undefined") cache = await caches.open(cacheName);
  } catch {
    cache = null; // private windows, opaque origins
  }
  let wasm: Uint8Array | null = null;
  let source: LoadedCompiler["source"] = "network";
  if (cache) {
    const hit = await cache.match(key);
    if (hit) {
      progress({ phase: "cache", loaded: 0, total: manifest.bytes });
      wasm = new Uint8Array(await hit.arrayBuffer());
      source = "cache";
    }
  }
  if (!wasm) {
    const res = await f(`${base}plcc-compiler.wasm.gz?v=${manifest.sha256.slice(0, 12)}`);
    if (!res.ok) throw new Error(`downloading the compiler failed (${res.status})`);
    const total = manifest.gzip_bytes;
    const gz = await readAll(res, total, (n) => progress({ phase: "download", loaded: n, total }));
    wasm = await gunzip(gz);
    if ((await sha256(wasm)) !== manifest.sha256) throw new Error("the downloaded compiler is corrupt (SHA-256 mismatch)");
    if (cache) {
      try {
        for (const req of await cache.keys()) if (req.url !== key) await cache.delete(req);
        await cache.put(key, new Response(wasm as BodyInit, { headers: { "content-type": "application/wasm" } }));
      } catch {
        // quota: run without caching
      }
    }
  }
  progress({ phase: "compile", loaded: 0, total: manifest.bytes });
  const module = await WebAssembly.compile(wasm as BufferSource);
  return { module, manifest, source, ms: performance.now() - t0 };
}
