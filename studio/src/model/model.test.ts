// SPDX-License-Identifier: MPL-2.0
import { describe, expect, it } from 'vitest'
import {
  ProcessImage, addressTypeMismatch, createMissingTags, flatten, insertAfter, newTag, parseAddress, parseQuickEntry,
  parseRung, parseRungs, printRung, reconcileIds, removeElement, wrapInBranch, RungTextError,
} from '@/model'
import type { Block, Contact, Element } from '@/model'

// Canonical Logix rung text: what plcc's rll::write produces and Studio 5000 exports.
const ROUND_TRIP = [
  '[XIC(StartPB) ,XIC(Motor) ]XIO(StopPB)OTE(Motor);',
  'XIC(Motor)TON(RunTimer,5000,0);',
  'GRT(Level,2000)OTE(High);',
  'XIC(A)OTL(B);',
  'XIO(A)OTU(B);',
  'XIC(Go)ONS(GoOns)CTU(Parts,10,0);',
  'XIC(A)[XIC(B)[XIC(C) ,XIO(D) ] ,XIC(E) , ]OTE(F);',
  '[OTE(A) ,OTE(B) ];',
  'CPT(Out,(A+B)*2)MOV(5,X)EQU(A,B)NEQ(A,B)GEQ(A,B)LES(A,B)LEQ(A,B);',
  'ADD(A,1,A)SUB(A,1,A)MUL(A,2,A)DIV(A,2,A)RES(RunTimer)JSR(Sub,0);',
  'XIC(T.DN)OSR(S,O)TOF(T2,100,0)RTO(T3,100,0)CTD(C,5,0);',
  'LBL(Top)XIC(a)JMP(Top)RET();',
  'NOP()MSG(Msg1)COP(Src[0],Dst[0],10);',
  'ST("x := x + 1; (* a, b *)");',
  ';',
]

describe('rung text', () => {
  it.each(ROUND_TRIP)('round-trips %s', (text) => {
    expect(printRung(parseRung(text))).toBe(text)
  })

  it('accepts whitespace and a missing semicolon; instruction names keep their spelling', () => {
    expect(printRung(parseRung(' [ XIC(A) , XIC( B ) ] OTE(C) '))).toBe('[XIC(A) ,XIC(B) ]OTE(C);')
    // plcc reads only the exact upper-case spelling as a contact.
    expect(parseRung('xic(A);').elements[0].type).toBe('block')
  })

  it('builds the expected tree for the seal-in rung', () => {
    const r = parseRung('[XIC(StartPB) ,XIC(Motor) ]XIO(StopPB)OTE(Motor);')
    expect(r.elements.map((e) => e.type)).toEqual(['branch', 'contact', 'coil'])
    const br = r.elements[0]
    if (br.type !== 'branch') throw new Error('not a branch')
    expect(br.legs.map((l) => (l[0] as Contact).operand)).toEqual(['StartPB', 'Motor'])
  })

  it('names block pins from plcc catalog', () => {
    const r = parseRung('TON(T1,500,0)ADD(a,b,c)JSR(Sub,1,x,y)FOO(p,q);')
    const names = (i: number) => (r.elements[i] as Block).pins.map((p) => p.name)
    expect(names(0)).toEqual(['Timer', 'Preset', 'Accum'])
    expect(names(1)).toEqual(['Source A', 'Source B', 'Dest'])
    expect(names(2)).toEqual(['Routine Name', 'Input Count', 'Parameter 1', 'Parameter 2'])
    expect(names(3)).toEqual(['Operand 1', 'Operand 2'])
  })

  it('keeps ? as the operand text, as plcc does', () => {
    const r = parseRung('XIC(?)TON(?,1000,0);')
    expect((r.elements[0] as Contact).operand).toBe('?')
    expect(printRung(r)).toBe('XIC(?)TON(?,1000,0);')
  })

  it('reports errors with a position', () => {
    expect(() => parseRung('XIC(A')).toThrow(RungTextError)
    expect(() => parseRung('[XIC(A) ,XIC(B)')).toThrow(/unterminated branch|expected/)
    expect(() => parseRung('XIC(A)]')).toThrow(/unexpected/)
    try {
      parseRung('XIC(A)9BAD(B)')
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
    expect(() => parseQuickEntry('FROB A')).toThrow(/unknown instruction/)
  })
})

describe('tree operations', () => {
  const seal = () => parseRung('[XIC(StartPB) ,XIC(Motor) ]XIO(StopPB)OTE(Motor);').elements
  const text = (elements: Element[]) => printRung({ elements })

  it('flattens depth first', () => {
    expect(flatten(seal()).map((e) => e.type)).toEqual(['branch', 'contact', 'contact', 'contact', 'coil'])
  })

  it('inserts inputs before trailing outputs when there is no anchor', () => {
    const b = insertAfter(seal(), null, { type: 'contact', id: 999, kind: 'no', operand: 'Ok' })
    expect(text(b)).toBe('[XIC(StartPB) ,XIC(Motor) ]XIO(StopPB)XIC(Ok)OTE(Motor);')
  })

  it('inserts after an anchor inside a branch', () => {
    const s = seal()
    const motorContact = flatten(s)[2]
    const b = insertAfter(s, motorContact.id, { type: 'contact', id: 999, kind: 'nc', operand: 'Fault' })
    expect(text(b)).toBe('[XIC(StartPB) ,XIC(Motor)XIO(Fault) ]XIO(StopPB)OTE(Motor);')
  })

  it('collapses a branch left with one leg after delete', () => {
    const s = seal()
    expect(text(removeElement(s, flatten(s)[2].id))).toBe('XIC(StartPB)XIO(StopPB)OTE(Motor);')
  })

  it('wraps an element in a branch and adds legs', () => {
    const s = parseRung('XIC(A)OTE(B);').elements
    const { elements, branchId } = wrapInBranch(s, flatten(s)[0].id)
    expect(text(elements)).toBe('[XIC(A) , ]OTE(B);')
    expect(text(wrapInBranch(elements, branchId).elements)).toBe('[XIC(A) , , ]OTE(B);')
  })

  it('keeps ids across a text edit where the shape matches', () => {
    const s = seal()
    const edited = reconcileIds(s, parseRung('[XIC(StartPB) ,XIC(Motor) ]XIO(StopPB)XIO(Fault)OTE(Motor);').elements)
    expect(flatten(edited).slice(0, 4).map((e) => e.id)).toEqual(flatten(s).slice(0, 4).map((e) => e.id))
  })

  it('does not mutate its input', () => {
    const s = seal()
    const before = JSON.stringify(s)
    removeElement(s, flatten(s)[1].id)
    insertAfter(s, null, { type: 'coil', id: 998, kind: 'normal', operand: 'Z' })
    expect(JSON.stringify(s)).toBe(before)
  })

  it('creates missing tags with inferred types', () => {
    const r = parseRung('XIC(Go)XIC(RunTimer.DN)TON(RunTimer,5000,0)CTU(Count,3,0)ADD(A,1,Sum)OTE(Lamp);')
    const { created } = createMissingTags([newTag('go', 'BOOL')], r.elements)
    expect(created.map((t) => [t.name, t.data_type])).toEqual([
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
