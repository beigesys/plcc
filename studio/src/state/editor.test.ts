// SPDX-License-Identifier: MPL-2.0
import { beforeEach, describe, expect, it } from 'vitest'
import { demoProject, flatten, printRung, type Project } from '@/model'
import {
  addRung, addRungFromQuickEntry, deleteSelection, insertInstruction, moveRung, moveSelection, setRungText,
  updateInstruction, wrapSelectionInBranch,
} from './commands'
import { currentRoutine, useEditor } from './editor'

function routine() {
  const r = currentRoutine(useEditor.getState())
  if (!r) throw new Error('no routine')
  return r
}
const texts = () => routine().rungs.map((r) => printRung(r))
const tag = (p: Project, n: string) => p.globals.find((t) => t.name === n)

beforeEach(() => {
  useEditor.getState().openProject('test', demoProject())
})

describe('editing commands', () => {
  it('adds a rung from quick entry and creates its tags', () => {
    addRungFromQuickEntry('XIC Start XIO Stop OTE Lamp')
    expect(texts()[3]).toBe('XIC(Start)XIO(Stop)OTE(Lamp);')
    const p = useEditor.getState().project
    expect(tag(p, 'Start')?.data_type).toBe('BOOL')
    expect(tag(p, 'Lamp')?.data_type).toBe('BOOL')
  })

  it('undoes and redoes', () => {
    addRungFromQuickEntry('XIC A OTE B')
    expect(texts().length).toBe(4)
    useEditor.getState().undo()
    expect(texts().length).toBe(3)
    expect(tag(useEditor.getState().project, 'A')).toBeUndefined()
    useEditor.getState().redo()
    expect(texts().length).toBe(4)
  })

  it('inserts after the selection, then fills in the operand', () => {
    const r0 = routine().rungs[0]
    const stop = flatten(r0.elements).find((e) => e.type === 'contact' && e.operand === 'StopPB')
    useEditor.getState().select({ rungId: r0.id, elementId: stop?.id ?? null })
    insertInstruction('XIO')
    const s = useEditor.getState()
    expect(s.editing?.elementId).toBe(s.selection.elementId)
    const id = s.selection.elementId ?? -1
    updateInstruction(r0.id, id, (e) => (e.type === 'contact' ? { ...e, operand: 'Fault' } : e))
    expect(texts()[0]).toBe('[XIC(StartPB) ,XIC(Motor) ]XIO(StopPB)XIO(Fault)OTE(Motor);')
    expect(tag(useEditor.getState().project, 'Fault')?.data_type).toBe('BOOL')
  })

  it('creates a TIMER for a new TON', () => {
    addRungFromQuickEntry('XIC Go TON T2 1000 0')
    expect(tag(useEditor.getState().project, 'T2')?.data_type).toBe('TIMER')
  })

  it('wraps the selection in a branch and deletes back', () => {
    const r2 = routine().rungs[2]
    const grt = flatten(r2.elements)[0]
    useEditor.getState().select({ rungId: r2.id, elementId: grt.id })
    wrapSelectionInBranch()
    expect(texts()[2]).toBe('[GRT(Level,2000) , ]OTE(High);')
    insertInstruction('XIC', { rungId: r2.id, afterId: null, edit: false })
    deleteSelection()
    expect(texts()[2]).toBe('[GRT(Level,2000) , ]OTE(High);')
  })

  it('edits through rung text and keeps ids', () => {
    const r1 = routine().rungs[1]
    const before = flatten(r1.elements).map((e) => e.id)
    setRungText(r1.id, 'XIC(Motor)XIO(StopPB)TON(RunTimer,3000,0);')
    const after = flatten(routine().rungs[1].elements)
    expect(after[0].id).toBe(before[0])
    expect(printRung(routine().rungs[1])).toBe('XIC(Motor)XIO(StopPB)TON(RunTimer,3000,0);')
    expect(() => setRungText(r1.id, 'XIC(')).toThrow()
  })

  it('moves rungs and the selection', () => {
    const [a, b] = routine().rungs
    moveRung(a.id, 1)
    expect(routine().rungs[0].id).toBe(b.id)
    useEditor.getState().select({ rungId: null, elementId: null })
    moveSelection('down')
    expect(useEditor.getState().selection.rungId).toBe(routine().rungs[0].id)
    moveSelection('right')
    moveSelection('down')
    expect(useEditor.getState().selection.rungId).toBe(routine().rungs[1].id)
  })

  it('adds and deletes a rung', () => {
    const id = addRung()
    expect(texts().length).toBe(4)
    useEditor.getState().select({ rungId: id ?? null, elementId: null })
    deleteSelection()
    expect(texts().length).toBe(3)
  })
})
