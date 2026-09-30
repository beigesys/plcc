// SPDX-License-Identifier: MPL-2.0
import { describe, expect, it } from 'vitest'
import { demoProject, parseRung } from '@/model'
import { readRef } from '@/state/live'
import { G, allNodes, layoutRung } from './ladder/layout'
import { suggest } from './TagInput'

describe('ladder layout', () => {
  it('puts trailing outputs against the right rail', () => {
    const l = layoutRung(parseRung('XIC(A)XIO(B)OTE(C);'), 1000)
    const nodes = allNodes(l)
    const coil = nodes[nodes.length - 1]
    expect(coil.el.type).toBe('coil')
    expect(coil.x + coil.w).toBe(1000 - G.railPad)
    expect(l.stretchFrom).toBe(G.railX + 2 * G.contactW)
  })

  it('stacks parallel branches and shares one wire height', () => {
    const l = layoutRung(parseRung('[XIC(A) ,XIC(B) ,XIC(C) ]OTE(D);'), 600)
    const par = allNodes(l)[0]
    expect(par.branches?.length).toBe(3)
    const ys = par.branches?.map((b) => b.wy) ?? []
    expect(ys[0]).toBe(l.wy)
    expect(ys[1]).toBeGreaterThan(ys[0])
    expect(ys[2]).toBeGreaterThan(ys[1])
    expect(l.height).toBeGreaterThan(3 * G.contactH)
  })

  it('grows past the requested width when the rung needs it', () => {
    const text = `${'XIC(A)'.repeat(20)}OTE(B);`
    expect(layoutRung(parseRung(text), 400).width).toBeGreaterThan(20 * G.contactW)
  })
})

describe('tag suggestions', () => {
  const tags = demoProject().tags
  it('ranks prefix matches first', () => {
    expect(suggest(tags, 'st').map((s) => s.value)).toEqual(['StartPB', 'StopPB'])
    expect(suggest(tags, 'mot')[0].value).toBe('Motor')
  })
  it('offers structure members', () => {
    expect(suggest(tags, 'RunTimer.').map((s) => s.value)).toContain('RunTimer.DN')
    expect(suggest(tags, 'runtimer.a').map((s) => s.value)).toEqual(['RunTimer.ACC'])
    expect(suggest(tags, 'Motor.')).toEqual([])
  })
})

describe('live value reads', () => {
  const values = { motor: true, level: 5, runtimer: { PRE: 5000, ACC: 10, DN: false } }
  it('reads tags, members and bits', () => {
    expect(readRef(values, 'Motor')).toBe(true)
    expect(readRef(values, 'RunTimer.acc')).toBe(10)
    expect(readRef(values, 'Level.0')).toBe(true)
    expect(readRef(values, 'Level.1')).toBe(false)
    expect(readRef(values, 'Nope')).toBeUndefined()
  })
})
