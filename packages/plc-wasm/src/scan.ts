// SPDX-License-Identifier: MPL-2.0
//
// The scan scheduler: the same loop as plcc_hal::scan::ScanCycle and the
// Arduino runtime (docs/process-image.md, "Scheduling"):
//   - interval > 0: due every interval while SINGLE (if any) is FALSE; a late
//     task runs once and skips missed activations;
//   - SINGLE: due once on each FALSE → TRUE edge;
//   - neither: free-running, due every pass;
//   - due tasks run highest priority (lowest number) first, ties in table order;
//   - inputs are latched before readiness is evaluated, outputs flushed after
//     the last task.
// No DOM APIs: it runs in a window or a Web Worker.

import type { Clock } from "./clock";
import type { PlcFault } from "./imports";
import type { PlcModule } from "./plc";

export type PlcState = "stopped" | "running" | "faulted";

export interface ScanHooks {
  /** Before readiness is evaluated: copy field inputs into %I. */
  latchInputs?(plc: PlcModule): void;
  /** After the last task of a pass: copy %Q to the field (also after a fault, with %Q cleared). */
  flushOutputs?(plc: PlcModule): void;
  /** A task faulted; the PLC is now stopped until restart(). */
  onFault?(fault: PlcFault, task: number): void;
  /** After every pass that ran at least one task. */
  onScan?(plc: PlcModule, ranTasks: number[]): void;
}

export class ScanCycle {
  state: PlcState = "stopped";
  private nextDue: bigint[] = [];
  private prevSingle: boolean[] = [];
  /** Passes that ran at least one task, per task. */
  readonly runs: number[] = [];

  readonly plc: PlcModule;
  readonly clock: Clock;
  private readonly hooks: ScanHooks;

  constructor(plc: PlcModule, clock: Clock, hooks: ScanHooks = {}) {
    this.plc = plc;
    this.clock = clock;
    this.hooks = hooks;
    this.reset();
  }

  private reset(): void {
    const n = this.plc.tasks.length;
    this.nextDue = new Array(n).fill(0n);
    this.prevSingle = new Array(n).fill(false);
    this.runs.length = 0;
    for (let i = 0; i < n; i++) this.runs.push(0);
  }

  start(): void {
    if (this.plc.fault) throw new Error("the PLC is faulted: restart() it");
    this.state = "running";
  }

  stop(): void {
    if (this.state === "running") this.state = "stopped";
  }

  /** Cold restart: new instance, `plcc_init`, schedule reset; then running. */
  restart(): void {
    this.plc.restart();
    this.reset();
    this.state = "running";
  }

  /**
   * One pass of the loop at the clock's current time. Returns the tasks that
   * ran, in the order they ran.
   */
  step(): number[] {
    if (this.state !== "running") return [];
    const plc = this.plc;
    this.hooks.latchInputs?.(plc);
    const now = this.clock.nowNs();
    const due: boolean[] = [];
    for (let t = 0; t < plc.tasks.length; t++) {
      const task = plc.tasks[t];
      let d = false;
      const s = task.hasSingle ? plc.single(t) : false;
      if (plc.fault) {
        // A SINGLE expression can fault too (a division in it).
        this.faulted(t);
        return [];
      }
      if (task.hasSingle && s && !this.prevSingle[t]) d = true;
      this.prevSingle[t] = s;
      if (task.intervalNs > 0n && !s && now >= this.nextDue[t]) {
        d = true;
        const next = this.nextDue[t] + task.intervalNs;
        this.nextDue[t] = next > now ? next : now + task.intervalNs;
      }
      if (task.intervalNs === 0n && !task.hasSingle) d = true;
      due[t] = d;
    }
    const ran: number[] = [];
    for (;;) {
      let best = -1;
      for (let t = 0; t < due.length; t++) {
        if (due[t] && (best < 0 || plc.tasks[t].priority < plc.tasks[best].priority)) best = t;
      }
      if (best < 0) break;
      due[best] = false;
      ran.push(best);
      if (!plc.runTask(best)) {
        this.faulted(best);
        return ran;
      }
      this.runs[best]++;
    }
    this.hooks.flushOutputs?.(plc);
    if (ran.length) this.hooks.onScan?.(plc, ran);
    return ran;
  }

  private faulted(task: number): void {
    this.state = "faulted";
    this.hooks.flushOutputs?.(this.plc); // %Q was cleared by the module
    if (this.plc.fault) this.hooks.onFault?.(this.plc.fault, task);
  }

  /**
   * Time until the next cyclic task is due (ms, ≥ 0), or 0 if a free-running
   * task exists; `Infinity` if nothing is cyclic (only SINGLE tasks: poll).
   */
  msUntilDue(): number {
    const now = this.clock.nowNs();
    let best = Infinity;
    this.plc.tasks.forEach((task, t) => {
      if (task.intervalNs === 0n && !task.hasSingle) best = 0;
      else if (task.intervalNs > 0n) best = Math.min(best, Math.max(0, Number(this.nextDue[t] - now) / 1e6));
    });
    return best;
  }

  /**
   * Advance a FakeClock-like clock in steps of `tickMs` for `durationMs`,
   * stepping after each advance (and once before the first). For tests and
   * "run N ms" buttons.
   */
  simulate(clock: { advanceMs(ms: number): void }, durationMs: number, tickMs = 1): void {
    this.step();
    for (let t = 0; t < durationMs && this.state === "running"; t += tickMs) {
      clock.advanceMs(tickMs);
      this.step();
    }
  }
}

/**
 * Drives a ScanCycle in real time with setTimeout (works in a Web Worker):
 * sleeps until the next task is due, at least `minSleepMs`, at most `pollMs`
 * (the poll bounds how late a SINGLE edge is seen).
 */
export class Runner {
  private timer: ReturnType<typeof setTimeout> | null = null;

  readonly cycle: ScanCycle;
  private readonly opts: { minSleepMs?: number; pollMs?: number };

  constructor(cycle: ScanCycle, opts: { minSleepMs?: number; pollMs?: number } = {}) {
    this.cycle = cycle;
    this.opts = opts;
  }

  get running(): boolean {
    return this.timer !== null;
  }

  start(): void {
    if (this.timer !== null) return;
    if (this.cycle.state !== "running") this.cycle.start();
    const tick = () => {
      this.cycle.step();
      if (this.cycle.state !== "running") {
        this.timer = null;
        return;
      }
      const min = this.opts.minSleepMs ?? 1;
      const max = this.opts.pollMs ?? 10;
      const wait = Math.min(max, Math.max(min, this.cycle.msUntilDue()));
      this.timer = setTimeout(tick, wait);
    };
    this.timer = setTimeout(tick, 0);
  }

  stop(): void {
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
    this.cycle.stop();
  }
}
