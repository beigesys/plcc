// SPDX-License-Identifier: MPL-2.0
// Run a WASI program under Node: node wasi-run.mjs <prog.wasm> <preopen dir> args...
import { readFileSync } from "node:fs";
import { WASI } from "node:wasi";
const [prog, dir, ...args] = process.argv.slice(2);
const wasi = new WASI({ version: "preview1", args: [prog, ...args], preopens: { [dir]: dir }, returnOnExit: true });
const t0 = performance.now();
const module = await WebAssembly.compile(readFileSync(prog));
const t1 = performance.now();
const instance = await WebAssembly.instantiate(module, wasi.getImportObject());
const code = wasi.start(instance);
const t2 = performance.now();
console.error(`compile ${(t1 - t0).toFixed(0)} ms, run ${(t2 - t1).toFixed(0)} ms, exit ${code}`);
process.exitCode = code;
