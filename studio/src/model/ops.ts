// SPDX-License-Identifier: MPL-2.0
//
// Pure operations on rung bodies. Every function returns a new tree and leaves
// its input alone, so the editor can keep old versions for undo.

import { newId } from './ids'
import { BOX_SPECS } from './instructions'
import type { Element, Instruction, Parallel, Project, Series, Tag } from './types'

export function isOutput(e: Element): boolean {
  if (e.type === 'coil') return true
  if (e.type === 'box') return BOX_SPECS[e.instr].role === 'output'
  return false
}

/** Every instruction in depth-first, left-to-right order (a parallel comes before its contents). */
export function flatten(body: Series): Instruction[] {
  const out: Instruction[] = []
  const visit = (s: Series) => {
    for (const e of s.items) {
      if (e.type === 'series') visit(e)
      else {
        out.push(e)
        if (e.type === 'parallel') e.branches.forEach(visit)
      }
    }
  }
  visit(body)
  return out
}

export interface Location {
  parent: Series
  index: number
  /** Innermost parallel containing `parent`, if any, and which branch it is. */
  owner?: Parallel
  branch?: number
}

export function locate(body: Series, id: string): Location | undefined {
  const visit = (s: Series, owner?: Parallel, branch?: number): Location | undefined => {
    for (let i = 0; i < s.items.length; i++) {
      const e = s.items[i]
      if (e.type !== 'series' && e.id === id) return { parent: s, index: i, owner, branch }
      if (e.type === 'series') {
        const r = visit(e, owner, branch)
        if (r) return r
      } else if (e.type === 'parallel') {
        for (let b = 0; b < e.branches.length; b++) {
          const r = visit(e.branches[b], e, b)
          if (r) return r
        }
      }
    }
    return undefined
  }
  return visit(body)
}

export function findElement(body: Series, id: string): Instruction | undefined {
  return flatten(body).find((e) => e.id === id)
}

function clone<T>(v: T): T {
  return structuredClone(v)
}

/** Collapses parallels with a single branch and drops parallels whose branches are all empty. */
export function normalize(body: Series): Series {
  const fix = (s: Series): Series => {
    const items: Element[] = []
    for (const e of s.items) {
      if (e.type === 'series') {
        items.push(...fix(e).items)
      } else if (e.type === 'parallel') {
        const branches = e.branches.map(fix)
        if (branches.every((b) => b.items.length === 0)) continue
        if (branches.length === 1) items.push(...branches[0].items)
        else items.push({ ...e, branches })
      } else items.push(e)
    }
    return { type: 'series', items }
  }
  return fix(body)
}

/**
 * Inserts `el` after the element `afterId`. With no anchor, inputs go before the
 * trailing outputs and outputs go at the end.
 */
export function insertAfter(body: Series, afterId: string | null, el: Instruction): Series {
  const b = clone(body)
  if (afterId) {
    const loc = locate(b, afterId)
    if (loc) {
      loc.parent.items.splice(loc.index + 1, 0, el)
      return b
    }
  }
  if (isOutput(el)) b.items.push(el)
  else {
    let i = b.items.length
    while (i > 0 && isOutput(b.items[i - 1])) i--
    b.items.splice(i, 0, el)
  }
  return b
}

export function insertBefore(body: Series, beforeId: string, el: Instruction): Series {
  const b = clone(body)
  const loc = locate(b, beforeId)
  if (!loc) return insertAfter(b, null, el)
  loc.parent.items.splice(loc.index, 0, el)
  return b
}

export function removeElement(body: Series, id: string): Series {
  const b = clone(body)
  const loc = locate(b, id)
  if (!loc) return body
  loc.parent.items.splice(loc.index, 1)
  // A branch emptied by a delete is dropped; an empty branch would be a shunt.
  if (loc.parent.items.length === 0 && loc.owner && loc.branch !== undefined) {
    loc.owner.branches.splice(loc.branch, 1)
  }
  return normalize(b)
}

export function replaceElement(body: Series, id: string, el: Instruction): Series {
  const b = clone(body)
  const loc = locate(b, id)
  if (!loc) return body
  loc.parent.items[loc.index] = el
  return normalize(b)
}

export function updateElement(body: Series, id: string, patch: (e: Instruction) => Instruction): Series {
  const el = findElement(body, id)
  if (!el) return body
  return replaceElement(body, id, patch(clone(el)))
}

/**
 * Puts `id` in a parallel branch with a new empty branch below it; if `id`
 * is already a parallel, adds an empty branch to it. Returns the parallel's id.
 */
export function wrapInBranch(body: Series, id: string): { body: Series; parallelId: string } {
  const el = findElement(body, id)
  if (!el) return { body, parallelId: id }
  if (el.type === 'parallel') {
    const next = updateElement(body, id, (p) => {
      const par = p as Parallel
      return { ...par, branches: [...par.branches, { type: 'series', items: [] }] }
    })
    return { body: next, parallelId: id }
  }
  const b = clone(body)
  const loc = locate(b, id)
  if (!loc) return { body, parallelId: id }
  const par: Parallel = {
    type: 'parallel',
    id: newId('p'),
    branches: [{ type: 'series', items: [loc.parent.items[loc.index]] }, { type: 'series', items: [] }],
  }
  loc.parent.items[loc.index] = par
  return { body: b, parallelId: par.id }
}

/** Appends `el` to branch `branch` of parallel `parallelId`. */
export function appendToBranch(body: Series, parallelId: string, branch: number, el: Instruction): Series {
  return updateElement(body, parallelId, (p) => {
    const par = p as Parallel
    const branches = par.branches.map((s, i) => (i === branch ? { ...s, items: [...s.items, el] } : s))
    return { ...par, branches }
  })
}

/** Moves an element one step left (-1) or right (+1) within its series. */
export function moveElement(body: Series, id: string, dir: -1 | 1): Series {
  const b = clone(body)
  const loc = locate(b, id)
  if (!loc) return body
  const j = loc.index + dir
  if (j < 0 || j >= loc.parent.items.length) return body
  const items = loc.parent.items
  ;[items[loc.index], items[j]] = [items[j], items[loc.index]]
  return b
}

/** Gives a parsed body the ids of the old body where the shape matches (same DFS slot, same kind). */
export function reconcileIds(oldBody: Series, newBody: Series): Series {
  const olds = flatten(oldBody)
  const b = clone(newBody)
  const used = new Set<string>()
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
export function withNewIds(body: Series): Series {
  const b = clone(body)
  for (const e of flatten(b)) e.id = newId(e.id[0] ?? 'e')
  return b
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

/** Tags an instruction names that the project should have, with inferred types. */
export function tagUses(body: Series): TagUse[] {
  const out: TagUse[] = []
  for (const e of flatten(body)) {
    if (e.type === 'contact' || e.type === 'coil') {
      const name = baseTag(e.tag)
      // A member (`Timer.DN`) belongs to its structure tag; only plain names create BOOLs.
      if (name && name === e.tag.trim()) out.push({ name, type: 'BOOL' })
    } else if (e.type === 'box') {
      for (const o of BOX_SPECS[e.instr].operands) {
        if (!o.newTagType) continue
        const text = e.operands[o.key] ?? ''
        const name = baseTag(text)
        if (name && name === text.trim()) out.push({ name, type: o.newTagType })
      }
    }
  }
  return out
}

/** Adds any tag a body uses that the project lacks. Returns the tags that were created. */
export function createMissingTags(tags: Tag[], body: Series): { tags: Tag[]; created: Tag[] } {
  const have = new Set(tags.map((t) => t.name.toLowerCase()))
  const created: Tag[] = []
  for (const u of tagUses(body)) {
    if (have.has(u.name.toLowerCase())) continue
    have.add(u.name.toLowerCase())
    created.push({ name: u.name, type: u.type, initial: defaultInitial(u.type), comment: '' })
  }
  return { tags: created.length ? [...tags, ...created] : tags, created }
}

export function defaultInitial(type: string): string {
  switch (type.toUpperCase()) {
    case 'BOOL':
      return 'FALSE'
    case 'TIMER':
    case 'COUNTER':
      return ''
    case 'REAL':
    case 'LREAL':
      return '0.0'
    default:
      return '0'
  }
}

export function findTag(project: Project, name: string): Tag | undefined {
  const n = name.toLowerCase()
  return project.tags.find((t) => t.name.toLowerCase() === n)
}
