// SPDX-License-Identifier: MPL-2.0
//
// Fixed-period scan loop with drift correction. Scans are due at
// start + k * period; each wake-up runs every scan that is due (so timer
// fires late or clamped to 4 ms still produce the right number of scans and
// the right simulated time), bounded per wake-up so the loop always yields.
// Runs inside the simulator worker; the clock is injectable for tests.

export interface LoopClock {
  now(): number
  setTimeout(fn: () => void, ms: number): unknown
  clearTimeout(handle: unknown): void
}

export const realClock: LoopClock = {
  now: () => performance.now(),
  setTimeout: (fn, ms) => setTimeout(fn, ms),
  clearTimeout: (h) => clearTimeout(h as ReturnType<typeof setTimeout>),
}

export interface LoopStats {
  scans: number
  /** Mean |start - due| over recent scans, ms. */
  jitterMs: number
  maxJitterMs: number
  /** Scans dropped because the loop fell too far behind. */
  skipped: number
}

export interface LoopOptions {
  /** Most scans run in one wake-up before yielding. */
  maxCatchUp?: number
  /** Most time spent in one wake-up before yielding, ms. */
  sliceMs?: number
}

export class FixedPeriodLoop {
  private periodMs: number
  private readonly tick: (dtMs: number) => void
  private readonly clock: LoopClock
  private readonly maxCatchUp: number
  private readonly sliceMs: number
  private handle: unknown = null
  private due = 0
  running = false
  stats: LoopStats = { scans: 0, jitterMs: 0, maxJitterMs: 0, skipped: 0 }

  constructor(periodMs: number, tick: (dtMs: number) => void, clock: LoopClock = realClock, opts: LoopOptions = {}) {
    this.periodMs = periodMs
    this.tick = tick
    this.clock = clock
    this.maxCatchUp = opts.maxCatchUp ?? 64
    this.sliceMs = opts.sliceMs ?? 12
  }

  get period(): number {
    return this.periodMs
  }

  setPeriod(ms: number) {
    this.periodMs = Math.max(0.1, ms)
    this.due = this.clock.now()
  }

  start() {
    if (this.running) return
    this.running = true
    this.due = this.clock.now()
    this.stats = { scans: 0, jitterMs: 0, maxJitterMs: 0, skipped: 0 }
    this.schedule(0)
  }

  stop() {
    this.running = false
    if (this.handle !== null) this.clock.clearTimeout(this.handle)
    this.handle = null
  }

  private schedule(delay: number) {
    this.handle = this.clock.setTimeout(() => this.wake(), Math.max(0, delay))
  }

  private wake() {
    this.handle = null
    if (!this.running) return
    const start = this.clock.now()
    let n = 0
    while (this.running && this.clock.now() >= this.due && n < this.maxCatchUp) {
      const late = this.clock.now() - this.due
      const s = this.stats
      s.jitterMs = s.jitterMs + (Math.abs(late) - s.jitterMs) / Math.min(s.scans + 1, 100)
      s.maxJitterMs = Math.max(s.maxJitterMs, late)
      this.tick(this.periodMs)
      s.scans++
      this.due += this.periodMs
      n++
      if (this.clock.now() - start > this.sliceMs) break
    }
    // Hopelessly behind (tab suspended, huge program): drop the backlog rather than burst.
    const behind = this.clock.now() - this.due
    if (behind > this.periodMs * this.maxCatchUp) {
      this.stats.skipped += Math.floor(behind / this.periodMs)
      this.due = this.clock.now() + this.periodMs
    }
    if (this.running) this.schedule(this.due - this.clock.now())
  }
}
