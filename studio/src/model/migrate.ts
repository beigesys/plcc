// SPDX-License-Identifier: MPL-2.0
//
// Projects saved by the first studio (its draft model: `tags.toml`,
// `routines/<name>.ladder.json` with { format: "plcc-studio-ladder",
// version: 1 }) into the plcc-ladder model.
//
// The draft had a few plcc extensions Logix ladder has no instruction for;
// they become their Logix equivalents (what plcc's IEC → Logix translation
// does), and each is reported:
//
//   rising / falling contact X   → a rung before: XIC(X)OSR(edgeN_sb,edgeN) (OSF), and XIC(edgeN) in place
//   rising / falling coil X      → OSR(edgeN_sb,X) / OSF(edgeN_sb,X) in place
//   negated coil X               → OTE(negN) in place, and a rung after: XIO(negN)OTE(X)

import { newId } from './ids'
import { boxSpec, logixOperandDir } from './instructions'
import { newTag } from './ops'
import type { Element, Pou, Routine, Rung, Tag, Task } from './types'

type Obj = Record<string, unknown>

/** Draft operand keys → plcc pin names, per instruction. */
const PIN_NAMES: Record<string, Record<string, string>> = {
  timer: { timer: 'Timer', preset: 'Preset', accum: 'Accum' },
  counter: { counter: 'Counter', preset: 'Preset', accum: 'Accum' },
  RES: { target: 'Structure' },
  ONS: { storage: 'Storage Bit' },
  OSR: { storage: 'Storage Bit', output: 'Output Bit' },
  compare: { a: 'Source A', b: 'Source B' },
  math: { a: 'Source A', b: 'Source B', dest: 'Dest' },
  MOV: { source: 'Source', dest: 'Dest' },
  CPT: { dest: 'Dest', expr: 'Expression' },
}

function pinNames(instr: string): Record<string, string> {
  if (['TON', 'TOF', 'RTO'].includes(instr)) return PIN_NAMES.timer
  if (['CTU', 'CTD'].includes(instr)) return PIN_NAMES.counter
  if (['EQU', 'NEQ', 'GRT', 'GEQ', 'LES', 'LEQ'].includes(instr)) return PIN_NAMES.compare
  if (['ADD', 'SUB', 'MUL', 'DIV'].includes(instr)) return PIN_NAMES.math
  return PIN_NAMES[instr] ?? {}
}

function op(v: unknown): string {
  return typeof v === 'string' && v.trim() !== '' ? v : '?'
}

export interface Migration {
  /** Tags the replacements need. */
  tags: Tag[]
  notes: string[]
}

interface Ctx extends Migration {
  before: Element[][]
  after: Element[][]
  where: string
}


function element(e: Obj, ctx: Ctx): Element[] {
  switch (e.type) {
    case 'series':
      return series(e, ctx)
    case 'parallel':
      return [{ type: 'branch', id: newId(), legs: ((e.branches as Obj[]) ?? []).map((b) => series(b, ctx)) }]
    case 'contact': {
      const tag = op(e.tag)
      if (e.kind === 'nc') return [{ type: 'contact', id: newId(), operand: tag, kind: 'nc' }]
      if (e.kind === 'rise' || e.kind === 'fall') {
        const n = newId()
        const q = `edge${n}`
        const sb = `edge${n}_sb`
        ctx.tags.push(newTag(q, 'BOOL', { comment: `${e.kind === 'rise' ? 'rising' : 'falling'} edge of ${tag}` }), newTag(sb, 'BOOL'))
        ctx.before.push([
          { type: 'contact', id: newId(), operand: tag, kind: 'no' },
          {
            type: 'block', id: newId(), name: e.kind === 'rise' ? 'OSR' : 'OSF',
            pins: [{ name: 'Storage Bit', dir: 'in_out', value: sb }, { name: 'Output Bit', dir: 'output', value: q }],
          },
        ])
        ctx.notes.push(`${ctx.where}: the ${e.kind === 'rise' ? 'rising' : 'falling'}-edge contact on ${tag} is now XIC(${q}), with ${q} set by ${e.kind === 'rise' ? 'OSR' : 'OSF'} in a rung before (Logix has no edge contact)`)
        return [{ type: 'contact', id: newId(), operand: q, kind: 'no' }]
      }
      return [{ type: 'contact', id: newId(), operand: tag, kind: 'no' }]
    }
    case 'coil': {
      const tag = op(e.tag)
      if (e.kind === 'set' || e.kind === 'reset' || e.kind === 'normal' || e.kind === undefined) {
        return [{ type: 'coil', id: newId(), operand: tag, kind: (e.kind as 'set' | 'reset' | 'normal') ?? 'normal' }]
      }
      if (e.kind === 'rise' || e.kind === 'fall') {
        const n = newId()
        const sb = `edge${n}_sb`
        ctx.tags.push(newTag(sb, 'BOOL'))
        ctx.notes.push(`${ctx.where}: the ${e.kind === 'rise' ? 'rising' : 'falling'}-edge coil on ${tag} is now ${e.kind === 'rise' ? 'OSR' : 'OSF'}(${sb},${tag})`)
        return [{
          type: 'block', id: newId(), name: e.kind === 'rise' ? 'OSR' : 'OSF',
          pins: [{ name: 'Storage Bit', dir: 'in_out', value: sb }, { name: 'Output Bit', dir: 'output', value: tag }],
        }]
      }
      // negated
      const n = newId()
      const neg = `neg${n}`
      ctx.tags.push(newTag(neg, 'BOOL'))
      ctx.after.push([
        { type: 'contact', id: newId(), operand: neg, kind: 'nc' },
        { type: 'coil', id: newId(), operand: tag, kind: 'normal' },
      ])
      ctx.notes.push(`${ctx.where}: the negated coil on ${tag} is now OTE(${neg}), with ${tag} written from it in a rung after (Logix has no negated coil)`)
      return [{ type: 'coil', id: newId(), operand: neg, kind: 'normal' }]
    }
    case 'box': {
      const instr = String(e.instr)
      const ops = (e.operands as Obj) ?? {}
      const names = pinNames(instr)
      const spec = boxSpec({ name: instr, pins: [] })
      if (instr === 'JSR') {
        return [{
          type: 'block', id: newId(), name: 'JSR',
          pins: [{ name: 'Routine Name', dir: 'input', value: op(ops.routine) }, { name: 'Input Count', dir: 'input', value: '0' }],
        }]
      }
      const keys = Object.keys(names)
      const pins = (keys.length ? keys : Object.keys(ops)).map((k, i) => ({
        name: names[k] ?? spec.pins[i]?.name ?? k,
        dir: logixOperandDir(instr, i),
        value: op(ops[k]),
      }))
      return [{ type: 'block', id: newId(), name: instr, pins }]
    }
    case 'st':
      return [{ type: 'st', id: newId(), code: typeof e.code === 'string' ? e.code : '' }]
    default:
      ctx.notes.push(`${ctx.where}: an element of unknown type ${JSON.stringify(e.type)} was left out`)
      return []
  }
}

function series(s: Obj, ctx: Ctx): Element[] {
  return ((s.items as Obj[]) ?? []).flatMap((e) => element(e, ctx))
}

/** One draft routine's rungs. */
function rungs(draft: Obj[], ctx: Migration, where: string): Rung[] {
  const out: Rung[] = []
  draft.forEach((r, i) => {
    const c: Ctx = { ...ctx, before: [], after: [], where: `${where}, rung ${i}` }
    const elements = series((r.body as Obj) ?? { items: [] }, c)
    for (const b of c.before) out.push({ id: newId(), elements: b })
    const comment = typeof r.comment === 'string' && r.comment ? r.comment : undefined
    out.push(comment ? { id: newId(), comment, elements } : { id: newId(), elements })
    for (const a of c.after) out.push({ id: newId(), elements: a })
  })
  return out
}

export interface DraftRoutine {
  name: string
  kind: 'ladder' | 'st'
  /** Parsed `routines/<name>.ladder.json` rungs. */
  rungs?: Obj[]
  st?: string
}

export interface DraftProgram {
  name: string
  main: string
  routines: DraftRoutine[]
}

/** Draft programs → POUs (main routine first). */
export function migratePrograms(programs: DraftProgram[], m: Migration): Pou[] {
  return programs.map((p) => {
    const ordered = [...p.routines].sort((a, b) => Number(b.name === p.main) - Number(a.name === p.main))
    const routines: Routine[] = ordered.map((r) => {
      if (r.kind === 'st') {
        return { id: newId(), name: r.name, rungs: [{ id: newId(), elements: [{ type: 'st', id: newId(), code: r.st ?? '' }] }] }
      }
      return { id: newId(), name: r.name, rungs: rungs(r.rungs ?? [], m, `${p.name}/${r.name}`) }
    })
    return { id: newId(), name: p.name, kind: 'program', variables: [], routines }
  })
}

/** A draft tag: `initial` "", "0", "FALSE", "0.0" was the default and is left out. */
export function migrateTag(t: { name: string; type: string; initial?: string; address?: string; comment?: string }): Tag {
  const extra: Partial<Tag> = {}
  const init = (t.initial ?? '').trim()
  if (init && !/^(0|0\.0|false)$/i.test(init)) extra.initial = init === 'TRUE' ? '1' : init
  if (t.address) extra.address = t.address
  if (t.comment) extra.comment = t.comment
  return newTag(t.name, t.type, extra)
}

export function migrateTask(t: { name: string; intervalMs: number; programs: string[] }): Task {
  return { name: t.name, interval_ms: t.intervalMs, programs: [...t.programs] }
}
