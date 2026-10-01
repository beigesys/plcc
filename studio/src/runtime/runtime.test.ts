// SPDX-License-Identifier: MPL-2.0
import { describe, expect, it } from 'vitest'
import { demoProject, parseRung, type Project } from '@/model'
import { FixedPeriodLoop, type LoopClock } from './loop'
import type { FromWorker, Snapshot } from './messages'
import { SimHost } from './simHost'
import { catalogEntry } from '@/devices/catalog'

const OPTA = catalogEntry('arduino-opta')!.device
const SIM = catalogEntry('simulator')!.device

/** A clock whose timers fire when `advance` passes them. setTimeout clamps to `minDelay` like a browser. */
function fakeClock(minDelay = 0) {
  let t = 0
  let timers: { at: number; fn: () => void; id: number }[] = []
  let nextId = 1
  const clock: LoopClock = {
    now: () => t,
    setTimeout: (fn, ms) => {
      const id = nextId++
      timers.push({ at: t + Math.max(ms, minDelay), fn, id })
      return id
    },
    clearTimeout: (h) => {
      timers = timers.filter((x) => x.id !== h)
    },
  }
  const advance = (ms: number) => {
    const end = t + ms
    for (;;) {
      timers.sort((a, b) => a.at - b.at)
      const next = timers[0]
      if (!next || next.at > end) break
      timers.shift()
      t = Math.max(t, next.at)
      next.fn()
    }
    t = end
  }
  return { clock, advance, tick: (ms: number) => (t += ms) }
}

describe('fixed-period loop', () => {
  it('runs the right number of scans even when timers are clamped to 4 ms', () => {
    const { clock, advance } = fakeClock(4)
    let scans = 0
    let simulated = 0
    const loop = new FixedPeriodLoop(1, (dt) => {
      scans++
      simulated += dt
    }, clock)
    loop.start()
    advance(1000)
    loop.stop()
    expect(scans).toBeGreaterThanOrEqual(997)
    expect(scans).toBeLessThanOrEqual(1001)
    expect(simulated).toBe(scans)
  })

  it('does not drift at 10 ms', () => {
    const { clock, advance } = fakeClock(0)
    let scans = 0
    const loop = new FixedPeriodLoop(10, () => scans++, clock)
    loop.start()
    advance(60_000)
    expect(scans).toBe(6001)
    expect(loop.stats.jitterMs).toBe(0)
  })

  it('drops the backlog instead of bursting after a long stall', () => {
    const f = fakeClock(0)
    let scans = 0
    const loop = new FixedPeriodLoop(1, () => scans++, f.clock, { maxCatchUp: 16 })
    loop.start()
    f.advance(10)
    const before = scans
    f.tick(5000) // the thread was suspended
    f.advance(1)
    expect(scans - before).toBeLessThanOrEqual(17)
    expect(loop.stats.skipped).toBeGreaterThan(4000)
  })

  it('a 1 ms loop leaves the event loop free for UI work, even on the same thread', async () => {
    // In the app the loop runs in a worker; this checks it also yields when it does not.
    const project = demoProject()
    const host = new SimHost(() => {})
    host.handle({ type: 'init', project, device: OPTA, periodMs: 1 })
    host.handle({ type: 'start' })
    const frames: number[] = []
    const started = performance.now()
    await new Promise<void>((resolve) => {
      const iv = setInterval(() => {
        frames.push(performance.now())
        if (performance.now() - started > 400) {
          clearInterval(iv)
          resolve()
        }
      }, 16)
    })
    host.handle({ type: 'stop' })
    const gaps = frames.slice(1).map((f, i) => f - frames[i])
    const worst = Math.max(...gaps)
    expect(frames.length).toBeGreaterThanOrEqual(15)
    expect(worst).toBeLessThan(60)
    expect(host.stats().scans).toBeGreaterThan(100)
  })
})

describe('simulator host', () => {
  function run(project: Project, ms: number, opts: { post?: number } = {}) {
    const f = fakeClock(0)
    const posts: Snapshot[] = []
    const host = new SimHost((m: FromWorker) => m.type === 'snapshot' && posts.push(m), {
      clock: f.clock,
      postEveryMs: opts.post ?? 33,
    })
    host.handle({ type: 'init', project, device: OPTA, periodMs: 10 })
    host.handle({ type: 'start' })
    f.advance(ms)
    return { host, posts, f }
  }

  it('throttles snapshots to about 30 Hz and sends only changed values', () => {
    const { posts } = run(demoProject(), 1000)
    expect(posts.length).toBeGreaterThan(20)
    expect(posts.length).toBeLessThanOrEqual(34)
    expect(Object.keys(posts[0].values)).toContain('motor')
    // Nothing is running, so later snapshots carry no value changes.
    expect(Object.keys(posts[posts.length - 1].values)).toEqual([])
  })

  it('applies writes from the main thread and reports the seal-in', () => {
    const { host, posts, f } = run(demoProject(), 100)
    host.handle({ type: 'write', ref: 'StartPB', value: true })
    f.advance(50)
    host.handle({ type: 'write', ref: 'StartPB', value: false })
    f.advance(50)
    const last = posts[posts.length - 1]
    const merged = Object.assign({}, ...posts.map((p) => p.values))
    expect(merged.motor).toBe(true)
    expect(last.image.Q[0] & 1).toBe(1)
    expect(last.trace.length).toBeGreaterThan(0)
  })

  it('reports an overrun instead of spinning on a huge program', () => {
    const p = demoProject()
    const rung = parseRung('XIC(A)CPT(X,SQRT(X*X+1)*1.0001)OTE(B);')
    p.pous[0].routines[0].rungs = Array.from({ length: 40_000 }, (_, i) => ({ id: 100_000 + i, ...rung }))
    const posts: Snapshot[] = []
    const host = new SimHost((m) => m.type === 'snapshot' && posts.push(m), { scanBudgetMs: 5 })
    host.handle({ type: 'init', project: p, device: SIM, periodMs: 10 })
    const t0 = performance.now()
    // One scan through the private loop path: call the tick the loop would call.
    ;(host as unknown as { scan(dt: number): void }).scan(10)
    const took = performance.now() - t0
    host.flush()
    expect(posts[posts.length - 1].overrun).toBe(true)
    expect(host.stats().overruns).toBe(1)
    expect(took).toBeLessThan(250)
  })
})
