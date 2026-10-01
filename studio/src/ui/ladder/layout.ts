// SPDX-License-Identifier: MPL-2.0
//
// Ladder geometry. Pure: sizes and positions only, no drawing. Every node knows
// its box (x, y, w, h) and the y of the wire running through it.

import { boxSpec, isOutput, type Element } from '@/model'

export const G = {
  railX: 14,
  railPad: 14,
  contactW: 92,
  contactH: 66,
  contactWire: 34,
  boxW: 172,
  boxTop: 8,
  boxHeader: 24,
  boxRow: 18,
  stW: 200,
  stLine: 16,
  branchPad: 16,
  branchGap: 2,
  emptyW: 44,
  emptyH: 40,
  stretchMin: 40,
  padY: 6,
}

export interface Size {
  w: number
  h: number
  /** Wire y relative to the node's top. */
  wo: number
}

export interface PlacedNode {
  el: Element
  x: number
  y: number
  w: number
  h: number
  /** Absolute wire y. */
  wy: number
  /** Branches only. */
  legs?: PlacedSeries[]
}

export interface PlacedSeries {
  x: number
  y: number
  w: number
  h: number
  wy: number
  nodes: PlacedNode[]
  /** Width left over to the right of the last node (legs shorter than their branch). */
  end: number
}

export function boxRows(el: Element): number {
  if (el.type === 'block') return Math.max(1, el.pins.length)
  if (el.type === 'st') return Math.max(1, Math.min(6, el.code.split('\n').length))
  return 0
}

function measureSeries(items: Element[]): Size {
  if (items.length === 0) return { w: G.emptyW, h: G.emptyH, wo: G.emptyH / 2 }
  const sizes = items.map(measure)
  const wo = Math.max(...sizes.map((s) => s.wo))
  const h = Math.max(...sizes.map((s) => wo - s.wo + s.h))
  return { w: sizes.reduce((a, s) => a + s.w, 0), h, wo }
}

export function measure(e: Element): Size {
  switch (e.type) {
    case 'contact':
    case 'coil':
    case 'jump':
    case 'return':
      return { w: G.contactW, h: G.contactH, wo: G.contactWire }
    case 'block':
      return {
        w: G.boxW + (boxSpec(e).outputs ? 26 : 0),
        h: G.boxTop + G.boxHeader + boxRows(e) * G.boxRow + 10 + G.padY,
        wo: G.boxTop + G.boxHeader / 2,
      }
    case 'st':
      return {
        w: G.stW,
        h: G.boxTop + G.boxHeader + boxRows(e) * G.stLine + 10 + G.padY,
        wo: G.boxTop + G.boxHeader / 2,
      }
    case 'branch': {
      const sizes = e.legs.map(measureSeries)
      const w = Math.max(...sizes.map((s) => s.w)) + 2 * G.branchPad
      const h = sizes.reduce((a, s) => a + s.h, 0) + G.branchGap * (sizes.length - 1)
      return { w, h, wo: sizes[0]?.wo ?? G.emptyH / 2 }
    }
  }
}

/** Places a series with its wire at absolute `wy`, starting at `x`, `width` wide (>= its natural width). */
export function placeSeries(items: Element[], x: number, wy: number, width?: number): PlacedSeries {
  const size = measureSeries(items)
  const w = Math.max(width ?? size.w, size.w)
  const nodes: PlacedNode[] = []
  let cx = x
  for (const item of items) {
    nodes.push(placeNode(item, cx, wy))
    cx += measure(item).w
  }
  return { x, y: wy - size.wo, w, h: size.h, wy, nodes, end: x + w }
}

function placeNode(el: Element, x: number, wy: number): PlacedNode {
  const size = measure(el)
  const node: PlacedNode = { el, x, y: wy - size.wo, w: size.w, h: size.h, wy }
  if (el.type === 'branch') {
    const inner = size.w - 2 * G.branchPad
    let top = node.y
    node.legs = el.legs.map((b) => {
      const bs = measureSeries(b)
      const placed = placeSeries(b, x + G.branchPad, top + bs.wo, inner)
      top += bs.h + G.branchGap
      return placed
    })
  }
  return node
}

export interface RungLayout {
  width: number
  height: number
  wy: number
  inputs: PlacedSeries
  outputs: PlacedSeries
  /** x where the stretch wire between inputs and outputs starts / ends. */
  stretchFrom: number
  stretchTo: number
}

function onlyOutputs(e: Element): boolean {
  if (e.type === 'branch') return e.legs.every((b) => b.length > 0 && b.every(onlyOutputs))
  return isOutput(e)
}

/** Splits the top level into leading inputs and trailing outputs, which sit against the right rail. */
export function layoutRung(items: Element[], minWidth: number): RungLayout {
  let split = items.length
  while (split > 0 && onlyOutputs(items[split - 1])) split--
  const ins = items.slice(0, split)
  const outs = items.slice(split)
  const si = ins.length ? measureSeries(ins) : { w: 0, h: G.emptyH, wo: G.emptyH / 2 }
  const so = outs.length ? measureSeries(outs) : { w: 0, h: G.emptyH, wo: G.emptyH / 2 }
  const wo = Math.max(si.wo, so.wo)
  const wy = wo + G.padY
  const natural = G.railX + si.w + G.stretchMin + so.w + G.railPad
  const width = Math.max(minWidth, natural)
  const height = Math.max(wo - si.wo + si.h, wo - so.wo + so.h) + 2 * G.padY
  const inputs = placeSeries(ins, G.railX, wy)
  const outX = width - G.railPad - so.w
  const outputs = placeSeries(outs, outX, wy)
  return {
    width,
    height,
    wy,
    inputs,
    outputs,
    stretchFrom: G.railX + si.w,
    stretchTo: outX,
  }
}

/** Every placed node, depth first. */
export function allNodes(l: RungLayout): PlacedNode[] {
  const out: PlacedNode[] = []
  const visit = (s: PlacedSeries) => {
    for (const n of s.nodes) {
      out.push(n)
      n.legs?.forEach(visit)
    }
  }
  visit(l.inputs)
  visit(l.outputs)
  return out
}
