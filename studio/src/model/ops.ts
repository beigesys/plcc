// SPDX-License-Identifier: MPL-2.0
//
// Pure operations on rungs (series of elements) and projects. Every function
// returns new values and leaves its input alone, so the editor can keep old
// versions for undo.

import { newId } from './ids'
import { boxSpec, isOutput } from './instructions'
import type { Branch, Element, Id, Pou, Project, Routine, RoutineKind, Rung, Tag } from './types'

/** Every element, depth first, left to right (a branch before its legs). */
export function flatten(items: Element[]): Element[] {
  const out: Element[] = []
  const visit = (s: Element[]) => {
    for (const e of s) {
      out.push(e)
      if (e.type === 'branch') e.legs.forEach(visit)
    }
  }
  visit(items)
  return out
}

export interface Location {
  parent: Element[]
  index: number
  /** Innermost branch containing `parent`, if any, and which leg it is. */
  owner?: Branch
  leg?: number
}

export function locate(items: Element[], id: Id): Location | undefined {
  const visit = (s: Element[], owner?: Branch, leg?: number): Location | undefined => {
    for (let i = 0; i < s.length; i++) {
      const e = s[i]
      if (e.id === id) return { parent: s, index: i, owner, leg }
      if (e.type === 'branch') {
        for (let b = 0; b < e.legs.length; b++) {
          const r = visit(e.legs[b], e, b)
          if (r) return r
        }
      }
    }
    return undefined
  }
  return visit(items)
}

export function findElement(items: Element[], id: Id | null | undefined): Element | undefined {
  if (id == null) return undefined
  return flatten(items).find((e) => e.id === id)
}

function clone<T>(v: T): T {
  return structuredClone(v)
}

/** Collapses branches with one leg and drops branches whose legs are all empty. */
export function normalize(items: Element[]): Element[] {
  const fix = (s: Element[]): Element[] => {
    const out: Element[] = []
    for (const e of s) {
      if (e.type === 'branch') {
        const legs = e.legs.map(fix)
        if (legs.every((l) => l.length === 0)) continue
        if (legs.length === 1) out.push(...legs[0])
        else out.push({ ...e, legs })
      } else out.push(e)
    }
    return out
  }
  return fix(items)
}

/**
 * Inserts `el` after the element `afterId`. With no anchor, inputs go before the
 * trailing outputs and outputs go at the end.
 */
export function insertAfter(items: Element[], afterId: Id | null, el: Element): Element[] {
  const b = clone(items)
  if (afterId != null) {
    const loc = locate(b, afterId)
    if (loc) {
      loc.parent.splice(loc.index + 1, 0, el)
      return b
    }
  }
  if (isOutput(el)) b.push(el)
  else {
    let i = b.length
    while (i > 0 && isOutput(b[i - 1])) i--
    b.splice(i, 0, el)
  }
  return b
}

export function insertBefore(items: Element[], beforeId: Id, el: Element): Element[] {
  const b = clone(items)
  const loc = locate(b, beforeId)
  if (!loc) return insertAfter(b, null, el)
  loc.parent.splice(loc.index, 0, el)
  return b
}

export function removeElement(items: Element[], id: Id): Element[] {
  const b = clone(items)
  const loc = locate(b, id)
  if (!loc) return items
  loc.parent.splice(loc.index, 1)
  // A leg emptied by a delete is dropped; an empty leg would be a short.
  if (loc.parent.length === 0 && loc.owner && loc.leg !== undefined) loc.owner.legs.splice(loc.leg, 1)
  return normalize(b)
}

export function replaceElement(items: Element[], id: Id, el: Element): Element[] {
  const b = clone(items)
  const loc = locate(b, id)
  if (!loc) return items
  loc.parent[loc.index] = el
  return normalize(b)
}

export function updateElement(items: Element[], id: Id, patch: (e: Element) => Element): Element[] {
  const el = findElement(items, id)
  if (!el) return items
  return replaceElement(items, id, patch(clone(el)))
}

/**
 * Puts `id` in a branch with a new empty leg below it; if `id` is already a
 * branch, adds an empty leg to it. Returns the branch's id.
 */
export function wrapInBranch(items: Element[], id: Id): { elements: Element[]; branchId: Id } {
  const el = findElement(items, id)
  if (!el) return { elements: items, branchId: id }
  if (el.type === 'branch') {
    return { elements: updateElement(items, id, (b) => ({ ...(b as Branch), legs: [...(b as Branch).legs, []] })), branchId: id }
  }
  const b = clone(items)
  const loc = locate(b, id)
  if (!loc) return { elements: items, branchId: id }
  const branch: Branch = { type: 'branch', id: newId(), legs: [[loc.parent[loc.index]], []] }
  loc.parent[loc.index] = branch
  return { elements: b, branchId: branch.id }
}

/** Appends `el` to leg `leg` of branch `branchId`. */
export function appendToBranch(items: Element[], branchId: Id, leg: number, el: Element): Element[] {
  return updateElement(items, branchId, (b) => {
    const br = b as Branch
    return { ...br, legs: br.legs.map((l, i) => (i === leg ? [...l, el] : l)) }
  })
}

/** Moves an element one step left (-1) or right (+1) within its series. */
export function moveElement(items: Element[], id: Id, dir: -1 | 1): Element[] {
  const b = clone(items)
  const loc = locate(b, id)
  if (!loc) return items
  const j = loc.index + dir
  if (j < 0 || j >= loc.parent.length) return items
  ;[loc.parent[loc.index], loc.parent[j]] = [loc.parent[j], loc.parent[loc.index]]
  return b
}

/** Gives parsed elements the ids of the old ones where the shape matches (same DFS slot, same kind). */
export function reconcileIds(oldItems: Element[], newItems: Element[]): Element[] {
  const olds = flatten(oldItems)
  const b = clone(newItems)
  const used = new Set<Id>()
  flatten(b).forEach((e, i) => {
    const o = olds[i]
    if (o && o.type === e.type && !used.has(o.id)) {
      e.id = o.id
      used.add(o.id)
    }
  })
  return b
}

/** Deep copy with fresh ids (copy/paste, duplicate rung). */
export function withNewIds(items: Element[]): Element[] {
  const b = clone(items)
  for (const e of flatten(b)) e.id = newId()
  return b
}

// ---------------------------------------------------------------- operands

/** The operand texts of an element, in pin order. */
export function operandsOf(e: Element): string[] {
  switch (e.type) {
    case 'contact':
    case 'coil':
      return [e.operand]
    case 'block':
      return e.pins.map((p) => p.value ?? '')
    case 'jump':
      return [e.label]
    default:
      return []
  }
}

/** Whether an operand is still to be filled in. */
export function isUnassigned(text: string | undefined): boolean {
  return text === undefined || text.trim() === '' || text.trim() === '?'
}

// ---------------------------------------------------------------- routines

/** A routine that is one ST box is an ST routine (as plcc writes it to L5X). */
export function isStRoutine(r: Routine): boolean {
  return r.rungs.length === 1 && !r.rungs[0].label && r.rungs[0].elements.length === 1 && r.rungs[0].elements[0].type === 'st'
}

export function routineKind(r: Routine): RoutineKind {
  return isStRoutine(r) ? 'st' : 'ladder'
}

export function stRoutineCode(r: Routine): string {
  const e = r.rungs[0]?.elements[0]
  return e?.type === 'st' ? e.code : ''
}

export function withStRoutineCode(r: Routine, code: string): Routine {
  const rung = r.rungs[0]
  const e = rung?.elements[0]
  if (!rung || e?.type !== 'st') return r
  return { ...r, rungs: [{ ...rung, elements: [{ ...e, code }] }] }
}

export function newRoutine(name: string, kind: RoutineKind): Routine {
  if (kind === 'st') return { id: newId(), name, rungs: [{ id: newId(), elements: [{ type: 'st', id: newId(), code: '' }] }] }
  return { id: newId(), name, rungs: [] }
}

export function newRung(elements: Element[] = [], comment?: string): Rung {
  return comment ? { id: newId(), comment, elements } : { id: newId(), elements }
}

/** The main routine (run every scan): the first. */
export function mainRoutine(p: Pou): Routine | undefined {
  return p.routines[0]
}

// ---------------------------------------------------------------- tags

const TAG_REF = /^[A-Za-z_][A-Za-z0-9_]*(\.[A-Za-z0-9_]+)*$/

/** The base tag name an operand refers to, or undefined for literals and expressions. */
export function baseTag(operand: string): string | undefined {
  const t = operand.trim()
  if (!TAG_REF.test(t)) return undefined
  if (/^(TRUE|FALSE)$/i.test(t)) return undefined
  return t.split('.')[0]
}

export interface TagUse {
  name: string
  /** Type to create the tag with if it does not exist. */
  type: string
}

/** Tags the elements name that the project should have, with inferred types. */
export function tagUses(items: Element[]): TagUse[] {
  const out: TagUse[] = []
  for (const e of flatten(items)) {
    if (e.type === 'contact' || e.type === 'coil') {
      const name = baseTag(e.operand)
      // A member (`Timer.DN`) belongs to its structure tag; only plain names create BOOLs.
      if (name && name === e.operand.trim()) out.push({ name, type: 'BOOL' })
    } else if (e.type === 'block') {
      const spec = boxSpec(e)
      e.pins.forEach((p, i) => {
        const type = spec.pins[i]?.newTagType
        if (!type) return
        const text = p.value ?? ''
        const name = baseTag(text)
        if (name && name === text.trim()) out.push({ name, type })
      })
    }
  }
  return out
}

export function newTag(name: string, type: string, extra: Partial<Tag> = {}): Tag {
  return { name, data_type: type, section: 'global', ...extra }
}

/** Adds any tag the elements use that the project lacks. Returns the tags that were created. */
export function createMissingTags(tags: Tag[], items: Element[], known: string[] = []): { tags: Tag[]; created: Tag[] } {
  const have = new Set([...tags.map((t) => t.name.toLowerCase()), ...known.map((k) => k.toLowerCase())])
  const created: Tag[] = []
  for (const u of tagUses(items)) {
    if (have.has(u.name.toLowerCase())) continue
    have.add(u.name.toLowerCase())
    created.push(newTag(u.name, u.type))
  }
  return { tags: created.length ? [...tags, ...created] : tags, created }
}

function mapOperands(e: Element, fix: (s: string) => string): Element {
  switch (e.type) {
    case 'contact':
    case 'coil':
      return { ...e, operand: fix(e.operand) }
    case 'block':
      return { ...e, pins: e.pins.map((p) => (p.value === undefined ? p : { ...p, value: fix(p.value) })) }
    case 'branch':
      return { ...e, legs: e.legs.map((l) => l.map((x) => mapOperands(x, fix))) }
    default:
      return e
  }
}

/** Renames a tag and every reference to it (`Old`, `Old.DN`) in every ladder routine. */
export function renameTag(project: Project, from: string, to: string): Project {
  const f = from.toLowerCase()
  const fix = (text: string) => {
    const base = baseTag(text)
    if (!base || base.toLowerCase() !== f) return text
    return to + text.trim().slice(base.length)
  }
  return {
    ...project,
    globals: project.globals.map((t) => (t.name.toLowerCase() === f ? { ...t, name: to } : t)),
    pous: project.pous.map((p) => ({
      ...p,
      routines: p.routines.map((r) => ({
        ...r,
        rungs: r.rungs.map((g) => ({ ...g, elements: g.elements.map((e) => mapOperands(e, fix)) })),
      })),
    })),
  }
}

/** Every element that references a tag, with where it is. */
export function tagReferences(project: Project, name: string): { program: string; routine: string; rung: Id; element: Id }[] {
  const n = name.toLowerCase()
  const out: { program: string; routine: string; rung: Id; element: Id }[] = []
  for (const p of project.pous)
    for (const r of p.routines)
      for (const g of r.rungs)
        for (const e of flatten(g.elements)) {
          if (operandsOf(e).some((o) => baseTag(o)?.toLowerCase() === n)) {
            out.push({ program: p.name, routine: r.name, rung: g.id, element: e.id })
          }
        }
  return out
}

/** How many instructions reference a tag. */
export function tagReferenceCount(project: Project, name: string): number {
  return tagReferences(project, name).length
}

export function findTag(project: Project, name: string): Tag | undefined {
  const n = name.toLowerCase()
  return project.globals.find((t) => t.name.toLowerCase() === n)
}
