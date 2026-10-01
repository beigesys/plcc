// SPDX-License-Identifier: MPL-2.0
//
// Editing commands shared by the keyboard, command palette, context menus,
// toolbar and drag and drop (registry.ts names them). Each one reads the
// editor state, commits an edit, and moves the selection.

import {
  boxSpec, findElement, flatten, insertAfter, insertBefore, locate, moveElement, newId, newRung, parseQuickEntry,
  parseRung, parseRungs, printRung, printSeries, reconcileIds, removeElement, specOf, updateElement, withNewIds,
  wrapInBranch, type Element, type Id, type Rung,
} from '@/model'
import { currentRoutine, editRung, updateRungs, useEditor } from './editor'

export function makeElement(mnemonic: string): Element | undefined {
  const m = mnemonic.toUpperCase()
  if (m === 'XIC' || m === 'XIO') return { type: 'contact', id: newId(), kind: m === 'XIC' ? 'no' : 'nc', operand: '?' }
  if (m === 'OTE' || m === 'OTL' || m === 'OTU') {
    return { type: 'coil', id: newId(), kind: m === 'OTE' ? 'normal' : m === 'OTL' ? 'set' : 'reset', operand: '?' }
  }
  if (m === 'ST') return { type: 'st', id: newId(), code: '' }
  if (m === 'JMP') return { type: 'jump', id: newId(), label: '?' }
  if (m === 'RET') return { type: 'return', id: newId() }
  const spec = specOf(m)
  if (!spec) return undefined
  return {
    type: 'block',
    id: newId(),
    name: m,
    pins: spec.pins.map((p) => ({ name: p.name, dir: 'input' as const, value: p.default })),
  }
}

function state() {
  return useEditor.getState()
}

function rungs(): Rung[] {
  return currentRoutine(state())?.rungs ?? []
}

function rungOf(id: Id | null | undefined): Rung | undefined {
  return id == null ? undefined : rungs().find((g) => g.id === id)
}

/** Adds a rung after `after` (default: the selected rung; null: at the end; 'before:<id>' above it) and selects it. */
export function addRung(elements?: Element[], opts: { comment?: string; label?: string; after?: Id | null; before?: Id } = {}): Id | undefined {
  const s = state()
  if (s.view.kind !== 'routine') return undefined
  const r: Rung = { ...newRung(elements ?? [], opts.comment), ...(opts.label ? { label: opts.label } : {}) }
  const after = opts.after !== undefined ? opts.after : s.selection.rungId
  updateRungs((list) => {
    const next = [...list]
    if (opts.before !== undefined) {
      const i = list.findIndex((g) => g.id === opts.before)
      next.splice(i < 0 ? 0 : i, 0, r)
      return next
    }
    const i = list.findIndex((g) => g.id === after)
    next.splice(i < 0 ? list.length : i + 1, 0, r)
    return next
  })
  s.select({ rungId: r.id, elementId: null })
  return r.id
}

/** Inserts an instruction after (or before) an element, into the selected rung, or into a new rung. */
export function insertInstruction(
  mnemonic: string,
  opts: { rungId?: Id; afterId?: Id | null; beforeId?: Id; edit?: boolean } = {},
) {
  const el = makeElement(mnemonic)
  if (!el) return
  const s = state()
  if (s.view.kind !== 'routine') return
  let rungId: Id | null | undefined = opts.rungId ?? s.selection.rungId
  const afterId = opts.afterId !== undefined ? opts.afterId : opts.rungId != null ? null : s.selection.elementId
  if (rungId == null || !rungOf(rungId)) {
    rungId = addRung([el])
    if (rungId == null) return
  } else if (opts.beforeId !== undefined) {
    const before = opts.beforeId
    editRung(rungId, (b) => insertBefore(b, before, el))
  } else {
    // A branch as anchor means "after the branch", which insertAfter does.
    editRung(rungId, (b) => insertAfter(b, afterId ?? null, el))
  }
  state().select({ rungId, elementId: el.id })
  if (opts.edit !== false && el.type !== 'branch' && el.type !== 'return') state().setEditing({ rungId, elementId: el.id })
}

export function deleteElement(rungId: Id, elementId: Id) {
  const s = state()
  const rung = rungOf(rungId)
  if (!rung) return
  const els = flatten(rung.elements)
  const i = els.findIndex((e) => e.id === elementId)
  editRung(rungId, (b) => removeElement(b, elementId))
  const after = flatten(rungOf(rungId)?.elements ?? rung.elements)
  const next = after[Math.min(i, after.length - 1)] ?? after[i - 1]
  s.select({ rungId, elementId: next?.id ?? null })
}

export function deleteSelection() {
  const { rungId, elementId } = state().selection
  if (rungId == null) return
  if (elementId != null) return deleteElement(rungId, elementId)
  deleteRung(rungId)
}

export function deleteRung(rungId: Id) {
  const list = rungs()
  const i = list.findIndex((g) => g.id === rungId)
  updateRungs((l) => l.filter((g) => g.id !== rungId))
  const next = list[i + 1] ?? list[i - 1]
  state().select({ rungId: next?.id ?? null, elementId: null })
}

export function wrapInBranchAt(rungId: Id, elementId: Id) {
  let bid = elementId
  editRung(rungId, (b) => {
    const r = wrapInBranch(b, elementId)
    bid = r.branchId
    return r.elements
  })
  state().select({ rungId, elementId: bid })
}

export function wrapSelectionInBranch() {
  const { rungId, elementId } = state().selection
  if (rungId == null || elementId == null) return
  wrapInBranchAt(rungId, elementId)
}

export function moveSelectedElement(dir: -1 | 1) {
  const { rungId, elementId } = state().selection
  if (rungId == null || elementId == null) return
  editRung(rungId, (b) => moveElement(b, elementId, dir))
}

export function moveRung(rungId: Id, dir: -1 | 1) {
  updateRungs((l) => {
    const i = l.findIndex((g) => g.id === rungId)
    const j = i + dir
    if (i < 0 || j < 0 || j >= l.length) return l
    const next = [...l]
    ;[next[i], next[j]] = [next[j], next[i]]
    return next
  })
}

export function duplicateRung(rungId: Id) {
  const r = rungOf(rungId)
  if (!r) return
  const copy: Rung = { ...newRung(withNewIds(r.elements), r.comment) }
  updateRungs((l) => {
    const i = l.findIndex((g) => g.id === rungId)
    const next = [...l]
    next.splice(i + 1, 0, copy)
    return next
  })
  state().select({ rungId: copy.id, elementId: null })
}

export function setRungComment(rungId: Id, comment: string) {
  updateRungs((l) =>
    l.map((g) => {
      if (g.id !== rungId) return g
      const { comment: _old, ...rest } = g
      void _old
      return comment ? { ...rest, comment } : rest
    }),
  )
}

/** Replaces a rung's elements from rung text. Throws RungTextError. */
export function setRungText(rungId: Id, text: string) {
  const r = rungOf(rungId)
  if (!r) return
  const parsed = parseRung(text)
  if (printRung(parsed) === printRung(r)) return
  updateRungs((l) =>
    l.map((g) => {
      if (g.id !== rungId) return g
      const { label: _l, ...rest } = g
      void _l
      const elements = reconcileIds(g.elements, parsed.elements)
      return parsed.label ? { ...rest, label: parsed.label, elements } : { ...rest, elements }
    }),
  )
}

/** Creates a rung from `XIC Start XIO Stop OTE Motor` or rung text. Throws RungTextError. */
export function addRungFromQuickEntry(text: string): Id | undefined {
  const r = parseQuickEntry(text)
  return addRung(r.elements, { label: r.label })
}

export function updateInstruction(rungId: Id, id: Id, fn: (e: Element) => Element) {
  editRung(rungId, (b) => updateElement(b, id, fn))
}

// ---------------------------------------------------------------- change type

/** Instructions an element can be switched to in place (same operands). */
export const SWAPS: string[][] = [
  ['XIC', 'XIO'],
  ['OTE', 'OTL', 'OTU'],
  ['TON', 'TOF', 'RTO'],
  ['CTU', 'CTD'],
  ['EQU', 'NEQ', 'GRT', 'GEQ', 'LES', 'LEQ'],
  ['ADD', 'SUB', 'MUL', 'DIV'],
  ['OSR', 'OSF'],
]

export function mnemonicOf(e: Element): string | undefined {
  switch (e.type) {
    case 'contact':
      return e.kind === 'nc' ? 'XIO' : e.kind === 'no' ? 'XIC' : undefined
    case 'coil':
      return e.kind === 'set' ? 'OTL' : e.kind === 'reset' ? 'OTU' : e.kind === 'normal' ? 'OTE' : undefined
    case 'block':
      return e.name.toUpperCase()
    default:
      return undefined
  }
}

/** The mnemonics `e` can become in place. */
export function changeTypeOptions(e: Element): string[] {
  const m = mnemonicOf(e)
  if (!m) return []
  return SWAPS.find((g) => g.includes(m))?.filter((x) => x !== m) ?? []
}

/** `e` as `mnemonic`, keeping its id and operands. */
export function changedType(e: Element, mnemonic: string): Element {
  const m = mnemonic.toUpperCase()
  if (e.type === 'contact' && (m === 'XIC' || m === 'XIO')) return { ...e, kind: m === 'XIC' ? 'no' : 'nc' }
  if (e.type === 'coil' && (m === 'OTE' || m === 'OTL' || m === 'OTU')) {
    return { ...e, kind: m === 'OTE' ? 'normal' : m === 'OTL' ? 'set' : 'reset' }
  }
  if (e.type === 'block') {
    const spec = boxSpec({ name: m, pins: e.pins })
    return { ...e, name: m, pins: e.pins.map((p, i) => ({ ...p, name: spec.pins[i]?.name ?? p.name })) }
  }
  return e
}

export function changeType(rungId: Id, elementId: Id, mnemonic: string) {
  updateInstruction(rungId, elementId, (e) => changedType(e, mnemonic))
  state().select({ rungId, elementId })
}

// ---------------------------------------------------------------- clipboard

/** Copied ladder, as rung text (also put on the system clipboard). */
let clip: { kind: 'elements' | 'rungs'; text: string } | null = null

function toClipboard(text: string) {
  try {
    void navigator.clipboard?.writeText(text).catch(() => {})
  } catch {
    // no clipboard permission: the in-page clipboard still works
  }
}

export function copyElement(rungId: Id, elementId: Id) {
  const el = findElement(rungOf(rungId)?.elements ?? [], elementId)
  if (!el) return
  clip = { kind: 'elements', text: printSeries([el]) }
  toClipboard(clip.text)
  state().notify(`Copied ${clip.text}`)
}

export function cutElement(rungId: Id, elementId: Id) {
  copyElement(rungId, elementId)
  deleteElement(rungId, elementId)
}

export function copyRungs(ids: Id[]) {
  const list = rungs().filter((g) => ids.includes(g.id))
  if (!list.length) return
  clip = { kind: 'rungs', text: list.map((g) => printRung(g)).join('\n') }
  toClipboard(clip.text)
  state().notify(`Copied ${list.length} rung${list.length > 1 ? 's' : ''}`)
}

export function hasClipboard(): boolean {
  return clip !== null
}

/** Pastes after the element (elements) or after the rung (rungs). Throws RungTextError on bad text. */
export function paste(rungId: Id | null, elementId: Id | null, text = clip?.text) {
  if (!text) return
  const s = state()
  const parsed = parseRungs(text)
  if (clip?.kind === 'elements' && rungId != null && parsed.length === 1) {
    const els = withNewIds(parsed[0].elements)
    editRung(rungId, (b) => {
      let out = b
      let anchor = elementId
      for (const e of els) {
        out = insertAfter(out, anchor, e)
        anchor = e.id
      }
      return out
    })
    s.select({ rungId, elementId: els[els.length - 1]?.id ?? null })
    return
  }
  let after = rungId
  for (const r of parsed) after = addRung(withNewIds(r.elements), { label: r.label, after: after ?? null }) ?? after
}

export function duplicateElement(rungId: Id, elementId: Id) {
  const el = findElement(rungOf(rungId)?.elements ?? [], elementId)
  if (!el) return
  const [copy] = withNewIds([el])
  editRung(rungId, (b) => insertAfter(b, elementId, copy))
  state().select({ rungId, elementId: copy.id })
}

// ---------------------------------------------------------------- navigation

export function moveSelection(dir: 'left' | 'right' | 'up' | 'down') {
  const s = state()
  const list = rungs()
  if (!list.length) return
  const ri = list.findIndex((g) => g.id === s.selection.rungId)
  if (ri < 0) {
    const first = list[0]
    s.select({ rungId: first.id, elementId: flatten(first.elements)[0]?.id ?? null })
    return
  }
  const rung = list[ri]
  const els = flatten(rung.elements)
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

  // Up/down: to the neighbouring leg of the enclosing branch, else the neighbouring rung.
  const id = s.selection.elementId
  if (id != null) {
    const loc = locate(rung.elements, id)
    if (loc?.owner && loc.leg !== undefined) {
      const target = loc.owner.legs[loc.leg + (dir === 'down' ? 1 : -1)]
      if (target) {
        const pick = target[Math.min(loc.index, target.length - 1)]
        if (pick) {
          s.select({ rungId: rung.id, elementId: pick.id })
          return
        }
      }
    }
  }
  const nr = list[ri + (dir === 'down' ? 1 : -1)]
  if (!nr) return
  const nels = flatten(nr.elements)
  s.select({ rungId: nr.id, elementId: nels[Math.min(Math.max(ei, 0), nels.length - 1)]?.id ?? null })
}
