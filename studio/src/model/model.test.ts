// SPDX-License-Identifier: MPL-2.0
import { describe, expect, it } from 'vitest'
import {
  ProcessImage, addressTypeMismatch, createMissingTags, flatten, insertAfter, parseAddress, parseQuickEntry,
  parseRung, parseRungs, printRung, reconcileIds, removeElement, wrapInBranch, RungTextError,
} from '@/model'
import type { Contact, Series } from '@/model'

const ROUND_TRIP = [
  '[XIC(StartPB) ,XIC(Motor) ]XIO(StopPB)OTE(Motor);',
  'XIC(Motor)TON(RunTimer,5000,0);',
  'GRT(Level,2000)OTE(High);',
  'XIC(A)OTL(B);',
  'XIO(A)OTU(B);',
  'XIC(Go)ONS(GoOns)CTU(Parts,10,0);',
  'XIC(A)[XIC(B)[XIC(C) ,XIO(D) ] ,XIC(E) , ]OTE(F);',
  '[OTE(A) ,OTE(B) ];',
  'XICR(A)XICF(B)OTEN(C)OTER(D)OTEF(E);',
  'CPT(Out,(A+B)*2)MOV(5,X)EQU(A,B)NEQ(A,B)GEQ(A,B)LES(A,B)LEQ(A,B);',
  'ADD(A,1,A)SUB(A,1,A)MUL(A,2,A)DIV(A,2,A)RES(RunTimer)JSR(Sub);',
  'XIC(T.DN)OSR(S,O)TOF(T2,100,0)RTO(T3,100,0)CTD(C,5,0);',
  'ST("x := x + 1; (* a, b *)");',
  ';',
]

describe('rung text', () => {
  it.each(ROUND_TRIP)('round-trips %s', (text) => {
    expect(printRung(parseRung(text))).toBe(text)
  })

  it('accepts whitespace, lower case and a missing semicolon', () => {
    expect(printRung(parseRung(' [ xic(A) , xic( B ) ] ote(C) '))).toBe('[XIC(A) ,XIC(B) ]OTE(C);')
  })

  it('builds the expected tree for the seal-in rung', () => {
    const body = parseRung('[XIC(StartPB) ,XIC(Motor) ]XIO(StopPB)OTE(Motor);')
    expect(body.items.map((e) => e.type)).toEqual(['parallel', 'contact', 'coil'])
    const par = body.items[0]
    if (par.type !== 'parallel') throw new Error('not parallel')
    expect(par.branches.map((b) => (b.items[0] as Contact).tag)).toEqual(['StartPB', 'Motor'])
  })

  it('maps ? to an empty operand and back', () => {
    const body = parseRung('XIC(?)TON(?,1000,0);')
    expect((body.items[0] as Contact).tag).toBe('')
    expect(printRung(body)).toBe('XIC(?)TON(?,1000,0);')
  })

  it('reports errors with a position', () => {
    expect(() => parseRung('XIC(A')).toThrow(RungTextError)
    expect(() => parseRung('FOO(A)')).toThrow(/unknown instruction FOO/)
    expect(() => parseRung('TON(A,1)')).toThrow(/takes 3 operands/)
    expect(() => parseRung('[XIC(A) ,XIC(B)')).toThrow(/expected '\]'/)
    expect(() => parseRung('XIC(A)]')).toThrow(/unexpected/)
    try {
      parseRung('XIC(A)BAD(B)')
    } catch (e) {
      expect((e as RungTextError).offset).toBe(6)
    }
  })

  it('parses several rungs', () => {
    expect(parseRungs('XIC(A)OTE(B);XIC(B)OTE(C);').length).toBe(2)
  })
})

describe('quick entry', () => {
  it('reads mnemonic sequences', () => {
    expect(printRung(parseQuickEntry('XIC Start XIO Stop OTE Motor'))).toBe('XIC(Start)XIO(Stop)OTE(Motor);')
    expect(printRung(parseQuickEntry('xic Motor ton RunTimer 5000 0'))).toBe('XIC(Motor)TON(RunTimer,5000,0);')
  })
  it('reads branches', () => {
    expect(printRung(parseQuickEntry('BST XIC Start NXB XIC Motor BND XIO Stop OTE Motor'))).toBe(
      '[XIC(Start) ,XIC(Motor) ]XIO(Stop)OTE(Motor);',
    )
    expect(printRung(parseQuickEntry('[ XIC A , XIC B ] OTE C'))).toBe('[XIC(A) ,XIC(B) ]OTE(C);')
  })
  it('pads missing operands with ?', () => {
    expect(printRung(parseQuickEntry('XIC A TON T'))).toBe('XIC(A)TON(T,?,?);')
  })
  it('falls back to rung text', () => {
    expect(printRung(parseQuickEntry('XIC(A)OTE(B)'))).toBe('XIC(A)OTE(B);')
  })
})

describe('tree operations', () => {
  const seal = () => parseRung('[XIC(StartPB) ,XIC(Motor) ]XIO(StopPB)OTE(Motor);')

  it('flattens depth first', () => {
    expect(flatten(seal()).map((e) => e.type)).toEqual(['parallel', 'contact', 'contact', 'contact', 'coil'])
  })

  it('inserts inputs before trailing outputs when there is no anchor', () => {
    const b = insertAfter(seal(), null, { type: 'contact', id: 'x', kind: 'no', tag: 'Ok' })
    expect(printRung(b)).toBe('[XIC(StartPB) ,XIC(Motor) ]XIO(StopPB)XIC(Ok)OTE(Motor);')
  })

  it('inserts after an anchor inside a branch', () => {
    const s = seal()
    const motorContact = flatten(s)[2]
    const b = insertAfter(s, motorContact.id, { type: 'contact', id: 'x', kind: 'nc', tag: 'Fault' })
    expect(printRung(b)).toBe('[XIC(StartPB) ,XIC(Motor)XIO(Fault) ]XIO(StopPB)OTE(Motor);')
  })

  it('collapses a parallel left with one branch after delete', () => {
    const s = seal()
    const b = removeElement(s, flatten(s)[2].id)
    expect(printRung(b)).toBe('XIC(StartPB)XIO(StopPB)OTE(Motor);')
  })

  it('wraps an element in a branch and adds branches', () => {
    const s = parseRung('XIC(A)OTE(B);')
    const { body, parallelId } = wrapInBranch(s, flatten(s)[0].id)
    expect(printRung(body)).toBe('[XIC(A) , ]OTE(B);')
    expect(printRung(wrapInBranch(body, parallelId).body)).toBe('[XIC(A) , , ]OTE(B);')
  })

  it('keeps ids across a text edit where the shape matches', () => {
    const s = seal()
    const edited = reconcileIds(s, parseRung('[XIC(StartPB) ,XIC(Motor) ]XIO(StopPB)XIO(Fault)OTE(Motor);'))
    const a = flatten(s)
    const b = flatten(edited)
    expect(b.slice(0, 4).map((e) => e.id)).toEqual(a.slice(0, 4).map((e) => e.id))
  })

  it('does not mutate its input', () => {
    const s = seal()
    const before = JSON.stringify(s)
    removeElement(s, flatten(s)[1].id)
    insertAfter(s, null, { type: 'coil', id: 'y', kind: 'normal', tag: 'Z' })
    expect(JSON.stringify(s)).toBe(before)
  })

  it('creates missing tags with inferred types', () => {
    const body: Series = parseRung('XIC(Go)XIC(RunTimer.DN)TON(RunTimer,5000,0)CTU(Count,3,0)ADD(A,1,Sum)OTE(Lamp);')
    const { created } = createMissingTags([{ name: 'go', type: 'BOOL', initial: 'FALSE', comment: '' }], body)
    expect(created.map((t) => [t.name, t.type])).toEqual([
      ['RunTimer', 'TIMER'], ['Count', 'COUNTER'], ['A', 'DINT'], ['Sum', 'DINT'], ['Lamp', 'BOOL'],
    ])
  })
})

describe('addresses and the process image', () => {
  it('parses direct addresses', () => {
    expect(parseAddress('%IX0.7')).toMatchObject({ area: 'I', size: 'X', byte: 0, bit: 7 })
    expect(parseAddress('%MW3')).toMatchObject({ area: 'M', size: 'W', byte: 6, width: 2 })
    expect(parseAddress('%QD1')).toMatchObject({ byte: 4, width: 4 })
    expect(parseAddress('%IX0.8')).toBeUndefined()
    expect(parseAddress('%IW2.1')).toBeUndefined()
    expect(parseAddress('Motor')).toBeUndefined()
  })

  it('reads and writes bits and little-endian words', () => {
    const img = new ProcessImage()
    img.write(parseAddress('%MW1')!, 0x0102)
    expect(img.M[2]).toBe(0x02)
    expect(img.M[3]).toBe(0x01)
    expect(img.read(parseAddress('%MX2.1')!)).toBe(true)
    img.write(parseAddress('%MX2.0')!, true)
    expect(img.read(parseAddress('%MW1')!, 'INT')).toBe(0x0103)
    img.write(parseAddress('%IW1')!, -2)
    expect(img.read(parseAddress('%IW1')!, 'INT')).toBe(-2)
    expect(img.read(parseAddress('%IW1')!, 'UINT')).toBe(0xfffe)
  })

  it('flags type mismatches', () => {
    expect(addressTypeMismatch('%IX0.0', 'BOOL')).toBeUndefined()
    expect(addressTypeMismatch('%IW2', 'BOOL')).toMatch(/does not fit/)
    expect(addressTypeMismatch('%QX0.0', 'INT')).toMatch(/does not fit/)
    expect(addressTypeMismatch('bogus', 'INT')).toMatch(/not a direct address/)
  })
})
