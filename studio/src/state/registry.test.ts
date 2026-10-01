// SPDX-License-Identifier: MPL-2.0
// The command registry: menus, keys and the palette all run these commands.
import { beforeEach, describe, expect, it } from 'vitest'
import { demoProject, flatten, printRung } from '@/model'
import { currentRoutine, useEditor } from './editor'
import { COMMANDS, command, commandForKey, keyName, menuFor, runCommand, type MenuEntry, type Target } from './registry'

function routine() {
  const r = currentRoutine(useEditor.getState())
  if (!r) throw new Error('no routine')
  return r
}
const texts = () => routine().rungs.map((r) => printRung(r))

function titles(entries: MenuEntry[]): string[] {
  return entries.flatMap((e) => ('separator' in e ? ['—'] : 'submenu' in e ? [`${e.submenu} ▸`] : [e.command.title]))
}

function contact(tag: string): Target {
  for (const r of routine().rungs) {
    const e = flatten(r.elements).find((x) => x.type === 'contact' && x.operand === tag)
    if (e) return { kind: 'element', rungId: r.id, elementId: e.id }
  }
  throw new Error(tag)
}

beforeEach(() => {
  useEditor.getState().openProject('test', demoProject())
  useEditor.getState().setMode('offline')
})

describe('command registry', () => {
  it('has unique ids and a group for every command', () => {
    const ids = COMMANDS.map((c) => c.id)
    expect(new Set(ids).size).toBe(ids.length)
    for (const c of COMMANDS) expect(c.group).toBeTruthy()
  })

  it('builds the element menu, with Change type, Insert ▸ and no live items offline', () => {
    const t = contact('StopPB')
    const m = titles(menuFor(t))
    expect(m.slice(0, 4)).toEqual(['Edit operand…', 'Change type ▸', 'Insert before ▸', 'Insert after ▸'])
    expect(m).toContain('Add branch around')
    expect(m).toContain('Delete')
    expect(m).toContain('Go to tag')
    expect(m).not.toContain('Force ON')
    const swap = menuFor(t).find((e) => 'submenu' in e && e.submenu === 'Change type')
    expect(swap && 'submenu' in swap && swap.items.map((i) => ('command' in i ? i.command.title : ''))).toEqual(['XIC'])
  })

  it('Change type → XIC turns XIO(StopPB) into XIC(StopPB), keeping its id', () => {
    const t = contact('StopPB')
    expect(runCommand('element.changeType:XIC', t)).toBe(true)
    expect(texts()[0]).toBe('[XIC(StartPB) ,XIC(Motor) ]XIC(StopPB)OTE(Motor);')
    expect(t.kind === 'element' && flatten(routine().rungs[0].elements).some((e) => e.id === t.elementId && e.type === 'contact' && e.kind === 'no')).toBe(true)
    useEditor.getState().undo()
    expect(texts()[0]).toBe('[XIC(StartPB) ,XIC(Motor) ]XIO(StopPB)OTE(Motor);')
  })

  it('changes box types with their operands (TON → TOF)', () => {
    const r1 = routine().rungs[1]
    const ton = flatten(r1.elements)[1]
    runCommand('element.changeType:TOF', { kind: 'element', rungId: r1.id, elementId: ton.id })
    expect(texts()[1]).toBe('XIC(Motor)TOF(RunTimer,5000,0);')
  })

  it('inserts before / after an element from the submenu', () => {
    const t = contact('StopPB')
    runCommand('element.insertBefore:XIO', t)
    expect(texts()[0]).toBe('[XIC(StartPB) ,XIC(Motor) ]XIO(?)XIO(StopPB)OTE(Motor);')
    runCommand('element.insertAfter:ONS', contact('StopPB'))
    expect(texts()[0]).toBe('[XIC(StartPB) ,XIC(Motor) ]XIO(?)XIO(StopPB)ONS(?)OTE(Motor);')
  })

  it('copies, pastes, duplicates and deletes elements and rungs', () => {
    runCommand('element.copy', contact('StopPB'))
    const r2 = routine().rungs[2]
    const grt = flatten(r2.elements)[0]
    runCommand('element.paste', { kind: 'element', rungId: r2.id, elementId: grt.id })
    expect(texts()[2]).toBe('GRT(Level,2000)XIO(StopPB)OTE(High);')
    runCommand('element.duplicate', contact('Motor'))
    expect(texts()[0]).toBe('[XIC(StartPB) ,XIC(Motor)XIC(Motor) ]XIO(StopPB)OTE(Motor);')
    runCommand('rung.copy', { kind: 'rung', rungId: routine().rungs[1].id })
    runCommand('rung.paste', { kind: 'rung', rungId: routine().rungs[2].id })
    expect(texts()).toHaveLength(4)
    expect(texts()[3]).toBe('XIC(Motor)TON(RunTimer,5000,0);')
    runCommand('rung.delete', { kind: 'rung', rungId: routine().rungs[3].id })
    expect(texts()).toHaveLength(3)
  })

  it('inserts rungs above and below, moves and duplicates them', () => {
    const first = routine().rungs[0].id
    runCommand('rung.insertAbove', { kind: 'rung', rungId: first })
    expect(texts()[0]).toBe(';')
    runCommand('rung.moveDown', { kind: 'rung', rungId: routine().rungs[0].id })
    expect(texts()[1]).toBe(';')
    runCommand('rung.duplicate', { kind: 'rung', rungId: first })
    expect(texts()[1]).toBe(texts()[0])
  })

  it('offers live commands only in Simulate / Online, for BOOL tags', () => {
    useEditor.getState().setMode('simulate')
    const m = titles(menuFor(contact('StartPB')))
    expect(m).toContain('Toggle')
    expect(m).toContain('Force ON')
    expect(m).toContain('Force OFF')
    const r2 = routine().rungs[2]
    expect(titles(menuFor({ kind: 'element', rungId: r2.id, elementId: flatten(r2.elements)[0].id }))).not.toContain('Toggle')
  })

  it('maps keys to the same commands, Mod = Ctrl or Cmd', () => {
    const t = contact('StopPB')
    expect(keyName({ key: 'c', ctrlKey: true, metaKey: false, shiftKey: false, altKey: false })).toBe('Mod+c')
    expect(keyName({ key: 'c', ctrlKey: false, metaKey: true, shiftKey: false, altKey: false })).toBe('Mod+c')
    expect(keyName({ key: 'N', ctrlKey: false, metaKey: false, shiftKey: true, altKey: false })).toBe('Shift+N')
    expect(commandForKey('Mod+c', t)?.id).toBe('element.copy')
    expect(commandForKey('Delete', t)?.id).toBe('element.delete')
    expect(commandForKey('Enter', t)?.id).toBe('element.edit')
    expect(commandForKey('Delete', { kind: 'rung', rungId: 1 })?.id).toBe('rung.delete')
    // Menus show each command's shortcut.
    expect(command('element.copy')?.shortcut).toBe('Ctrl+C')
  })

  it('has routine, tag and device menus', () => {
    expect(titles(menuFor({ kind: 'routine', program: 'MainProgram', routine: 'MainRoutine' }))).toEqual([
      'Open', 'Rename…', 'Duplicate', 'Export ▸', '—', 'Delete',
    ])
    expect(titles(menuFor({ kind: 'tag', tag: 'Motor' }))).toEqual(['Go to usages', 'Rename…', 'Change address / map to terminal…', 'Copy name'])
    expect(titles(menuFor({ kind: 'device', device: 'Opta' }))).toEqual(['Detect over USB…', 'Change device…', 'View manifest', 'Update manifest…'])
    runCommand('routine.duplicate', { kind: 'routine', program: 'MainProgram', routine: 'MainRoutine' })
    const p = useEditor.getState().project
    expect(p.pous[0].routines.map((r) => r.name)).toEqual(['MainRoutine', 'MainRoutine_2'])
    // Fresh ids for the copy.
    const ids = p.pous[0].routines.flatMap((r) => r.rungs.flatMap((g) => [g.id, ...flatten(g.elements).map((e) => e.id)]))
    expect(new Set(ids).size).toBe(ids.length)
  })

  it('goes to a tag\'s usages one after another', () => {
    runCommand('tag.usages', { kind: 'tag', tag: 'Motor' })
    const first = useEditor.getState().selection.elementId
    runCommand('tag.usages', { kind: 'tag', tag: 'Motor' })
    expect(useEditor.getState().selection.elementId).not.toBe(first)
  })
})
