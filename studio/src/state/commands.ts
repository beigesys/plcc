// SPDX-License-Identifier: MPL-2.0
//
// Editing commands shared by the keyboard, command palette, toolbar and drag
// and drop. Each one reads the editor state, commits an edit, and moves the
// selection.

import {
  BOX_SPECS, COIL_BY_MNEMONIC, CONTACT_BY_MNEMONIC, flatten, insertAfter, isBoxInstr, locate, moveElement, newId,
  parseQuickEntry, parseRung, printRung, reconcileIds, removeElement, updateElement, withNewIds, wrapInBranch,
  type Instruction, type Rung, type Series,
} from '@/model'
import { currentRoutine, editRung, updateRungs, useEditor } from './editor'

export function makeElement(mnemonic: string): Instruction | undefined {
  const m = mnemonic.toUpperCase()
  if (m in CONTACT_BY_MNEMONIC) return { type: 'contact', id: newId('c'), kind: CONTACT_BY_MNEMONIC[m], tag: '' }
  if (m in COIL_BY_MNEMONIC) return { type: 'coil', id: newId('o'), kind: COIL_BY_MNEMONIC[m], tag: '' }
  if (m === 'ST') return { type: 'st', id: newId('s'), code: '' }
  if (isBoxInstr(m)) {
    const operands: Record<string, string> = {}
    for (const o of BOX_SPECS[m].operands) operands[o.key] = o.default === '?' ? '' : o.default
    return { type: 'box', id: newId('b'), instr: m, operands }
  }
  return undefined
}

function state() {
  return useEditor.getState()
}

function rungs(): Rung[] {
  return currentRoutine(state())?.rungs ?? []
}

function newRung(body: Series = { type: 'series', items: [] }): Rung {
  return { id: newId('r'), comment: '', body }
}

/** Adds a rung after the selected one (or at the end) and selects it. */
export function addRung(body?: Series, comment = ''): string | undefined {
  const s = state()
  if (s.view.kind !== 'routine') return undefined
  const r = { ...newRung(body), comment }
  const after = s.selection.rungId
  updateRungs((list) => {
    const i = list.findIndex((g) => g.id === after)
    const next = [...list]
    next.splice(i < 0 ? list.length : i + 1, 0, r)
    return next
  })
  s.select({ rungId: r.id, elementId: null })
  return r.id
}

/** Inserts an instruction after the selection (or into the selected rung, or a new rung). */
export function insertInstruction(mnemonic: string, opts: { rungId?: string; afterId?: string | null; edit?: boolean } = {}) {
  const el = makeElement(mnemonic)
  if (!el) return
  const s = state()
  if (s.view.kind !== 'routine') return
  let rungId: string | null | undefined = opts.rungId ?? s.selection.rungId
  const afterId = opts.afterId !== undefined ? opts.afterId : opts.rungId ? null : s.selection.elementId
  if (!rungId || !rungs().some((g) => g.id === rungId)) {
    rungId = addRung({ type: 'series', items: [el] })
    if (!rungId) return
  } else {
    // A parallel as anchor means "after the branch", which insertAfter does.
    editRung(rungId, (b) => insertAfter(b, afterId ?? null, el))
  }
  state().select({ rungId, elementId: el.id })
  if (opts.edit !== false && el.type !== 'parallel') state().setEditing({ rungId, elementId: el.id })
}

export function deleteSelection() {
  const s = state()
  const { rungId, elementId } = s.selection
  if (!rungId) return
  if (elementId) {
    const rung = rungs().find((g) => g.id === rungId)
    if (!rung) return
    const els = flatten(rung.body)
    const i = els.findIndex((e) => e.id === elementId)
    editRung(rungId, (b) => removeElement(b, elementId))
    const after = flatten(rungs().find((g) => g.id === rungId)?.body ?? rung.body)
    const next = after[Math.min(i, after.length - 1)] ?? after[i - 1]
    s.select({ rungId, elementId: next?.id ?? null })
    return
  }
  deleteRung(rungId)
}

export function deleteRung(rungId: string) {
  const list = rungs()
  const i = list.findIndex((g) => g.id === rungId)
  updateRungs((l) => l.filter((g) => g.id !== rungId))
  const next = list[i + 1] ?? list[i - 1]
  state().select({ rungId: next?.id ?? null, elementId: null })
}

export function wrapSelectionInBranch() {
  const { rungId, elementId } = state().selection
  if (!rungId || !elementId) return
  let pid = elementId
  editRung(rungId, (b) => {
    const r = wrapInBranch(b, elementId)
    pid = r.parallelId
    return r.body
  })
  state().select({ rungId, elementId: pid })
}

export function moveSelectedElement(dir: -1 | 1) {
  const { rungId, elementId } = state().selection
  if (!rungId || !elementId) return
  editRung(rungId, (b) => moveElement(b, elementId, dir))
}

export function moveRung(rungId: string, dir: -1 | 1) {
  updateRungs((l) => {
    const i = l.findIndex((g) => g.id === rungId)
    const j = i + dir
    if (i < 0 || j < 0 || j >= l.length) return l
    const next = [...l]
    ;[next[i], next[j]] = [next[j], next[i]]
    return next
  })
}

export function duplicateRung(rungId: string) {
  const r = rungs().find((g) => g.id === rungId)
  if (!r) return
  const copy: Rung = { id: newId('r'), comment: r.comment, body: withNewIds(r.body) }
  updateRungs((l) => {
    const i = l.findIndex((g) => g.id === rungId)
    const next = [...l]
    next.splice(i + 1, 0, copy)
    return next
  })
  state().select({ rungId: copy.id, elementId: null })
}

export function setRungComment(rungId: string, comment: string) {
  updateRungs((l) => l.map((g) => (g.id === rungId ? { ...g, comment } : g)))
}

/** Replaces a rung's body from rung text. Throws RungTextError. */
export function setRungText(rungId: string, text: string) {
  const r = rungs().find((g) => g.id === rungId)
  if (!r) return
  const parsed = parseRung(text)
  if (printRung(parsed) === printRung(r.body)) return
  editRung(rungId, (old) => reconcileIds(old, parsed))
}

/** Creates a rung from `XIC Start XIO Stop OTE Motor` or rung text. Throws RungTextError. */
export function addRungFromQuickEntry(text: string): string | undefined {
  const body = parseQuickEntry(text)
  return addRung(body)
}

export function updateInstruction(rungId: string, id: string, fn: (e: Instruction) => Instruction) {
  editRung(rungId, (b) => updateElement(b, id, fn))
}

// ---------------------------------------------------------------- navigation

export function moveSelection(dir: 'left' | 'right' | 'up' | 'down') {
  const s = state()
  const list = rungs()
  if (!list.length) return
  const ri = list.findIndex((g) => g.id === s.selection.rungId)
  if (ri < 0) {
    const first = list[0]
    s.select({ rungId: first.id, elementId: flatten(first.body)[0]?.id ?? null })
    return
  }
  const rung = list[ri]
  const els = flatten(rung.body)
  const ei = els.findIndex((e) => e.id === s.selection.elementId)

  if (dir === 'left' || dir === 'right') {
    if (ei < 0) {
      s.select({ rungId: rung.id, elementId: (dir === 'right' ? els[0] : els[els.length - 1])?.id ?? null })
      return
    }
    const next = els[ei + (dir === 'right' ? 1 : -1)]
    if (next) s.select({ rungId: rung.id, elementId: next.id })
    return
  }

  // Up/down: to the neighbouring branch of the enclosing parallel, else the neighbouring rung.
  const id = s.selection.elementId
  if (id) {
    const loc = locate(rung.body, id)
    if (loc?.owner && loc.branch !== undefined) {
      const b = loc.branch + (dir === 'down' ? 1 : -1)
      const target = loc.owner.branches[b]
      if (target) {
        const pick = target.items[Math.min(loc.index, target.items.length - 1)]
        if (pick && pick.type !== 'series') {
          s.select({ rungId: rung.id, elementId: pick.id })
          return
        }
      }
    }
  }
  const nr = list[ri + (dir === 'down' ? 1 : -1)]
  if (!nr) return
  const nels = flatten(nr.body)
  s.select({ rungId: nr.id, elementId: nels[Math.min(Math.max(ei, 0), nels.length - 1)]?.id ?? null })
}
