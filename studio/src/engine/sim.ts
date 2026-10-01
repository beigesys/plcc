// SPDX-License-Identifier: MPL-2.0
//
// Preview scan engine: interprets the ladder model with Logix-like semantics.
// The real simulator runs plcc's own wasm32 output (src/runtime/plcHost.ts);
// this engine runs while the compiler downloads or when compiling fails, and
// computes power flow from live values (Online, and the compiled simulator).
//
// Rung-condition-in starts TRUE. A series is an AND chain, and every
// instruction executes even when its input is FALSE (OTE writes FALSE, TON
// resets). A branch gives each leg the same input and ORs the outputs.

import { ProcessImage, boxSpec } from '@/model'
import type { Block, Element, Id, Program, Project, Routine } from '@/model'
import type { Device } from '@/devices/manifest'
import { evaluateExpression } from './expr'
import { TagStore, parseLiteral, toBool, toNumber } from './values'
import type { Scalar } from './values'

export interface ElementTrace {
  /** Power into the element. */
  in: boolean
  /** Power out of the element. */
  out: boolean
  /** The element's own state: contact passes, coil tag TRUE, box output or EN TRUE. */
  active: boolean
}

export type Trace = Map<Id, ElementTrace>

export interface ExecContext {
  read(ref: string): Scalar | undefined
  /** Absent in a pure trace: nothing is written. */
  write?: (ref: string, value: Scalar) => boolean
  dtMs: number
  trace: Trace
  errors: Map<Id, string>
  /** Hidden per-element memory (edge detectors), keyed by element id. */
  mem: Map<Id, Scalar>
  /** Runs a routine by name (JSR). Returns false if there is none. */
  jsr?: (name: string) => boolean
  /** Set by RET / JMP: the routine stops, or continues at the label. */
  ret?: boolean
  jumpTo?: string
}

const INT32_MAX = 2147483647
const INT32_MIN = -2147483648

function readBool(ctx: ExecContext, ref: string, id: Id): boolean {
  const v = ctx.read(ref)
  if (v === undefined) {
    ctx.errors.set(id, ref.trim() && ref.trim() !== '?' ? `unknown tag ${ref}` : 'missing operand')
    return false
  }
  return toBool(v)
}

function operandNum(ctx: ExecContext, text: string, id: Id): number {
  const lit = parseLiteral(text)
  if (lit !== undefined) return toNumber(lit)
  const v = ctx.read(text)
  if (v === undefined) {
    ctx.errors.set(id, text.trim() && text.trim() !== '?' ? `unknown tag ${text}` : 'missing operand')
    return 0
  }
  return toNumber(v)
}

function write(ctx: ExecContext, ref: string, value: Scalar, id: Id): void {
  if (!ctx.write) return
  if (!ctx.write(ref, value)) ctx.errors.set(id, ref.trim() ? `cannot write ${ref}` : 'missing operand')
}

function execSeries(s: Element[], pin: boolean, ctx: ExecContext): boolean {
  let p = pin
  for (const e of s) p = execElement(e, p, ctx)
  return p
}

function execElement(e: Element, pin: boolean, ctx: ExecContext): boolean {
  switch (e.type) {
    case 'branch': {
      let out = false
      for (const b of e.legs) out = execSeries(b, pin, ctx) || out
      ctx.trace.set(e.id, { in: pin, out, active: out })
      return out
    }
    case 'contact': {
      const v = readBool(ctx, e.operand, e.id)
      let pass: boolean
      if (e.kind === 'no') pass = v
      else if (e.kind === 'nc') pass = !v
      else {
        // Pure traces have no history: treat the previous value as the current one (no edge).
        const prev = ctx.write ? toBool(ctx.mem.get(e.id) ?? false) : v
        ctx.mem.set(e.id, v)
        pass = e.kind === 'rising' ? v && !prev : !v && prev
      }
      const out = pin && pass
      ctx.trace.set(e.id, { in: pin, out, active: pass })
      return out
    }
    case 'coil': {
      switch (e.kind) {
        case 'normal':
          write(ctx, e.operand, pin, e.id)
          break
        case 'negated':
          write(ctx, e.operand, !pin, e.id)
          break
        case 'set':
          if (pin) write(ctx, e.operand, true, e.id)
          break
        case 'reset':
          if (pin) write(ctx, e.operand, false, e.id)
          break
        case 'rising':
        case 'falling': {
          const prev = toBool(ctx.mem.get(e.id) ?? false)
          ctx.mem.set(e.id, pin)
          write(ctx, e.operand, e.kind === 'rising' ? pin && !prev : !pin && prev, e.id)
          break
        }
      }
      const active = readBool(ctx, e.operand, e.id)
      ctx.trace.set(e.id, { in: pin, out: pin, active })
      return pin
    }
    case 'block':
      return execBlock(e, pin, ctx)
    case 'jump':
      if (pin) ctx.jumpTo = e.label
      ctx.trace.set(e.id, { in: pin, out: pin, active: pin })
      return pin
    case 'return':
      if (pin) ctx.ret = true
      ctx.trace.set(e.id, { in: pin, out: pin, active: pin })
      return pin
    case 'st':
      ctx.errors.set(e.id, 'ST box: not run by the preview simulator')
      ctx.trace.set(e.id, { in: pin, out: pin, active: pin })
      return pin
  }
}

function execBlock(b: Block, pin: boolean, ctx: ExecContext): boolean {
  const op = (i: number) => b.pins[i]?.value ?? ''
  const num = (i: number) => operandNum(ctx, op(i), b.id)
  const set = (ref: string, v: Scalar) => write(ctx, ref, v, b.id)
  const member = (t: string, m: string) => `${t.trim()}.${m}`
  const bit = (t: string, m: string) => {
    const v = ctx.read(member(t, m))
    if (v === undefined) {
      ctx.errors.set(b.id, t.trim() && t.trim() !== '?' ? `unknown tag ${t}` : 'missing operand')
      return false
    }
    return toBool(v)
  }
  const acc = (t: string, m: string) => toNumber(ctx.read(member(t, m)) ?? 0)
  let out = pin
  let active = pin

  switch (b.name.toUpperCase()) {
    case 'TON':
    case 'TOF':
    case 'RTO': {
      const kind = b.name.toUpperCase()
      const t = op(0)
      const pre = num(1)
      const en = bit(t, 'EN')
      let dn = bit(t, 'DN')
      let a = acc(t, 'ACC')
      let tt: boolean
      if (!ctx.write) {
        ctx.trace.set(b.id, { in: pin, out: pin, active: en || dn })
        return pin
      }
      if (kind === 'TOF') {
        if (pin) {
          a = 0
          dn = true
          tt = false
        } else if (dn) {
          a += ctx.dtMs
          tt = true
          if (a >= pre) {
            a = pre
            dn = false
            tt = false
          }
        } else tt = false
      } else if (pin) {
        if (!dn) {
          a += ctx.dtMs
          if (a >= pre) {
            a = pre
            dn = true
          }
        }
        tt = !dn
      } else {
        tt = false
        if (kind === 'TON') {
          a = 0
          dn = false
        }
      }
      set(member(t, 'PRE'), pre)
      set(member(t, 'ACC'), a)
      set(member(t, 'EN'), pin)
      set(member(t, 'TT'), tt)
      set(member(t, 'DN'), dn)
      active = pin || dn
      break
    }
    case 'CTU':
    case 'CTD': {
      const up = b.name.toUpperCase() === 'CTU'
      const c = op(0)
      const pre = num(1)
      const edgeBit = up ? 'CU' : 'CD'
      const was = bit(c, edgeBit)
      if (!ctx.write) {
        ctx.trace.set(b.id, { in: pin, out: pin, active: bit(c, 'DN') })
        return pin
      }
      let a = acc(c, 'ACC')
      if (pin && !was) {
        if (up) {
          if (a >= INT32_MAX) {
            a = INT32_MIN
            set(member(c, 'OV'), true)
          } else a++
        } else if (a <= INT32_MIN) {
          a = INT32_MAX
          set(member(c, 'UN'), true)
        } else a--
      }
      set(member(c, 'PRE'), pre)
      set(member(c, 'ACC'), a)
      set(member(c, edgeBit), pin)
      set(member(c, 'DN'), a >= pre)
      active = a >= pre
      break
    }
    case 'RES': {
      const t = op(0)
      if (ctx.read(member(t, 'ACC')) === undefined) {
        ctx.errors.set(b.id, t.trim() && t.trim() !== '?' ? `${t} is not a timer or counter` : 'missing operand')
        break
      }
      if (pin && ctx.write) {
        for (const m of ['ACC', 'DN', 'TT', 'EN', 'CU', 'CD', 'OV', 'UN']) {
          if (ctx.read(member(t, m)) !== undefined) set(member(t, m), m === 'ACC' ? 0 : false)
        }
      }
      break
    }
    case 'ONS': {
      const s = op(0)
      const stored = readBool(ctx, s, b.id)
      out = pin && !stored
      if (ctx.write) set(s, pin)
      active = out
      break
    }
    case 'OSR':
    case 'OSF': {
      const s = op(0)
      const stored = readBool(ctx, s, b.id)
      if (ctx.write) {
        set(op(1), b.name.toUpperCase() === 'OSR' ? pin && !stored : !pin && stored)
        set(s, pin)
      }
      active = readBool(ctx, op(1), b.id)
      break
    }
    case 'EQU':
    case 'NEQ':
    case 'GRT':
    case 'GEQ':
    case 'LES':
    case 'LEQ': {
      const a = num(0)
      const c = num(1)
      const cmp = { EQU: a === c, NEQ: a !== c, GRT: a > c, GEQ: a >= c, LES: a < c, LEQ: a <= c }[b.name.toUpperCase() as 'EQU']
      out = pin && cmp
      active = cmp
      break
    }
    case 'LIM': {
      const lo = num(0)
      const t = num(1)
      const hi = num(2)
      const inside = lo <= hi ? lo <= t && t <= hi : t >= lo || t <= hi
      out = pin && inside
      active = inside
      break
    }
    case 'ADD':
    case 'SUB':
    case 'MUL':
    case 'DIV': {
      const a = num(0)
      const c = num(1)
      if (pin && ctx.write) {
        const name = b.name.toUpperCase()
        if (name === 'DIV' && c === 0) ctx.errors.set(b.id, 'divide by zero')
        else set(op(2), { ADD: a + c, SUB: a - c, MUL: a * c, DIV: a / c }[name as 'ADD'])
      }
      break
    }
    case 'MOV': {
      const v = num(0)
      if (pin && ctx.write) set(op(1), v)
      break
    }
    case 'CLR': {
      if (pin && ctx.write) set(op(0), 0)
      break
    }
    case 'CPT': {
      if (pin && ctx.write) {
        try {
          set(op(0), evaluateExpression(op(1), ctx.read))
        } catch (err) {
          ctx.errors.set(b.id, err instanceof Error ? err.message : String(err))
        }
      }
      break
    }
    case 'JSR': {
      if (pin && ctx.jsr && !ctx.jsr(op(0).trim())) ctx.errors.set(b.id, `JSR: no routine ${op(0)}`)
      break
    }
    case 'NOP':
      break
    case 'AFI':
      out = false
      active = false
      break
    default:
      ctx.errors.set(b.id, `${b.name} is not run by the preview simulator`)
  }
  ctx.trace.set(b.id, { in: pin, out, active })
  return out
}

/** Executes one rung and returns its rung-condition-out. Shared by the simulator and pure traces. */
export function executeRung(elements: Element[], ctx: ExecContext): boolean {
  return execSeries(elements, true, ctx)
}

/**
 * Power flow from given values, writing nothing (Online, and the compiled
 * simulator). Contacts read the values, coils show their tag, compares are
 * evaluated, timers and counters show their EN/DN (or DN) bits.
 */
export function traceRoutine(routine: Routine, read: (ref: string) => Scalar | undefined): Trace {
  const ctx: ExecContext = { read, dtMs: 0, trace: new Map(), errors: new Map(), mem: new Map() }
  for (const r of routine.rungs) executeRung(r.elements, ctx)
  return ctx.trace
}

const MAX_JSR_DEPTH = 8

/** True when `b` is an input instruction (it gates power). */
export function isConditionBox(b: Block): boolean {
  return boxSpec(b).role === 'input'
}

export class Simulator {
  readonly image: ProcessImage
  readonly tags: TagStore
  trace: Trace = new Map()
  errors = new Map<Id, string>()
  scanCount = 0
  lastScanMs = 0
  /** The last scan ran out of budget and was cut short. */
  overrun = false
  overrunCount = 0
  private project: Project
  private mem = new Map<Id, Scalar>()

  /** `device` sizes the process image (its manifest's `target.image`). */
  constructor(project: Project, device: Pick<Device, 'target'>) {
    this.project = project
    this.image = new ProcessImage(device.target.image)
    this.tags = new TagStore(project, this.image)
  }

  setProject(project: Project): void {
    this.project = project
    this.tags.sync(project)
  }

  reset(): void {
    this.image.I.fill(0)
    this.image.Q.fill(0)
    this.image.M.fill(0)
    this.mem.clear()
    this.tags.reset()
    this.trace = new Map()
    this.errors = new Map()
    this.scanCount = 0
  }

  private programsInOrder(): Program[] {
    const { tasks, pous } = this.project
    if (tasks.length === 0) return pous
    const out: Program[] = []
    for (const t of tasks) {
      for (const name of t.programs) {
        const p = pous.find((x) => x.name === name)
        if (p) out.push(p)
      }
    }
    return out
  }

  /**
   * Runs one scan. With `budgetMs`, the clock is checked between rungs (also
   * inside JSR); once the budget is spent the rest of the scan is skipped and
   * `overrun` is set for this scan.
   */
  scan(dtMs: number, opts: ScanOptions = {}): void {
    const now = opts.now ?? (() => performance.now())
    const start = now()
    const budget = opts.budgetMs
    this.tags.applyForces()
    const trace: Trace = new Map()
    const errors = new Map<Id, string>()
    let overrun = false
    try {
      this.runPrograms(dtMs, trace, errors, () => {
        if (budget !== undefined && now() - start > budget) throw OVERRUN
      })
    } catch (err) {
      if (err !== OVERRUN) throw err
      overrun = true
    }
    this.overrun = overrun
    if (overrun) this.overrunCount++
    this.trace = trace
    this.errors = errors
    this.scanCount++
    this.lastScanMs = now() - start
  }

  /** The current trace, packed for postMessage: bits in=1, out=2, active=4. */
  snapshotTrace(): Array<[Id, number]> {
    return packTrace(this.trace)
  }

  private runPrograms(dtMs: number, trace: Trace, errors: Map<Id, string>, check: () => void): void {
    for (const program of this.programsInOrder()) {
      let depth = 0
      const ctx: ExecContext = {
        read: (ref) => this.tags.read(ref),
        write: (ref, v) => this.tags.write(ref, v),
        dtMs,
        trace,
        errors,
        mem: this.mem,
      }
      const run = (routine: Routine) => {
        let i = 0
        while (i < routine.rungs.length) {
          check()
          executeRung(routine.rungs[i].elements, ctx)
          if (ctx.ret) {
            ctx.ret = false
            return
          }
          if (ctx.jumpTo !== undefined) {
            const label = ctx.jumpTo.toLowerCase()
            ctx.jumpTo = undefined
            const to = routine.rungs.findIndex((g) => g.label?.toLowerCase() === label)
            if (to >= 0) {
              i = to
              continue
            }
          }
          i++
        }
      }
      ctx.jsr = (name) => {
        const routine = program.routines.find((r) => r.name.toLowerCase() === name.toLowerCase())
        if (!routine) return false
        if (depth >= MAX_JSR_DEPTH) return true
        depth++
        try {
          run(routine)
        } finally {
          depth--
        }
        return true
      }
      const main = program.routines[0]
      if (main) run(main)
    }
  }
}

export interface ScanOptions {
  /** Abort the rest of the scan once this much time has passed. */
  budgetMs?: number
  /** Clock in ms (defaults to performance.now). */
  now?: () => number
}

const OVERRUN = Symbol('scan overrun')

export function packTrace(trace: Trace): Array<[Id, number]> {
  const out: Array<[Id, number]> = []
  for (const [id, t] of trace) out.push([id, (t.in ? 1 : 0) | (t.out ? 2 : 0) | (t.active ? 4 : 0)])
  return out
}

export function unpackTrace(packed: Array<[Id, number]>): Trace {
  const out: Trace = new Map()
  for (const [id, bits] of packed) out.set(id, { in: (bits & 1) !== 0, out: (bits & 2) !== 0, active: (bits & 4) !== 0 })
  return out
}
