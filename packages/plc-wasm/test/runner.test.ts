// SPDX-License-Identifier: MPL-2.0
//
// Against modules produced by `plcc compile --target wasm32-unknown-unknown`
// + wasm-ld (test/fixtures/build.sh).
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { FAULT_DIV_BY_ZERO, FakeClock, PlcFault, PlcModule, Runner, ScanCycle, type SymbolTable, buildImports, libm } from "../src/index";
import { type Event, serveWorker } from "../src/worker";

const here = dirname(fileURLToPath(import.meta.url));
const wasm = (name: string) => readFileSync(join(here, "fixtures", `${name}.wasm`));
const symbols = (name: string) => JSON.parse(readFileSync(join(here, "fixtures", `${name}.symbols.json`), "utf8")) as SymbolTable;

async function load(name: string, clock = new FakeClock(), prints: string[] = []) {
  const plc = await PlcModule.load(wasm(name), { clock, symbols: symbols(name), onPrint: (m) => prints.push(m) });
  return { plc, clock, prints, cycle: new ScanCycle(plc, clock) };
}

describe("imports", () => {
  it("covers exactly what real modules import", () => {
    for (const name of ["seal_in", "ton", "div_zero"]) {
      const module = new WebAssembly.Module(wasm(name));
      const imports = buildImports(module, { nowNs: () => 0n, print: () => {}, cString: () => "" });
      const names = Object.keys(imports.env).sort();
      expect(names).toEqual(WebAssembly.Module.imports(module).map((i) => i.name).sort());
    }
  });

  it("maps libm with f32 rounding and C rounding rules", () => {
    expect(libm("sinf")!(1.5)).toBe(Math.fround(Math.sin(1.5)));
    expect(libm("round")!(-2.5)).toBe(-3);
    expect(libm("fmod")!(-7, 3)).toBe(-1);
    expect(libm("not_a_libm_function")).toBeUndefined();
  });

  it("refuses a module importing something unknown", () => {
    // (module (import "env" "mystery" (func)))
    const bytes = new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0, 1, 4, 1, 96, 0, 0, 2, 15, 1, 3, 101, 110, 118, 7, 109, 121, 115, 116, 101, 114, 121, 0, 0]);
    const module = new WebAssembly.Module(bytes);
    expect(() => buildImports(module, { nowNs: () => 0n, print: () => {}, cString: () => "" })).toThrow(/env\.mystery/);
  });
});

describe("seal-in", () => {
  it("reads the task table from the module", async () => {
    const { plc } = await load("seal_in");
    expect(plc.abiVersion).toBe(1);
    expect(plc.tasks).toEqual([
      { index: 0, name: "MainTask", intervalNs: 20_000_000n, priority: 1, programCount: 1, hasSingle: false, programs: [{ name: "SealIn", programType: "SealIn" }] },
    ]);
  });

  it("latches over scans and releases on stop", async () => {
    const { plc, cycle, clock, prints } = await load("seal_in");
    cycle.start();
    const scan = () => {
      clock.advanceMs(20);
      expect(cycle.step()).toEqual([0]);
    };
    expect(plc.read("SealIn.level")).toBeCloseTo(1.5);
    expect(plc.read("SealIn.name")).toBe("seal");
    scan();
    expect(plc.readBit("Q", 0, 0)).toBe(false);

    plc.writeBit("I", 0, 0, true); // press start
    scan();
    expect(plc.read("SealIn.motor")).toBe(true);
    expect(plc.image("Q")[0] & 1).toBe(1);
    plc.writeBit("I", 0, 0, false); // release start: sealed in
    for (let i = 0; i < 5; i++) scan();
    expect(plc.read("SealIn.motor")).toBe(true);
    expect(plc.read("SealIn.starts")).toBe(1);
    expect(prints).toEqual(["motor on"]);

    plc.write("SealIn.stop", true); // tag write lands in %IX0.1
    expect(plc.image("I")[0]).toBe(0b10);
    scan();
    expect(plc.read("SealIn.motor")).toBe(false);
    plc.write("SealIn.stop", false);
    scan();
    expect(plc.read("SealIn.motor")).toBe(false);
    expect(plc.read("SealIn.runs")).toBe(9);
    expect(plc.read("SealIn.wave")).toBeCloseTo(Math.sin(1.5), 6);
  });

  it("does not run a cyclic task before it is due", async () => {
    const { cycle, clock } = await load("seal_in");
    cycle.start();
    expect(cycle.step()).toEqual([0]);
    clock.advanceMs(19);
    expect(cycle.step()).toEqual([]);
    clock.advanceMs(1);
    expect(cycle.step()).toEqual([0]);
    // Late by 3 periods: runs once, then waits a full period from now.
    clock.advanceMs(70);
    expect(cycle.step()).toEqual([0]);
    clock.advanceMs(10);
    expect(cycle.step()).toEqual([]);
    clock.advanceMs(10);
    expect(cycle.step()).toEqual([0]);
  });
});

describe("TON with a fake clock", () => {
  it("times exactly on the caller's clock", async () => {
    const { plc, cycle, clock } = await load("ton");
    expect(plc.tasks.map((t) => [t.name, t.intervalNs, t.priority])).toEqual([
      ["Fast", 10_000_000n, 1],
      ["Slow", 100_000_000n, 2],
    ]);
    cycle.start();
    plc.writeBit("I", 0, 0, true);
    cycle.simulate(clock, 490, 10); // scans at t = 0, 10, ..., 490 ms
    expect(plc.read("Delay.done")).toBe(false);
    expect(plc.read("Cpu.Main.t.ET")).toBe(490_000_000n);
    expect(plc.read("Delay.et_ms")).toBe(490);
    clock.advanceMs(10);
    cycle.step();
    expect(plc.read("Delay.done")).toBe(true);
    expect(plc.readBit("Q", 0, 0)).toBe(true);
    expect(plc.read("Cpu.Main.t.ET")).toBe(500_000_000n);
    // Both tasks ran at their own rates: Fast 51x, Slow 6x (t = 0, 100, ..., 500).
    expect(cycle.runs).toEqual([51, 6]);
    expect(plc.read("Counter.n")).toBe(6);

    plc.writeBit("I", 0, 0, false);
    clock.advanceMs(10);
    cycle.step();
    expect(plc.read("Delay.done")).toBe(false);
    expect(plc.read("Cpu.Main.t.ET")).toBe(0n);
  });

  it("runs tasks highest priority first when both are due", async () => {
    const { cycle } = await load("ton");
    cycle.start();
    expect(cycle.step()).toEqual([0, 1]);
  });
});

describe("faults", () => {
  it("a division by zero stops the PLC with its code and location", async () => {
    const { plc, cycle, clock } = await load("div_zero");
    const faults: [number, string, number][] = [];
    const flushed: number[] = [];
    const c = new ScanCycle(plc, clock, {
      onFault: (f, task) => faults.push([f.code, f.where, task]),
      flushOutputs: (p) => flushed.push(p.image("Q")[0]),
    });
    plc.write("Ratio.divisor", 4);
    c.start();
    expect(c.step()).toEqual([0]);
    expect(plc.read("Ratio.q")).toBe(25);
    expect(plc.read("Ratio.lamp")).toBe(true);
    expect(plc.read("Ratio.after")).toBe(1);

    plc.write("Ratio.divisor", 0);
    clock.advanceMs(20);
    c.step();
    expect(c.state).toBe("faulted");
    expect(plc.fault).toBeInstanceOf(PlcFault);
    expect(plc.fault!.code).toBe(FAULT_DIV_BY_ZERO);
    // file:line:col: POU — the division is on line 11, column 6 of the source.
    expect(plc.fault!.where).toBe("div_zero.st:11:6: Ratio");
    expect(faults).toEqual([[1, plc.fault!.where, 0]]);
    // Statements after the division did not run; outputs are off.
    expect(plc.read("Ratio.after")).toBe(1);
    expect(plc.image("Q")[0]).toBe(0);
    expect(flushed.at(-1)).toBe(0);
    // Stopped until restarted.
    clock.advanceMs(20);
    expect(c.step()).toEqual([]);
    expect(() => plc.runTask(0)).toThrow(/restart/);

    c.restart();
    expect(plc.fault).toBeNull();
    expect(plc.read("Ratio.after")).toBe(0); // cold start
    plc.write("Ratio.divisor", 5);
    expect(c.step()).toEqual([0]);
    expect(plc.read("Ratio.q")).toBe(20);
    void cycle;
  });
});

describe("Runner (real timers)", () => {
  it("drives the cycle with setTimeout and stops", async () => {
    const { plc } = await load("seal_in");
    const clock = { nowNs: () => BigInt(Math.round(performance.now() * 1e6)) };
    const cycle = new ScanCycle(plc, clock);
    const runner = new Runner(cycle, { pollMs: 5 });
    runner.start();
    await new Promise((r) => setTimeout(r, 120));
    runner.stop();
    const runs = cycle.runs[0];
    expect(runs).toBeGreaterThanOrEqual(3); // 20 ms task over ~120 ms
    expect(runs).toBeLessThanOrEqual(8);
    expect(runner.running).toBe(false);
    expect(cycle.state).toBe("stopped");
  });
});

describe("worker protocol", () => {
  it("loads, steps a fake clock, writes and reads tags, reports a fault", async () => {
    const events: Event[] = [];
    let listener: ((e: { data: never }) => void) | null = null;
    const handle = serveWorker(
      { postMessage: (m) => events.push(m), addEventListener: (_t, l) => (listener = l as never) },
      { snapshotMs: 0 },
    );
    expect(listener).not.toBeNull();
    const reply = async (req: Parameters<typeof handle>[0]) => {
      events.length = 0;
      await handle(req);
      const r = events.find((e) => e.type === "reply" || e.type === "error")!;
      if (r.type === "error") throw new Error(r.message);
      return (r as { result?: unknown }).result as never;
    };
    const loaded = await reply({ type: "load", wasm: wasm("div_zero"), symbols: symbols("div_zero"), clock: "fake" });
    expect(loaded).toMatchObject({ tasks: [{ name: "MainTask", intervalNs: "20000000" }], fault: null });
    await reply({ type: "writeTag", path: "Ratio.divisor", value: 2 });
    const s1: { ran: number[]; snapshot: { Q: Uint8Array } } = await reply({ type: "step" });
    expect(s1.ran).toEqual([0]);
    expect(s1.snapshot.Q[0]).toBe(1);
    expect(await reply({ type: "readTags", paths: ["Ratio.q"] })).toEqual({ "Ratio.q": 50 });
    await reply({ type: "writeImage", area: "M", offset: 0, bytes: new Uint8Array([0, 0]) });
    const s2: { snapshot: { state: string; fault: { code: number } } } = await reply({ type: "step", advanceMs: 20 });
    expect(s2.snapshot.state).toBe("faulted");
    expect(s2.snapshot.fault.code).toBe(1);
    expect(events.some((e) => e.type === "fault" && e.code === 1)).toBe(true);
    const r: { state: string; fault: null } = await reply({ type: "restart" });
    expect(r.fault).toBeNull();
    await expect(reply({ type: "readTags", paths: ["nope"] })).rejects.toThrow(/no variable/);
  });
});
