// SPDX-License-Identifier: MPL-2.0
/// <reference types="node" />
//
// Golden tests against plcc itself (packages/plcc-wasm; skipped when its
// wasm has not been built): the studio's model is plcc-ladder's, its JSON
// round-trips through plcc unchanged, its rung text reads the way plcc reads
// it, its catalog is plcc's, and the demo project checks clean.

import { existsSync, readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { beforeAll, describe, expect, it } from 'vitest'
import { catalog, check, convert, load } from '@plcc/plcc-wasm'
import {
  LOGIX_CATALOG, demoProject, fromLadderJson, newId, newTag, parseRung, toLadderJson, type Element, type Project,
} from '@/model'

const here = dirname(fileURLToPath(import.meta.url))
const wasm = join(here, '../../../packages/plcc-wasm/pkg/plcc_wasm_bg.wasm')
const built = existsSync(wasm)

/** A project using every element kind and optional field. */
export function featureProject(): Project {
  const p = demoProject()
  const rung = (text: string, comment?: string) => ({ id: newId(), ...(comment ? { comment } : {}), ...parseRung(text) })
  p.pous.push({
    id: newId(),
    name: 'Second',
    kind: 'program',
    variables: [newTag('Count', 'COUNTER', { section: 'local', comment: 'parts' }), { ...newTag('Ref', 'DINT'), section: 'local', initial: '5', constant: true }],
    routines: [
      {
        id: newId(),
        name: 'Main',
        rungs: [
          rung('XIC(StartPB)ONS(Os)CTU(Count,10,0);', 'count starts'),
          rung('LBL(Again)[XIC(Count.DN) ,XIO(StopPB)XIC(Motor) , ]JSR(Calc,0);'),
          rung('LES(Count.ACC,Ref)JMP(Again);'),
          { id: newId(), elements: [{ type: 'st', id: newId(), code: 'Ref := Ref + 1;', notes: ['a note'] } as Element] },
          rung('RET();'),
        ],
      },
      { id: newId(), name: 'Calc', rungs: [{ id: newId(), elements: [{ type: 'st', id: newId(), code: 'Ref := Count.ACC * 2;\nRef := Ref + 1;' }] }] },
    ],
  })
  p.globals.push(newTag('Os', 'BOOL'), newTag('Flags', 'DINT[4]', { initial: '[0,0,0,0]' }))
  p.tasks.push({ name: 'Slow', interval_ms: 100, priority: 5, programs: ['Second'] })
  return p
}

function modelPart(p: Project) {
  const { devices: _d, deviceFiles: _f, ...m } = p
  void _d
  void _f
  return m
}

function stripIds(v: unknown): unknown {
  if (Array.isArray(v)) return v.map(stripIds)
  if (v && typeof v === 'object') {
    const o: Record<string, unknown> = {}
    for (const [k, x] of Object.entries(v)) if (k !== 'id') o[k] = stripIds(x)
    return o
  }
  return v
}

describe.skipIf(!built)('golden: the studio model is plcc-ladder\'s', () => {
  beforeAll(async () => {
    await load(readFileSync(wasm))
  })

  it('uses plcc\'s Logix catalog (src/model/logix-catalog.json is current)', async () => {
    expect(LOGIX_CATALOG).toEqual(await catalog('logix'))
  })

  it.each([['demo', demoProject], ['every element kind', featureProject]])('%s project round-trips through plcc convert', async (_n, make) => {
    const p = make()
    const json = toLadderJson(p)
    const r = await convert({ files: { 'project.json': json }, to: 'ladder-json' })
    expect(r.ok, JSON.stringify(r.diagnostics)).toBe(true)
    expect(JSON.parse(r.output!)).toEqual(JSON.parse(json))
    expect(fromLadderJson(r.output!)).toEqual(modelPart(p))
  })

  it('reads rung text the way plcc reads it', async () => {
    // TS parse → model → L5X (plcc writes the rung text) → plcc reads it back.
    const p = featureProject()
    const viaPlcc = await convert({ files: { 'p.json': toLadderJson({ ...p, pous: p.pous.map((x) => ({ ...x, routines: x.routines.filter((r) => r.name !== 'Calc') })) }) }, to: 'l5x' })
    expect(viaPlcc.ok, JSON.stringify(viaPlcc.diagnostics)).toBe(true)
    const back = await convert({ files: { 'p.L5X': viaPlcc.output! }, to: 'ladder-json' })
    expect(back.ok).toBe(true)
    const model = JSON.parse(back.output!)
    const ours = JSON.parse(toLadderJson(p))
    // ST boxes come back as JSRs to ST routines (the L5X form); compare ladder rungs only.
    const ladder = (m: { pous: { name: string; routines: { name: string; rungs: { elements: { type: string }[] }[] }[] }[] }) =>
      m.pous.map((x) => x.routines.filter((r) => !r.name.includes('_ST') && r.name !== 'Calc').map((r) => r.rungs.filter((g) => !g.elements.some((e) => e.type === 'st' || (e as { name?: string }).name === 'JSR' && JSON.stringify(e).includes('_ST')))))
    expect(stripIds(ladder(model))).toEqual(stripIds(ladder(ours)))
  })

  it('the demo project checks clean, with its addresses and task', async () => {
    const r = await check({ files: { 'project.json': toLadderJson(demoProject()) }, entry: ['project.json'] })
    expect(r.diagnostics).toEqual([])
    expect(r.ok).toBe(true)
    expect(r.tags?.image.map((t) => t.address).sort()).toEqual(['%IW2', '%MX0.0', '%MX2.0', '%QX0.0', '%QX0.4'])
    expect(r.tags?.tasks.find((t) => t.name.toUpperCase() === 'MAINTASK')?.interval_ns).toBe(10_000_000)
  })

  it('a broken rung has diagnostics on its elements', async () => {
    const p = demoProject()
    const rung = p.pous[0].routines[0].rungs[1]
    rung.elements = parseRung('XIC(Motor)TON(Motor,5000,0)OTE(Nowhere.DN);').elements
    const r = await check({ files: { 'project.json': toLadderJson(p) }, entry: ['project.json'] })
    expect(r.ok).toBe(false)
    const at = r.diagnostics.map((d) => [d.ladder?.rung, d.ladder?.element, d.message])
    expect(at).toContainEqual([rung.id, rung.elements[1].id, expect.stringContaining('TIMER')])
    expect(at).toContainEqual([rung.id, rung.elements[2].id, expect.stringContaining('Nowhere')])
  })
})
