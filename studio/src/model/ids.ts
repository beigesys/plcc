// SPDX-License-Identifier: MPL-2.0
//
// Element ids are numbers unique in a project (plcc-ladder's `Id`). One
// counter hands them out; opening a project moves it past the project's
// largest id, so new elements never collide with saved ones.

import type { Element, Project } from './types'

let next = 0

/** A fresh id. */
export function newId(): number {
  next += 1
  return next
}

/** Make sure later ids are above `id`. */
export function reserveId(id: number): void {
  if (id > next) next = id
}

function walkIds(items: Element[], f: (id: number) => void): void {
  for (const e of items) {
    f(e.id)
    if (e.type === 'branch') e.legs.forEach((l) => walkIds(l, f))
    else if (e.type === 'block') for (const p of e.pins) if (p.rung) walkIds(p.rung, f)
  }
}

/** The largest id in a project. */
export function maxId(p: Pick<Project, 'pous'>): number {
  let m = 0
  const see = (id: number) => {
    if (id > m) m = id
  }
  for (const pou of p.pous) {
    see(pou.id)
    for (const r of pou.routines) {
      see(r.id)
      for (const g of r.rungs) {
        see(g.id)
        walkIds(g.elements, see)
      }
    }
  }
  return m
}

/** Called when a project is opened: new ids go above its own. */
export function reserveProjectIds(p: Pick<Project, 'pous'>): void {
  reserveId(maxId(p))
}
