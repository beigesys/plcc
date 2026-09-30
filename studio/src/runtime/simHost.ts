// SPDX-License-Identifier: MPL-2.0
//
// Worker-side simulator host: owns the Simulator and the scan loop, applies
// commands from the main thread and posts throttled snapshots (changed values
// only) back. No DOM or Worker API here, so it runs under vitest too.

import { Simulator } from '@/engine'
import { parseAddress } from '@/model'
import { FixedPeriodLoop, realClock, type LoopClock } from './loop'
import type { FromWorker, Scalar, SimStats, TagValue, ToWorker } from './messages'

export interface SimHostOptions {
  clock?: LoopClock
  /** Minimum ms between snapshots (~30 Hz). */
  postEveryMs?: number
  /** Per-scan time budget; a scan that exceeds it is cut short and reported. */
  scanBudgetMs?: number
}

export class SimHost {
  private sim: Simulator | null = null
  private loop: FixedPeriodLoop | null = null
  private readonly post: (m: FromWorker) => void
  private readonly clock: LoopClock
  private readonly postEveryMs: number
  private readonly budgetMs: number
  private lastPost = -Infinity
  private sent = new Map<string, string>()
  private maxScanMs = 0
  private periodMs = 10
  private overrun = false

  constructor(post: (m: FromWorker) => void, opts: SimHostOptions = {}) {
    this.post = post
    this.clock = opts.clock ?? realClock
    this.postEveryMs = opts.postEveryMs ?? 33
    this.budgetMs = opts.scanBudgetMs ?? 50
  }

  handle(msg: ToWorker) {
    try {
      this.apply(msg)
    } catch (e) {
      this.post({ type: 'error', message: e instanceof Error ? e.message : String(e) })
    }
  }

  private apply(msg: ToWorker) {
    switch (msg.type) {
      case 'init':
        this.loop?.stop()
        this.periodMs = msg.periodMs
        this.sim = new Simulator(msg.project, msg.device)
        this.loop = new FixedPeriodLoop(msg.periodMs, (dt) => this.scan(dt), this.clock)
        this.sent.clear()
        this.maxScanMs = 0
        this.flush()
        return
      case 'project':
        this.sim?.setProject(msg.project)
        this.flush()
        return
      case 'start':
        this.loop?.start()
        this.flush()
        return
      case 'stop':
        this.loop?.stop()
        this.flush()
        return
      case 'period':
        this.periodMs = msg.periodMs
        this.loop?.setPeriod(msg.periodMs)
        return
      case 'reset':
        this.sim?.reset()
        this.maxScanMs = 0
        this.flush()
        return
      case 'write':
        this.sim?.tags.write(msg.ref, msg.value)
        this.flush()
        return
      case 'force':
        if (msg.value === null) this.sim?.tags.unforce(msg.tag)
        else this.sim?.tags.force(msg.tag, msg.value)
        this.flush()
        return
      case 'image': {
        const a = parseAddress(msg.address)
        if (a && this.sim) this.sim.image.write(a, msg.value)
        this.flush()
        return
      }
    }
  }

  private scan(dt: number) {
    const sim = this.sim
    if (!sim) return
    sim.scan(dt, { budgetMs: this.budgetMs, now: () => this.clock.now() })
    this.overrun = sim.overrun
    this.maxScanMs = Math.max(this.maxScanMs, sim.lastScanMs)
    if (this.clock.now() - this.lastPost >= this.postEveryMs) this.flush()
  }

  stats(): SimStats {
    const sim = this.sim
    const ls = this.loop?.stats
    return {
      running: this.loop?.running ?? false,
      periodMs: this.periodMs,
      scans: ls?.scans ?? 0,
      lastScanMs: sim?.lastScanMs ?? 0,
      maxScanMs: this.maxScanMs,
      jitterMs: ls?.jitterMs ?? 0,
      overruns: sim?.overrunCount ?? 0,
      skipped: ls?.skipped ?? 0,
    }
  }

  /** Posts a snapshot now. */
  flush() {
    const sim = this.sim
    if (!sim) return
    this.lastPost = this.clock.now()
    const snap = sim.tags.snapshot() as Record<string, TagValue>
    const values: Record<string, TagValue> = {}
    for (const [name, v] of Object.entries(snap)) {
      const k = name.toLowerCase()
      const j = JSON.stringify(v)
      if (this.sent.get(k) !== j) {
        values[k] = v
        this.sent.set(k, j)
      }
    }
    const lower = new Set(Object.keys(snap).map((n) => n.toLowerCase()))
    const removed: string[] = []
    for (const k of this.sent.keys()) {
      if (!lower.has(k)) {
        removed.push(k)
        this.sent.delete(k)
      }
    }
    const forces: Record<string, Scalar> = {}
    for (const [k, v] of sim.tags.forces) forces[k] = v
    this.post({
      type: 'snapshot',
      trace: sim.snapshotTrace(),
      values,
      removed,
      forces,
      errors: Object.fromEntries(sim.errors),
      image: { I: sim.image.I.slice(), Q: sim.image.Q.slice(), M: sim.image.M.slice() },
      stats: this.stats(),
      overrun: this.overrun,
    })
  }
}
