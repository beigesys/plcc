// SPDX-License-Identifier: MPL-2.0
//
// The real compiler module (dist/, built by ../build.sh; skipped when it has
// not been built) compiling ST and a ladder model to wasm32 and Arm, and the
// linked module running in @plcc/plc-wasm.
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { gunzipSync } from "node:zlib";
import { beforeAll, describe, expect, it } from "vitest";
import { FakeClock, PlcModule, ScanCycle, type SymbolTable } from "../../plc-wasm/src/index";
import { compile, loadCompiler } from "../src/index";
import { type CompilerEvent, serveCompiler } from "../src/worker";

const here = dirname(fileURLToPath(import.meta.url));
const dist = join(here, "..", "dist");
const built = existsSync(join(dist, "plcc-compiler.wasm.gz"));
const root = join(here, "..", "..", "..");

let module: WebAssembly.Module;

const SEAL_IN = readFileSync(join(root, "packages/plc-wasm/test/fixtures/seal_in.st"), "utf8");

const MODEL = JSON.stringify({
  dialect: "logix",
  name: "Demo",
  globals: [
    { name: "StartPB", data_type: "BOOL", section: "global", address: "%MX0.0" },
    { name: "StopPB", data_type: "BOOL", section: "global", address: "%MX2.0" },
    { name: "Motor", data_type: "BOOL", section: "global", address: "%QX0.0" },
    { name: "RunTimer", data_type: "TIMER", section: "global" },
  ],
  pous: [
    {
      id: 1, name: "MainProgram", kind: "program",
      routines: [{
        id: 2, name: "MainRoutine",
        rungs: [
          { id: 3, elements: [
            { type: "branch", id: 4, legs: [[{ type: "contact", id: 5, operand: "StartPB", kind: "no" }], [{ type: "contact", id: 6, operand: "Motor", kind: "no" }]] },
            { type: "contact", id: 7, operand: "StopPB", kind: "nc" },
            { type: "coil", id: 8, operand: "Motor", kind: "normal" },
          ] },
          { id: 9, elements: [
            { type: "contact", id: 10, operand: "Motor", kind: "no" },
            { type: "block", id: 11, name: "TON", pins: [{ name: "Timer", value: "RunTimer" }, { name: "Preset", value: "5000" }, { name: "Accum", value: "0" }] },
          ] },
        ],
      }],
    },
  ],
  tasks: [{ name: "MainTask", interval_ms: 10, programs: ["MainProgram"] }],
});

describe.skipIf(!built)("plcc compiler (wasm)", () => {
  beforeAll(async () => {
    module = await WebAssembly.compile(gunzipSync(readFileSync(join(dist, "plcc-compiler.wasm.gz"))));
  });

  it("compiles and links ST that plc-wasm runs", async () => {
    const r = await compile(module, { files: { "seal_in.st": SEAL_IN }, target: "wasm32-unknown-unknown", opt_level: 2, link: true });
    expect(r.ok, JSON.stringify(r.diagnostics)).toBe(true);
    expect(r.module).not.toBeNull();
    const clock = new FakeClock();
    const prints: string[] = [];
    const plc = await PlcModule.load(r.module!, { clock, symbols: r.symbols as SymbolTable, onPrint: (m) => prints.push(m) });
    const cycle = new ScanCycle(plc, clock);
    cycle.start();
    plc.writeBit("I", 0, 0, true);
    cycle.simulate(clock, 50, 10);
    expect(plc.readBit("Q", 0, 0)).toBe(true);
    expect(prints).toContain("motor on");
  });

  it("reports type errors as diagnostics", async () => {
    const r = await compile(module, { files: { "a.st": "PROGRAM P VAR x : INT; END_VAR x := TRUE + Nope; END_PROGRAM" } });
    expect(r.ok).toBe(false);
    expect(r.object).toBeNull();
    expect(r.diagnostics.some((d) => d.message.includes("Nope"))).toBe(true);
  });

  it("compiles a studio ladder model: seal-in and a timer, in its periodic task", async () => {
    const r = await compile(module, {
      files: { "project.json": MODEL },
      entry: ["project.json"],
      target: "wasm32-unknown-unknown",
      image: { I: 18, Q: 1, M: 64 },
      opt_level: 2,
      link: true,
    });
    expect(r.ok, JSON.stringify(r.diagnostics)).toBe(true);
    const clock = new FakeClock();
    const plc = await PlcModule.load(r.module!, { clock, symbols: r.symbols as SymbolTable });
    expect(plc.tasks.map((t) => [t.name.toUpperCase(), t.intervalNs])).toContainEqual(["MAINTASK", 10_000_000n]);
    const cycle = new ScanCycle(plc, clock);
    cycle.start();
    plc.writeBit("M", 0, 0, true); // StartPB
    cycle.simulate(clock, 30, 10);
    plc.writeBit("M", 0, 0, false);
    cycle.simulate(clock, 30, 10);
    expect(plc.readBit("Q", 0, 0)).toBe(true); // sealed in
    cycle.simulate(clock, 1000, 10);
    const acc = Number(plc.read("GLOBAL.RunTimer.ACC"));
    expect(acc).toBeGreaterThan(900);
    expect(acc).toBeLessThanOrEqual(5000);
    plc.writeBit("M", 2, 0, true); // StopPB
    cycle.simulate(clock, 30, 10);
    expect(plc.readBit("Q", 0, 0)).toBe(false);
  });

  it("builds for the Opta from its device manifest", async () => {
    const device = readFileSync(join(root, "crates/plcc-device/builtin/arduino-opta.toml"), "utf8");
    const r = await compile(module, { files: { "project.json": MODEL }, entry: ["project.json"], device, opt_level: 2 });
    expect(r.ok, JSON.stringify(r.diagnostics)).toBe(true);
    expect(r.target).toMatch(/^thumbv7em/);
    // An ELF object.
    expect([...r.object!.subarray(0, 4)]).toEqual([0x7f, 0x45, 0x4c, 0x46]);
  });

  it("serves compiles from a worker with progress, through Cache Storage-less loading", async () => {
    const events: CompilerEvent[] = [];
    const gz = readFileSync(join(dist, "plcc-compiler.wasm.gz"));
    const manifest = readFileSync(join(dist, "plcc-compiler.json"), "utf8");
    const fakeFetch = (async (url: string) =>
      url.includes(".json") ? new Response(manifest) : new Response(gz)) as unknown as typeof fetch;
    const loaded = await loadCompiler({ baseUrl: "http://x/", fetch: fakeFetch, cacheName: null, onProgress: (p) => events.push({ type: "progress", progress: p }) });
    expect(loaded.source).toBe("network");
    expect(events.some((e) => e.type === "progress" && e.progress.phase === "download")).toBe(true);
    // The worker protocol (load is shared between requests).
    const posted: CompilerEvent[] = [];
    const g = globalThis as { fetch: typeof fetch };
    const realFetch = g.fetch;
    g.fetch = fakeFetch;
    try {
      const handle = serveCompiler({ postMessage: (m) => posted.push(m), addEventListener: () => {} });
      await handle({ type: "compile", id: 7, baseUrl: "http://x/", request: { files: { "seal_in.st": SEAL_IN } } });
    } finally {
      g.fetch = realFetch;
    }
    const result = posted.find((e) => e.type === "result");
    expect(result && result.type === "result" && result.id === 7 && result.result.ok).toBe(true);
  });
});
