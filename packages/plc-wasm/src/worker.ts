// SPDX-License-Identifier: MPL-2.0
//
// A message protocol for running the PLC in a Web Worker, so the scan loop
// never competes with the editor's UI thread. In the worker:
//
//   import { serveWorker } from "@plcc/plc-wasm/worker";
//   serveWorker(self);
//
// From the page, post `Request`s and listen for `Event`s. Every request may
// carry an `id`, echoed on its `reply` (or `error`) event.

import { type Clock, FakeClock, RealClock } from "./clock";
import type { PlcFault } from "./imports";
import { type Area, PlcModule } from "./plc";
import { Runner, ScanCycle } from "./scan";
import type { SymbolTable, TagValue } from "./tags";

export type Request = { id?: number } & (
  | { type: "load"; wasm: ArrayBuffer | Uint8Array; symbols?: SymbolTable; clock?: "real" | "fake" }
  | { type: "start" }
  | { type: "stop" }
  | { type: "restart" }
  /** Single-step: with a fake clock, advance by `advanceMs` first. */
  | { type: "step"; advanceMs?: number }
  | { type: "writeImage"; area: Area; offset: number; bytes: Uint8Array }
  | { type: "writeBit"; area: Area; byte: number; bit: number; value: boolean }
  | { type: "writeTag"; path: string; value: TagValue }
  | { type: "readTags"; paths: string[] }
  | { type: "snapshot" }
);

export interface Snapshot {
  state: "stopped" | "running" | "faulted";
  I: Uint8Array;
  Q: Uint8Array;
  M: Uint8Array;
  runs: number[];
  fault: { code: number; where: string; message: string } | null;
}

export type Event =
  | { type: "reply"; id?: number; result?: unknown }
  | { type: "error"; id?: number; message: string }
  /** After every pass that ran a task (throttled to `snapshotMs`), and after a fault. */
  | { type: "scan"; snapshot: Snapshot }
  | { type: "print"; message: string }
  | { type: "fault"; code: number; where: string; task: number };

/** The subset of a worker's global scope the protocol uses. */
export interface Port {
  postMessage(message: Event): void;
  addEventListener(type: "message", listener: (e: { data: Request }) => void): void;
}

export interface ServeOptions {
  /** Minimum ms between `scan` events (default 50). */
  snapshotMs?: number;
}

function faultInfo(f: PlcFault | null) {
  return f ? { code: f.code, where: f.where, message: f.message } : null;
}

/** Handle PLC requests on `port` (a worker's `self`). Returns the handler, for tests. */
export function serveWorker(port: Port, options: ServeOptions = {}): (req: Request) => Promise<void> {
  let plc: PlcModule | null = null;
  let cycle: ScanCycle | null = null;
  let runner: Runner | null = null;
  let clock: Clock = new RealClock();
  let lastSnapshot = -Infinity;
  const snapshotMs = options.snapshotMs ?? 50;

  const snapshot = (): Snapshot => {
    if (!plc || !cycle) throw new Error("no program loaded");
    return {
      state: cycle.state,
      I: plc.image("I").slice(),
      Q: plc.image("Q").slice(),
      M: plc.image("M").slice(),
      runs: [...cycle.runs],
      fault: faultInfo(plc.fault),
    };
  };
  const need = () => {
    if (!plc || !cycle || !runner) throw new Error("no program loaded");
    return { plc, cycle, runner };
  };

  const handle = async (req: Request): Promise<void> => {
    try {
      let result: unknown;
      switch (req.type) {
        case "load": {
          runner?.stop();
          clock = req.clock === "fake" ? new FakeClock() : new RealClock();
          plc = await PlcModule.load(req.wasm as BufferSource, {
            clock,
            symbols: req.symbols,
            onPrint: (message) => port.postMessage({ type: "print", message }),
          });
          cycle = new ScanCycle(plc, clock, {
            onScan: () => {
              const now = performance.now();
              if (now - lastSnapshot >= snapshotMs) {
                lastSnapshot = now;
                port.postMessage({ type: "scan", snapshot: snapshot() });
              }
            },
            onFault: (f, task) => {
              port.postMessage({ type: "fault", code: f.code, where: f.where, task });
              port.postMessage({ type: "scan", snapshot: snapshot() });
            },
          });
          runner = new Runner(cycle);
          result = { tasks: plc.tasks.map((t) => ({ ...t, intervalNs: t.intervalNs.toString() })), fault: faultInfo(plc.fault) };
          break;
        }
        case "start":
          need().runner.start();
          break;
        case "stop":
          need().runner.stop();
          break;
        case "restart": {
          const { runner: r, cycle: c } = need();
          r.stop();
          c.restart();
          c.stop();
          result = snapshot();
          break;
        }
        case "step": {
          const { cycle: c } = need();
          if (req.advanceMs !== undefined) {
            if (!(clock instanceof FakeClock)) throw new Error("advanceMs needs the fake clock (load with clock: \"fake\")");
            clock.advanceMs(req.advanceMs);
          }
          if (c.state === "stopped") c.start();
          const ran = c.step();
          c.stop();
          result = { ran, snapshot: snapshot() };
          break;
        }
        case "writeImage":
          need().plc.image(req.area).set(req.bytes, req.offset);
          break;
        case "writeBit":
          need().plc.writeBit(req.area, req.byte, req.bit, req.value);
          break;
        case "writeTag":
          need().plc.write(req.path, req.value);
          break;
        case "readTags": {
          const p = need().plc;
          result = Object.fromEntries(req.paths.map((path) => [path, p.read(path)]));
          break;
        }
        case "snapshot":
          result = snapshot();
          break;
      }
      port.postMessage({ type: "reply", id: req.id, result });
    } catch (e) {
      port.postMessage({ type: "error", id: req.id, message: e instanceof Error ? e.message : String(e) });
    }
  };
  port.addEventListener("message", (e) => void handle(e.data));
  return handle;
}
