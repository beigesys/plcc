// SPDX-License-Identifier: MPL-2.0
//
// One registry of editor commands. The keyboard, the command palette and the
// context menus all run commands from here, so an action behaves the same
// whichever way it is invoked, and each shows the same shortcut everywhere.

import { findElement, findTag, isUnassigned, newId, tagReferences, withNewIds, type Element, type Id, type Routine } from '@/model'
import {
  addRung, changeType, changeTypeOptions, copyElement, copyRungs, cutElement, deleteElement, deleteRung, duplicateElement,
  duplicateRung, hasClipboard, insertInstruction, moveRung, paste, wrapInBranchAt,
} from './commands'
import { exportRoutine, type ExportFormat } from './convert'
import { currentRoutine, findProgram, mapRoutine, useEditor } from './editor'
import { canForce, forceTag, toggleTag } from './force'

/** What a command acts on. */
export type Target =
  | { kind: 'element'; rungId: Id; elementId: Id }
  | { kind: 'rung'; rungId: Id }
  | { kind: 'tag'; tag: string }
  | { kind: 'routine'; program: string; routine: string }
  | { kind: 'device'; device: string }
  | { kind: 'none' }

export interface Command {
  id: string
  title: string
  /** Shown in menus and the palette, e.g. `Ctrl+C`, `Del`. */
  shortcut?: string
  /** Keys that run it in the rung list: `c`, `Shift+C`, `Delete`, `Mod+c` (Ctrl or Cmd). */
  keys?: string[]
  group: string
  applies(t: Target): boolean
  enabled?(t: Target): boolean
  run(t: Target): void
}

export type MenuEntry = { command: Command } | { separator: true } | { submenu: string; items: MenuEntry[] }

function s() {
  return useEditor.getState()
}

function elementOf(t: Target): Element | undefined {
  if (t.kind !== 'element') return undefined
  const r = currentRoutine(s())?.rungs.find((g) => g.id === t.rungId)
  return r ? findElement(r.elements, t.elementId) : undefined
}

/** The tag an element's first operand names (`Timer.DN` → `Timer`). */
export function elementTag(e: Element | undefined): string | undefined {
  if (!e) return undefined
  const text = e.type === 'contact' || e.type === 'coil' ? e.operand : e.type === 'block' ? e.pins[0]?.value : undefined
  if (!text || isUnassigned(text)) return undefined
  const base = text.trim().split(/[.[]/)[0]
  return /^[A-Za-z_][A-Za-z0-9_]*$/.test(base) ? base : undefined
}

function boolTag(t: Target): string | undefined {
  const e = elementOf(t)
  if (!e || (e.type !== 'contact' && e.type !== 'coil')) return undefined
  const tag = elementTag(e)
  return tag && e.operand.trim() === tag ? tag : undefined
}

const live = () => s().mode !== 'offline'
const isEl = (t: Target) => t.kind === 'element'
const isRung = (t: Target) => t.kind === 'rung' || t.kind === 'element'
const rungIdOf = (t: Target): Id => (t.kind === 'rung' || t.kind === 'element' ? t.rungId : -1)

function insertCommand(where: 'before' | 'after', m: string): Command {
  return {
    id: `element.insert${where === 'before' ? 'Before' : 'After'}:${m}`,
    title: m,
    group: 'Insert',
    applies: isEl,
    run: (t) => {
      if (t.kind !== 'element') return
      if (where === 'before') insertInstruction(m, { rungId: t.rungId, beforeId: t.elementId })
      else insertInstruction(m, { rungId: t.rungId, afterId: t.elementId })
    },
  }
}

const INSERTABLE = ['XIC', 'XIO', 'OTE', 'OTL', 'OTU', 'ONS', 'TON', 'TOF', 'CTU', 'RES', 'GRT', 'EQU', 'ADD', 'MOV', 'CPT', 'JSR', 'ST']

function routineStRef(program: string, routine: string) {
  return { program, routine }
}

export const COMMANDS: Command[] = [
  // ---- element
  {
    id: 'element.edit', title: 'Edit operand…', shortcut: 'Enter', keys: ['Enter'], group: 'Edit', applies: isEl,
    enabled: (t) => {
      const e = elementOf(t)
      return !!e && e.type !== 'branch' && e.type !== 'return'
    },
    run: (t) => t.kind === 'element' && s().setEditing({ rungId: t.rungId, elementId: t.elementId }),
  },
  {
    id: 'element.branch', title: 'Add branch around', shortcut: 'P', keys: ['p', 'P'], group: 'Edit', applies: isEl,
    run: (t) => t.kind === 'element' && wrapInBranchAt(t.rungId, t.elementId),
  },
  {
    id: 'element.cut', title: 'Cut', shortcut: 'Ctrl+X', keys: ['Mod+x'], group: 'Edit', applies: isEl,
    run: (t) => t.kind === 'element' && cutElement(t.rungId, t.elementId),
  },
  {
    id: 'element.copy', title: 'Copy', shortcut: 'Ctrl+C', keys: ['Mod+c'], group: 'Edit', applies: isEl,
    run: (t) => t.kind === 'element' && copyElement(t.rungId, t.elementId),
  },
  {
    id: 'element.paste', title: 'Paste after', shortcut: 'Ctrl+V', keys: ['Mod+v'], group: 'Edit', applies: isRung,
    enabled: () => hasClipboard(),
    run: (t) => {
      if (t.kind === 'element') paste(t.rungId, t.elementId)
      else if (t.kind === 'rung') paste(t.rungId, null)
    },
  },
  {
    id: 'element.duplicate', title: 'Duplicate', shortcut: 'Ctrl+D', keys: ['Mod+d'], group: 'Edit', applies: isEl,
    run: (t) => t.kind === 'element' && duplicateElement(t.rungId, t.elementId),
  },
  {
    id: 'element.delete', title: 'Delete', shortcut: 'Del', keys: ['Delete', 'Backspace'], group: 'Edit', applies: isEl,
    run: (t) => t.kind === 'element' && deleteElement(t.rungId, t.elementId),
  },
  {
    id: 'element.goToTag', title: 'Go to tag', group: 'Navigate', applies: (t) => !!elementTag(elementOf(t)),
    run: (t) => {
      const tag = elementTag(elementOf(t))
      if (!tag) return
      s().setView({ kind: 'tags' })
      s().setFocus({ kind: 'tag', tag })
    },
  },
  {
    id: 'element.toggle', title: 'Toggle', shortcut: 'Space', keys: [' '], group: 'Live',
    applies: (t) => live() && !!boolTag(t),
    enabled: (t) => canForce(boolTag(t) ?? ''),
    run: (t) => {
      const tag = boolTag(t)
      if (tag) toggleTag(tag)
    },
  },
  {
    id: 'element.forceOn', title: 'Force ON', group: 'Live', applies: (t) => live() && !!boolTag(t),
    enabled: (t) => canForce(boolTag(t) ?? ''),
    run: (t) => {
      const tag = boolTag(t)
      if (tag) forceTag(tag, true)
    },
  },
  {
    id: 'element.forceOff', title: 'Force OFF', group: 'Live', applies: (t) => live() && !!boolTag(t),
    enabled: (t) => canForce(boolTag(t) ?? ''),
    run: (t) => {
      const tag = boolTag(t)
      if (tag) forceTag(tag, false)
    },
  },
  {
    id: 'element.showSt', title: 'Show in ST view', shortcut: 'Ctrl+Shift+S', keys: ['Mod+Shift+S'], group: 'Navigate', applies: isRung,
    run: () => s().setStView(true),
  },
  // ---- rung
  {
    id: 'rung.insertAbove', title: 'Insert rung above', shortcut: 'Shift+N', keys: ['Shift+N'], group: 'Rung', applies: isRung,
    run: (t) => addRung([], { before: rungIdOf(t) }),
  },
  {
    id: 'rung.insertBelow', title: 'Insert rung below', shortcut: 'N', keys: ['n'], group: 'Rung', applies: (t) => t.kind !== 'tag' && t.kind !== 'device',
    run: (t) => addRung([], { after: isRung(t) ? rungIdOf(t) : null }),
  },
  {
    id: 'rung.duplicate', title: 'Duplicate rung', group: 'Rung', applies: isRung,
    run: (t) => duplicateRung(rungIdOf(t)),
  },
  {
    id: 'rung.moveUp', title: 'Move rung up', shortcut: 'Alt+↑', group: 'Rung', applies: isRung,
    run: (t) => moveRung(rungIdOf(t), -1),
  },
  {
    id: 'rung.moveDown', title: 'Move rung down', shortcut: 'Alt+↓', group: 'Rung', applies: isRung,
    run: (t) => moveRung(rungIdOf(t), 1),
  },
  {
    id: 'rung.comment', title: 'Edit comment', group: 'Rung', applies: isRung,
    run: (t) => s().setFocus({ kind: 'comment', rungId: rungIdOf(t) }),
  },
  {
    id: 'rung.copy', title: 'Copy rung (as rung text)', shortcut: 'Ctrl+C', keys: ['Mod+c'], group: 'Rung', applies: (t) => t.kind === 'rung',
    run: (t) => copyRungs([rungIdOf(t)]),
  },
  {
    id: 'rung.paste', title: 'Paste rung below', shortcut: 'Ctrl+V', keys: ['Mod+v'], group: 'Rung', applies: (t) => t.kind === 'rung',
    enabled: () => hasClipboard(),
    run: (t) => paste(rungIdOf(t), null),
  },
  {
    id: 'rung.delete', title: 'Delete rung', shortcut: 'Del', keys: ['Delete', 'Backspace'], group: 'Rung', applies: (t) => t.kind === 'rung',
    run: (t) => deleteRung(rungIdOf(t)),
  },
  // ---- tag
  {
    id: 'tag.usages', title: 'Go to usages', group: 'Tag', applies: (t) => t.kind === 'tag',
    run: (t) => {
      if (t.kind !== 'tag') return
      const refs = tagReferences(s().project, t.tag)
      if (!refs.length) return s().notify(`${t.tag} is not used in any rung`)
      const view = s().view
      const cur = view.kind === 'routine' ? refs.findIndex((r) => r.element === s().selection.elementId) : -1
      const r = refs[(cur + 1) % refs.length]
      s().setView({ kind: 'routine', program: r.program, routine: r.routine })
      s().select({ rungId: r.rung, elementId: r.element })
      s().notify(`${t.tag}: usage ${((cur + 1) % refs.length) + 1} of ${refs.length}`)
    },
  },
  {
    id: 'tag.rename', title: 'Rename…', shortcut: 'F2', group: 'Tag', applies: (t) => t.kind === 'tag',
    run: (t) => {
      if (t.kind !== 'tag') return
      s().setView({ kind: 'tags' })
      s().setFocus({ kind: 'tag', tag: t.tag, rename: true })
    },
  },
  {
    id: 'tag.map', title: 'Change address / map to terminal…', group: 'Tag', applies: (t) => t.kind === 'tag',
    run: (t) => {
      if (t.kind !== 'tag') return
      s().setView({ kind: 'io', device: s().project.devices[0]?.name ?? '' })
      s().setFocus({ kind: 'point', tag: t.tag })
    },
  },
  {
    id: 'tag.forceOn', title: 'Force ON', group: 'Live', applies: (t) => t.kind === 'tag' && live(),
    enabled: (t) => t.kind === 'tag' && canForce(t.tag),
    run: (t) => t.kind === 'tag' && forceTag(t.tag, true),
  },
  {
    id: 'tag.forceOff', title: 'Force OFF', group: 'Live', applies: (t) => t.kind === 'tag' && live(),
    enabled: (t) => t.kind === 'tag' && canForce(t.tag),
    run: (t) => t.kind === 'tag' && forceTag(t.tag, false),
  },
  {
    id: 'tag.copyName', title: 'Copy name', group: 'Tag', applies: (t) => t.kind === 'tag',
    run: (t) => {
      if (t.kind !== 'tag') return
      try {
        void navigator.clipboard?.writeText(t.tag).catch(() => {})
      } catch {
        // no clipboard permission
      }
      s().notify(`Copied ${t.tag}`)
    },
  },
  // ---- routine
  {
    id: 'routine.open', title: 'Open', group: 'Routine', applies: (t) => t.kind === 'routine',
    run: (t) => t.kind === 'routine' && s().setView({ kind: 'routine', program: t.program, routine: t.routine }),
  },
  {
    id: 'routine.rename', title: 'Rename…', shortcut: 'F2', group: 'Routine', applies: (t) => t.kind === 'routine',
    run: (t) => t.kind === 'routine' && s().setFocus({ kind: 'renameRoutine', program: t.program, routine: t.routine }),
  },
  {
    id: 'routine.duplicate', title: 'Duplicate', group: 'Routine', applies: (t) => t.kind === 'routine',
    run: (t) => {
      if (t.kind !== 'routine') return
      duplicateRoutine(t.program, t.routine)
    },
  },
  ...(['st', 'plcopen', 'l5x'] as ExportFormat[]).map(
    (f): Command => ({
      id: `routine.export:${f}`,
      title: { st: 'Structured Text (.st)', plcopen: 'PLCopen XML (.xml)', l5x: 'Rockwell L5X (.L5X)' }[f],
      group: 'Export',
      applies: (t) => t.kind === 'routine',
      run: (t) => {
        if (t.kind !== 'routine') return
        void exportRoutine(f, routineStRef(t.program, t.routine))
      },
    }),
  ),
  {
    id: 'routine.delete', title: 'Delete', group: 'Routine', applies: (t) => t.kind === 'routine',
    enabled: (t) => t.kind === 'routine' && (findProgram(s().project, t.program)?.routines.length ?? 0) > 1,
    run: (t) => t.kind === 'routine' && deleteRoutine(t.program, t.routine),
  },
  // ---- device
  {
    id: 'device.detect', title: 'Detect over USB…', group: 'Device', applies: (t) => t.kind === 'device',
    run: () => s().setDialog({ kind: 'addDevice', pane: 'detect' }),
  },
  {
    id: 'device.change', title: 'Change device…', group: 'Device', applies: (t) => t.kind === 'device',
    run: (t) => {
      if (t.kind !== 'device') return
      s().setView({ kind: 'io', device: t.device })
      requestAnimationFrame(() => document.getElementById('profile')?.focus())
    },
  },
  {
    id: 'device.manifest', title: 'View manifest', group: 'Device', applies: (t) => t.kind === 'device',
    run: (t) => t.kind === 'device' && s().setDialog({ kind: 'manifest', device: t.device }),
  },
  {
    id: 'device.update', title: 'Update manifest…', group: 'Device', applies: (t) => t.kind === 'device',
    enabled: (t) => t.kind === 'device' && deviceHasUpdate(t.device),
    run: (t) => t.kind === 'device' && s().setDialog({ kind: 'updateManifest', device: t.device }),
  },
]

let deviceUpdateCheck: (device: string) => boolean = () => false
/** Set by the devices module (keeps this file free of the catalog). */
export function setDeviceUpdateCheck(f: (device: string) => boolean) {
  deviceUpdateCheck = f
}
function deviceHasUpdate(device: string) {
  return deviceUpdateCheck(device)
}

export function command(id: string): Command | undefined {
  return COMMANDS.find((c) => c.id === id) ?? dynamicCommand(id)
}

function dynamicCommand(id: string): Command | undefined {
  const [base, arg] = id.split(':')
  if (!arg) return undefined
  if (base === 'element.changeType') {
    return {
      id, title: arg, group: 'Change type',
      applies: (t) => changeTypeOptions(elementOf(t) ?? ({ type: 'return', id: 0 } as Element)).includes(arg),
      run: (t) => t.kind === 'element' && changeType(t.rungId, t.elementId, arg),
    }
  }
  if (base === 'element.insertBefore') return insertCommand('before', arg)
  if (base === 'element.insertAfter') return insertCommand('after', arg)
  return undefined
}

export function isEnabled(c: Command, t: Target): boolean {
  return c.enabled ? c.enabled(t) : true
}

/** Runs a command by id on a target; false when it does not apply. */
export function runCommand(id: string, t: Target): boolean {
  const c = command(id)
  if (!c || !c.applies(t) || !isEnabled(c, t)) return false
  c.run(t)
  return true
}

const entry = (id: string, t: Target): MenuEntry[] => {
  const c = command(id)
  return c && c.applies(t) ? [{ command: c }] : []
}

/** The context menu of a target. */
export function menuFor(t: Target): MenuEntry[] {
  const sep: MenuEntry = { separator: true }
  const out: MenuEntry[] = []
  const push = (...ids: string[]) => ids.forEach((id) => out.push(...entry(id, t)))
  if (t.kind === 'element') {
    const e = elementOf(t)
    push('element.edit')
    const swaps = e ? changeTypeOptions(e) : []
    if (swaps.length) out.push({ submenu: 'Change type', items: swaps.map((m) => ({ command: command(`element.changeType:${m}`)! })) })
    out.push({ submenu: 'Insert before', items: INSERTABLE.map((m) => ({ command: insertCommand('before', m) })) })
    out.push({ submenu: 'Insert after', items: INSERTABLE.map((m) => ({ command: insertCommand('after', m) })) })
    push('element.branch')
    out.push(sep)
    push('element.cut', 'element.copy', 'element.paste', 'element.duplicate', 'element.delete')
    out.push(sep)
    push('element.goToTag', 'element.showSt')
    if (live() && boolTag(t)) {
      out.push(sep)
      push('element.toggle', 'element.forceOn', 'element.forceOff')
    }
  } else if (t.kind === 'rung') {
    push('rung.insertAbove', 'rung.insertBelow', 'rung.duplicate')
    out.push(sep)
    push('rung.moveUp', 'rung.moveDown', 'rung.comment')
    out.push(sep)
    push('rung.copy', 'rung.paste', 'element.showSt')
    out.push(sep)
    push('rung.delete')
  } else if (t.kind === 'tag') {
    push('tag.usages', 'tag.rename', 'tag.map', 'tag.copyName')
    if (live()) {
      out.push(sep)
      push('tag.forceOn', 'tag.forceOff')
    }
  } else if (t.kind === 'routine') {
    push('routine.open', 'routine.rename', 'routine.duplicate')
    out.push({ submenu: 'Export', items: ['st', 'plcopen', 'l5x'].flatMap((f) => entry(`routine.export:${f}`, t)) })
    out.push(sep)
    push('routine.delete')
  } else if (t.kind === 'device') {
    push('device.detect', 'device.change', 'device.manifest', 'device.update')
  }
  // No leading, trailing or doubled separators.
  return out.filter((x, i, a) => !('separator' in x) || (i > 0 && i < a.length - 1 && !('separator' in a[i - 1])))
}

/** `Shift+C`, `Mod+c`, `Delete`, `c` for a keyboard event. */
export function keyName(e: { key: string; ctrlKey: boolean; metaKey: boolean; shiftKey: boolean; altKey: boolean }): string {
  const mod = e.ctrlKey || e.metaKey
  let k = e.key
  if (mod) k = `Mod+${e.shiftKey && k.length === 1 ? 'Shift+' : ''}${k.length === 1 ? (e.shiftKey ? k.toUpperCase() : k.toLowerCase()) : k}`
  else if (e.shiftKey && k.length === 1 && k !== ' ') k = `Shift+${k.toUpperCase()}`
  return k
}

/** The command a key runs on a target in the rung list, if any. */
export function commandForKey(key: string, t: Target): Command | undefined {
  return COMMANDS.find((c) => c.keys?.includes(key) && c.applies(t) && isEnabled(c, t))
}

// ---------------------------------------------------------------- routine helpers

export function duplicateRoutine(program: string, routine: string) {
  const st = s()
  const prog = findProgram(st.project, program)
  const src = prog?.routines.find((r) => r.name === routine)
  if (!prog || !src) return
  let n = 2
  while (prog.routines.some((r) => r.name.toLowerCase() === `${routine}_${n}`.toLowerCase())) n++
  const name = `${routine}_${n}`
  const copy: Routine = { id: newId(), name, rungs: src.rungs.map((g) => ({ ...g, id: newId(), elements: withNewIds(g.elements) })) }
  st.commit((p) => ({ ...p, pous: p.pous.map((pr) => (pr.name === program ? { ...pr, routines: [...pr.routines, copy] } : pr)) }))
  st.setView({ kind: 'routine', program, routine: name })
}

export function deleteRoutine(program: string, routine: string) {
  const st = s()
  const prog = findProgram(st.project, program)
  if (!prog || prog.routines.length <= 1) return st.notify('A program needs at least one routine', 'alarm')
  st.commit((p) => ({ ...p, pous: p.pous.map((pr) => (pr.name === program ? { ...pr, routines: pr.routines.filter((r) => r.name !== routine) } : pr)) }))
  const next = findProgram(useEditor.getState().project, program)?.routines[0]
  if (next) st.setView({ kind: 'routine', program, routine: next.name })
}

export function renameRoutine(program: string, from: string, to: string): string | undefined {
  const st = s()
  const n = to.trim()
  if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(n)) return 'Use letters, digits and _'
  if (n === from) return undefined
  if (st.project.pous.some((p) => p.routines.some((r) => r.name.toLowerCase() === n.toLowerCase()))) return 'A routine with that name exists'
  st.commit((p) => {
    const renamed = mapRoutine(p, program, from, (r) => ({ ...r, name: n }))
    // JSR operands naming it follow.
    return {
      ...renamed,
      pous: renamed.pous.map((pr) =>
        pr.name !== program
          ? pr
          : {
              ...pr,
              routines: pr.routines.map((r) => ({
                ...r,
                rungs: r.rungs.map((g) => ({
                  ...g,
                  elements: renameJsr(g.elements, from, n),
                })),
              })),
            },
      ),
    }
  })
  const v = st.view
  if (v.kind === 'routine' && v.program === program && v.routine === from) st.setView({ kind: 'routine', program, routine: n })
  return undefined
}

function renameJsr(items: Element[], from: string, to: string): Element[] {
  return items.map((e) => {
    if (e.type === 'branch') return { ...e, legs: e.legs.map((l) => renameJsr(l, from, to)) }
    if (e.type === 'block' && e.name.toUpperCase() === 'JSR' && e.pins[0]?.value?.trim().toLowerCase() === from.toLowerCase()) {
      return { ...e, pins: e.pins.map((p, i) => (i === 0 ? { ...p, value: to } : p)) }
    }
    return e
  })
}

export { findTag }
