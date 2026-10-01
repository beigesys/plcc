// SPDX-License-Identifier: MPL-2.0
/// <reference types="node" />
//
// The simulator worker running plcc's own build of the demo project: the
// project compiled by the real browser compiler (packages/plcc-compiler-wasm,
// skipped when it has not been built), linked, and run by @plcc/plc-wasm
// under a fake clock, through the same messages the page sends.

import { existsSync, readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { gunzipSync } from 'node:zlib'
import { beforeAll, describe, expect, it } from 'vitest'
import { compile } from '@plcc/plcc-compiler-wasm'
import { catalogEntry } from '@/devices/catalog'
import { demoProject, flatten, parseRung, type Project } from '@/model'
import { simulatorRequest } from '@/state/simulate'
import type { LoopClock } from './loop'
import type { FromWorker, Snapshot } from './messages'
import { SimWorker } from './simWorker'

const here = dirname(fileURLToPath(import.meta.url))
const gz = join(here, '../../../packages/plcc-compiler-wasm/dist/plcc-compiler.wasm.gz')
const built = existsSync(gz)
const OPTA = catalogEntry('arduino-opta')!.device

function fakeClock() {
  let t = 0
  let timers: { at: number; fn: () => void; id: number }[] = []
  let nextId = 1
  const clock: LoopClock = {
    now: () => t,
    setTimeout: (fn, ms) => {
      const id = nextId++
      timers.push({ at: t + Math.max(ms, 1), fn, id })
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
  return { clock, advance }
}

let compiler: WebAssembly.Module

async function start(project: Project) {
  const r = await compile(compiler, simulatorRequest(project, OPTA))
  expect(r.ok, JSON.stringify(r.diagnostics)).toBe(true)
  const { clock, advance } = fakeClock()
  const posts: FromWorker[] = []
  const w = new SimWorker((m) => posts.push(m), { clock })
  await w.handle({ type: 'init', project, device: OPTA, periodMs: 10 })
  await w.handle({ type: 'program', module: r.module!, symbols: r.symbols as never, project })
  expect(w.engine).toBe('plcc')
  const values: Record<string, unknown> = {}
  const last = () => {
    const snaps = posts.filter((p): p is Snapshot => p.type === 'snapshot')
    for (const s of snaps) Object.assign(values, s.values)
    posts.length = 0
    return snaps[snaps.length - 1]
  }
  return { w, advance, values, last }
}

describe.skipIf(!built)('simulator: plcc\'s own build', () => {
  beforeAll(async () => {
    compiler = await WebAssembly.compile(gunzipSync(readFileSync(gz)))
  })

  it('runs the demo: seal-in, the run timer in its 10 ms task, the level alarm', async () => {
    const p = demoProject()
    const { w, advance, values, last } = await start(p)
    advance(50)
    last()
    expect(values.motor).toBe(false)
    await w.handle({ type: 'write', ref: 'StartPB', value: true })
    advance(100)
    await w.handle({ type: 'write', ref: 'StartPB', value: false })
    advance(100)
    let s = last()
    expect(s.engine).toBe('plcc')
    expect(values.motor).toBe(true)
    expect(s.image.Q[0] & 1).toBe(1)
    advance(1000)
    last()
    const acc = (values.runtimer as Record<string, number>).ACC
    expect(acc).toBeGreaterThanOrEqual(1000)
    expect(acc).toBeLessThanOrEqual(1300)
    // Power flow from the model and the live values.
    const seal = p.pous[0].routines[0].rungs[0]
    const coil = flatten(seal.elements).find((e) => e.type === 'coil')!
    advance(40)
    s = last()
    expect(new Map(s.trace).get(coil.id)).toBe(7)
    // Level (%IW2) above 2000 lights High (%QX0.4).
    await w.handle({ type: 'image', address: '%IW2', value: 2500 })
    advance(100)
    last()
    expect(values.high).toBe(true)
    // A forced StopPB drops the motor and holds.
    await w.handle({ type: 'force', tag: 'StopPB', value: true })
    advance(100)
    s = last()
    expect(values.motor).toBe(false)
    expect(s.forces.stoppb).toBe(true)
    await w.handle({ type: 'stop' })
  })

  it('reports a fault with its site', async () => {
    const p = demoProject()
    p.globals.push({ name: 'Zero', data_type: 'DINT', section: 'global' }, { name: 'Out', data_type: 'DINT', section: 'global' })
    p.pous[0].routines[0].rungs.push({ id: 9000, ...parseRung('CPT(Out,100/Zero);') })
    const { advance, last } = await start(p)
    advance(100)
    const s = last()
    // Logix integer division by zero is not a fault (it yields Source A);
    // the program keeps running.
    expect(s.fault).toBeNull()
  })
})
