// SPDX-License-Identifier: MPL-2.0
//
// SVG drawing of one rung. Energized wires and elements use --power, idle
// ones --line-idle; alarm/fault states use --alarm / --fault. Nothing else is
// colored (ISA-101).

import { memo, type DragEvent, type ReactNode } from 'react'
import type { ElementTrace, Trace } from '@/engine'
import { BOX_SPECS, type Box, type Coil, type Contact, type Instruction, type StBox, type Series } from '@/model'
import type { Scalar } from '@/runtime/messages'
import { G, type PlacedNode, type PlacedSeries, type RungLayout } from './layout'

export const PALETTE_MIME = 'application/x-plcc-instruction'

export interface LadderProps {
  body: Series
  /** Precomputed layout of `body` (the caller may need node positions too). */
  layout: RungLayout
  trace: Trace | null
  /** Rails and wires show power (Simulate / Online). */
  live: boolean
  read?: (ref: string) => Scalar | undefined
  /** Address to show under a contact/coil tag, e.g. %QX0.0. */
  addressOf?: (tag: string) => string | undefined
  errors?: Record<string, string>
  selectedId: string | null
  dropTarget?: string | null
  onSelect(id: string | null): void
  onActivate(id: string): void
  onDragTarget?(id: string | null): void
  onDropInstruction?(afterId: string | null, mnemonic: string): void
}

const POWER = 'var(--power)'
const IDLE = 'var(--line-idle)'
const FAULT = 'var(--fault)'
const ALARM = 'var(--alarm)'

function wire(on: boolean | undefined): string {
  return on ? POWER : IDLE
}

function Line({ x1, y1, x2, y2, on, w = 2 }: { x1: number; y1: number; x2: number; y2: number; on?: boolean; w?: number }) {
  if (x1 === x2 && y1 === y2) return null
  return <line x1={x1} y1={y1} x2={x2} y2={y2} stroke={wire(on)} strokeWidth={w} strokeLinecap="square" />
}

interface Ctx extends LadderProps {
  tr(id: string): ElementTrace | undefined
}

function ContactGlyph({ n, el, t, ctx }: { n: PlacedNode; el: Contact; t?: ElementTrace; ctx: Ctx }) {
  const cx = n.x + n.w / 2
  const y = n.wy
  const gap = 12
  const bh = 26
  const on = ctx.live && t?.active
  const addr = el.tag ? ctx.addressOf?.(el.tag) : undefined
  const mark = el.kind === 'rise' ? 'P' : el.kind === 'fall' ? 'N' : ''
  return (
    <>
      <Line x1={n.x} y1={y} x2={cx - gap} y2={y} on={ctx.live && t?.in} />
      <Line x1={cx + gap} y1={y} x2={n.x + n.w} y2={y} on={ctx.live && t?.out} />
      <line x1={cx - gap} y1={y - bh / 2} x2={cx - gap} y2={y + bh / 2} stroke={wire(on)} strokeWidth={2.5} />
      <line x1={cx + gap} y1={y - bh / 2} x2={cx + gap} y2={y + bh / 2} stroke={wire(on)} strokeWidth={2.5} />
      {el.kind === 'nc' && (
        <line x1={cx - gap + 4} y1={y + bh / 2 - 3} x2={cx + gap - 4} y2={y - bh / 2 + 3} stroke={wire(on)} strokeWidth={2} />
      )}
      {mark && (
        <text x={cx} y={y + 4} textAnchor="middle" className="ld-mark" fill={wire(on)}>
          {mark}
        </text>
      )}
      <TagLabel n={n} tag={el.tag} addr={addr} ctx={ctx} />
    </>
  )
}

function CoilGlyph({ n, el, t, ctx }: { n: PlacedNode; el: Coil; t?: ElementTrace; ctx: Ctx }) {
  const cx = n.x + n.w / 2
  const y = n.wy
  const r = 14
  const on = ctx.live && t?.active
  const addr = el.tag ? ctx.addressOf?.(el.tag) : undefined
  const mark = { normal: '', negated: '/', set: 'L', reset: 'U', rise: 'P', fall: 'N' }[el.kind]
  return (
    <>
      <Line x1={n.x} y1={y} x2={cx - r + 3} y2={y} on={ctx.live && t?.in} />
      <Line x1={cx + r - 3} y1={y} x2={n.x + n.w} y2={y} on={ctx.live && t?.out} />
      <path d={`M ${cx - r + 6} ${y - r} A ${r} ${r} 0 0 0 ${cx - r + 6} ${y + r}`} fill="none" stroke={wire(on)} strokeWidth={2.5} />
      <path d={`M ${cx + r - 6} ${y - r} A ${r} ${r} 0 0 1 ${cx + r - 6} ${y + r}`} fill="none" stroke={wire(on)} strokeWidth={2.5} />
      {mark && (
        <text x={cx} y={y + 4} textAnchor="middle" className="ld-mark" fill={wire(on)}>
          {mark}
        </text>
      )}
      <TagLabel n={n} tag={el.tag} addr={addr} ctx={ctx} />
    </>
  )
}

function TagLabel({ n, tag, addr, ctx }: { n: PlacedNode; tag: string; addr?: string; ctx: Ctx }) {
  const cx = n.x + n.w / 2
  const unknown = !tag
  return (
    <>
      <text x={cx} y={n.wy - 20} textAnchor="middle" className="ld-tag" fill={unknown ? ALARM : 'var(--text)'}>
        {clip(tag || '?', 13)}
      </text>
      {addr && (
        <text x={cx} y={n.wy + 28} textAnchor="middle" className="ld-addr" fill="var(--text-muted)">
          {addr}
        </text>
      )}
      {!addr && ctx.live && tag && ctx.read && <ValueText x={cx} y={n.wy + 28} v={ctx.read(tag)} />}
    </>
  )
}

function ValueText({ x, y, v, anchor = 'middle' }: { x: number; y: number; v: Scalar | undefined; anchor?: 'middle' | 'end' }) {
  if (v === undefined) return null
  const text = typeof v === 'boolean' ? (v ? '1' : '0') : Number.isInteger(v) ? String(v) : v.toFixed(2)
  return (
    <text x={x} y={y} textAnchor={anchor} className="ld-addr" fill="var(--text-muted)">
      {text}
    </text>
  )
}

function clip(s: string, n: number) {
  return s.length > n ? `${s.slice(0, n - 1)}…` : s
}

function BoxGlyph({ n, el, t, ctx }: { n: PlacedNode; el: Box; t?: ElementTrace; ctx: Ctx }) {
  const spec = BOX_SPECS[el.instr]
  const bx = n.x + 10
  const bw = n.w - 20
  const by = n.y + G.boxTop
  const bh = G.boxHeader + spec.operands.length * G.boxRow + 10
  const y = n.wy
  const on = ctx.live && t?.active
  const hasError = !!ctx.errors?.[el.id]
  const structTag = el.operands[spec.operands[0]?.key ?? ''] ?? ''
  return (
    <>
      <Line x1={n.x} y1={y} x2={bx} y2={y} on={ctx.live && t?.in} />
      <Line x1={bx + bw} y1={y} x2={n.x + n.w} y2={y} on={ctx.live && t?.out} />
      <rect
        x={bx}
        y={by}
        width={bw}
        height={bh}
        rx={6}
        className="ld-box"
        fill="var(--surface-2)"
        stroke={hasError ? FAULT : on ? POWER : IDLE}
        strokeWidth={1.5}
      />
      <text x={bx + 10} y={by + 16} className="ld-instr" fill="var(--text)">
        {el.instr}
      </text>
      <text x={bx + bw - 8} y={by + 16} textAnchor="end" className="ld-title" fill="var(--text-muted)">
        {clip(spec.title, 18)}
      </text>
      <line x1={bx} y1={by + G.boxHeader} x2={bx + bw} y2={by + G.boxHeader} stroke="var(--border)" strokeWidth={1} />
      {spec.operands.map((o, i) => {
        const ry = by + G.boxHeader + 14 + i * G.boxRow
        const text = el.operands[o.key] || '?'
        const isStructOp = i === 0 && (spec.group === 'Timers' || spec.group === 'Counters')
        let live: Scalar | undefined
        if (ctx.live && ctx.read) {
          if (isStructOp) live = undefined
          else if (o.key === 'preset' && structTag) live = undefined
          else if (o.key === 'accum' && structTag) live = ctx.read(`${structTag}.ACC`)
          else if (/^[A-Za-z_]/.test(text)) live = ctx.read(text)
        }
        return (
          <g key={o.key}>
            <text x={bx + 10} y={ry} className="ld-oplabel" fill="var(--text-muted)">
              {o.label}
            </text>
            <text x={bx + bw - 8} y={ry} textAnchor="end" className="ld-op" fill={el.operands[o.key] ? 'var(--text)' : ALARM}>
              {clip(text, live === undefined ? 14 : 9)}
              {live !== undefined && <tspan fill="var(--text-muted)">{` ${fmt(live)}`}</tspan>}
            </text>
          </g>
        )
      })}
      {spec.outputs?.map((bit, i) => {
        const v = ctx.live && ctx.read && structTag ? ctx.read(`${structTag}.${bit}`) : undefined
        const py = by + G.boxHeader + 14 + i * G.boxRow
        return (
          <g key={bit}>
            <circle cx={bx + bw + 1} cy={py - 4} r={3} fill={v ? POWER : 'var(--surface-2)'} stroke={v ? POWER : IDLE} strokeWidth={1.2} />
            <text x={bx + bw + 7} y={py} className="ld-pin" fill="var(--text-muted)">
              {bit}
            </text>
          </g>
        )
      })}
    </>
  )
}

function fmt(v: Scalar): string {
  if (typeof v === 'boolean') return v ? '1' : '0'
  return Number.isInteger(v) ? String(v) : v.toFixed(2)
}

function StGlyph({ n, el, t, ctx }: { n: PlacedNode; el: StBox; t?: ElementTrace; ctx: Ctx }) {
  const bx = n.x + 10
  const bw = n.w - 20
  const by = n.y + G.boxTop
  const lines = el.code.split('\n').slice(0, 6)
  const bh = G.boxHeader + Math.max(1, lines.length) * G.stLine + 10
  return (
    <>
      <Line x1={n.x} y1={n.wy} x2={bx} y2={n.wy} on={ctx.live && t?.in} />
      <Line x1={bx + bw} y1={n.wy} x2={n.x + n.w} y2={n.wy} on={ctx.live && t?.out} />
      <rect x={bx} y={by} width={bw} height={bh} rx={6} fill="var(--surface-2)" stroke={IDLE} strokeWidth={1.5} strokeDasharray="4 3" />
      <text x={bx + 10} y={by + 16} className="ld-instr" fill="var(--text)">
        ST
      </text>
      <text x={bx + bw - 8} y={by + 16} textAnchor="end" className="ld-title" fill="var(--text-muted)">
        Structured Text
      </text>
      {lines.map((l, i) => (
        <text key={i} x={bx + 10} y={by + G.boxHeader + 14 + i * G.stLine} className="ld-op" fill="var(--text)">
          {clip(l, 24)}
        </text>
      ))}
    </>
  )
}

function NodeView({ n, ctx }: { n: PlacedNode; ctx: Ctx }) {
  const el = n.el
  const t = ctx.tr(el.id)
  const selected = ctx.selectedId === el.id
  const isDrop = ctx.dropTarget === el.id
  const error = ctx.errors?.[el.id]
  let glyph: ReactNode = null
  if (el.type === 'contact') glyph = <ContactGlyph n={n} el={el} t={t} ctx={ctx} />
  else if (el.type === 'coil') glyph = <CoilGlyph n={n} el={el} t={t} ctx={ctx} />
  else if (el.type === 'box') glyph = <BoxGlyph n={n} el={el} t={t} ctx={ctx} />
  else if (el.type === 'st') glyph = <StGlyph n={n} el={el} t={t} ctx={ctx} />
  else if (el.type === 'parallel') return <ParallelView n={n} ctx={ctx} t={t} />

  const drag = dragHandlers(ctx, el.id)
  return (
    <g
      data-element-id={el.id}
      onClick={(e) => {
        e.stopPropagation()
        ctx.onSelect(el.id)
      }}
      onDoubleClick={(e) => {
        e.stopPropagation()
        ctx.onActivate(el.id)
      }}
      {...drag}
      className="ld-node"
    >
      <rect
        x={n.x + 2}
        y={n.y + 1}
        width={n.w - 4}
        height={n.h - 2}
        rx={6}
        fill={selected ? 'var(--select-bg)' : 'transparent'}
        stroke={selected ? 'var(--focus-ring)' : isDrop ? 'var(--text-muted)' : 'transparent'}
        strokeDasharray={isDrop && !selected ? '4 3' : undefined}
        strokeWidth={1.25}
      />
      {glyph}
      {error && <title>{error}</title>}
    </g>
  )
}

function dragHandlers(ctx: Ctx, id: string | null) {
  if (!ctx.onDropInstruction) return {}
  return {
    onDragOver: (e: DragEvent) => {
      if (!e.dataTransfer.types.includes(PALETTE_MIME)) return
      e.preventDefault()
      e.stopPropagation()
      e.dataTransfer.dropEffect = 'copy'
      if (ctx.dropTarget !== id) ctx.onDragTarget?.(id)
    },
    onDrop: (e: DragEvent) => {
      const m = e.dataTransfer.getData(PALETTE_MIME)
      if (!m) return
      e.preventDefault()
      e.stopPropagation()
      ctx.onDragTarget?.(null)
      ctx.onDropInstruction?.(id, m)
    },
  }
}

function seriesEnd(s: PlacedSeries): number {
  const last = s.nodes[s.nodes.length - 1]
  return last ? last.x + last.w : s.x
}

function seriesOut(s: PlacedSeries, ctx: Ctx, inp: boolean | undefined): boolean | undefined {
  const last = s.nodes[s.nodes.length - 1]
  return last ? ctx.tr(last.el.id)?.out : inp
}

function ParallelView({ n, ctx, t }: { n: PlacedNode; ctx: Ctx; t?: ElementTrace }) {
  const branches = n.branches ?? []
  const first = branches[0]
  const last = branches[branches.length - 1]
  if (!first || !last) return null
  const lx = n.x + 4
  const rx = n.x + n.w - 4
  const live = ctx.live
  const selected = ctx.selectedId === n.el.id
  return (
    <g data-element-id={n.el.id}>
      <rect
        x={n.x}
        y={n.y}
        width={n.w}
        height={n.h}
        rx={6}
        fill="transparent"
        stroke={selected ? 'var(--focus-ring)' : 'transparent'}
        strokeDasharray="5 4"
        strokeWidth={1.25}
        onClick={(e) => {
          e.stopPropagation()
          ctx.onSelect(n.el.id)
        }}
      />
      <Line x1={n.x} y1={first.wy} x2={lx} y2={first.wy} on={live && t?.in} />
      <Line x1={lx} y1={first.wy} x2={lx} y2={last.wy} on={live && t?.in} />
      <Line x1={rx} y1={first.wy} x2={n.x + n.w} y2={first.wy} on={live && t?.out} />
      {branches.map((b, i) => {
        const out = seriesOut(b, ctx, t?.in)
        const next = branches[i + 1]
        return (
          <g key={i}>
            <Line x1={lx} y1={b.wy} x2={b.x} y2={b.wy} on={live && t?.in} />
            <Line x1={seriesEnd(b)} y1={b.wy} x2={rx} y2={b.wy} on={live && out} />
            {next && <Line x1={rx} y1={b.wy} x2={rx} y2={next.wy} on={live && branchesOut(branches, i + 1, ctx, t?.in)} />}
            {b.nodes.map((c) => (
              <NodeView key={c.el.id} n={c} ctx={ctx} />
            ))}
          </g>
        )
      })}
    </g>
  )
}

/** A right-leg segment above branch j carries power if any branch at or below j does. */
function branchesOut(bs: PlacedSeries[], j: number, ctx: Ctx, inp: boolean | undefined): boolean {
  for (let i = j; i < bs.length; i++) if (seriesOut(bs[i], ctx, inp)) return true
  return false
}

function LadderImpl(props: LadderProps) {
  const layout = props.layout
  const ctx: Ctx = { ...props, tr: (id) => props.trace?.get(id) }
  const live = props.live
  const insLast = layout.inputs.nodes[layout.inputs.nodes.length - 1]
  const stretchOn = live && (insLast ? ctx.tr(insLast.el.id)?.out : true)
  const outFirst = layout.outputs.nodes[0]
  const railTop = 2
  const railBottom = layout.height - 2
  const empty = props.body.items.length === 0
  return (
    <svg
      width={layout.width}
      height={layout.height}
      viewBox={`0 0 ${layout.width} ${layout.height}`}
      className="ld block select-none"
      role="img"
      aria-hidden="true"
      onClick={() => props.onSelect(null)}
      {...dragHandlers(ctx, null)}
    >
      <line x1={G.railX - 6} y1={railTop} x2={G.railX - 6} y2={railBottom} stroke={live ? POWER : IDLE} strokeWidth={3} />
      <line
        x1={layout.width - G.railPad + 6}
        y1={railTop}
        x2={layout.width - G.railPad + 6}
        y2={railBottom}
        stroke={IDLE}
        strokeWidth={3}
      />
      <Line x1={G.railX - 6} y1={layout.wy} x2={G.railX} y2={layout.wy} on={live} />
      <Line x1={layout.stretchFrom} y1={layout.wy} x2={layout.stretchTo} y2={layout.wy} on={stretchOn} />
      {!outFirst && (
        <Line x1={layout.stretchTo} y1={layout.wy} x2={layout.width - G.railPad + 6} y2={layout.wy} on={stretchOn} />
      )}
      {empty && (
        <text x={layout.width / 2} y={layout.wy - 8} textAnchor="middle" className="ld-title" fill="var(--text-muted)">
          Empty rung: press C, O or B, or drop an instruction here
        </text>
      )}
      {layout.inputs.nodes.map((n) => (
        <NodeView key={n.el.id} n={n} ctx={ctx} />
      ))}
      {layout.outputs.nodes.map((n) => (
        <NodeView key={n.el.id} n={n} ctx={ctx} />
      ))}
    </svg>
  )
}

export const Ladder = memo(LadderImpl)

export function describeElement(el: Instruction): string {
  switch (el.type) {
    case 'contact':
      return `${{ no: 'Normally open contact', nc: 'Normally closed contact', rise: 'Rising-edge contact', fall: 'Falling-edge contact' }[el.kind]} ${el.tag || 'unassigned'}`
    case 'coil':
      return `${{ normal: 'Coil', negated: 'Negated coil', set: 'Latch coil', reset: 'Unlatch coil', rise: 'Rising-edge coil', fall: 'Falling-edge coil' }[el.kind]} ${el.tag || 'unassigned'}`
    case 'box':
      return `${el.instr} ${Object.values(el.operands).filter(Boolean).join(', ')}`
    case 'st':
      return 'Structured Text box'
    case 'parallel':
      return `Branch with ${el.branches.length} legs`
  }
}
