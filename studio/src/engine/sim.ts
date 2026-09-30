// SPDX-License-Identifier: MPL-2.0
//
// Preview scan engine: interprets the ladder model with Logix-like semantics.
// It exists until plcc's wasm32 output can run the real program in the page.
//
// Rung-condition-in starts TRUE. A series is an AND chain, and every
// instruction executes even when its input is FALSE (OTE writes FALSE, TON
// resets). A parallel gives each branch the same input and ORs the outputs.

import { BOX_SPECS, ProcessImage } from '@/model'
import type { Box, Element, Program, Project, Routine, Series } from '@/model'
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

export type Trace = Map<string, ElementTrace>

export interface ExecContext {
  read(ref: string): Scalar | undefined
  /** Absent in a pure trace: nothing is written. */
  write?: (ref: string, value: Scalar) => boolean
  dtMs: number
  trace: Trace
  errors: Map<string, string>
  /** Hidden per-element memory (edge detectors), keyed by element id. */
  mem: Map<string, Scalar>
  /** Runs a routine by name (JSR). Returns false if there is none. */
  jsr?: (name: string) => boolean
}

const INT32_MAX = 2147483647
const INT32_MIN = -2147483648

function readBool(ctx: ExecContext, ref: string, id: string): boolean {
  const v = ctx.read(ref)
  if (v === undefined) {
    ctx.errors.set(id, ref.trim() ? `unknown tag ${ref}` : 'missing operand')
    return false
  }
  return toBool(v)
}

function operandNum(ctx: ExecContext, text: string, id: string): number {
  const lit = parseLiteral(text)
  if (lit !== undefined) return toNumber(lit)
  const v = ctx.read(text)
  if (v === undefined) {
    ctx.errors.set(id, text.trim() ? `unknown tag ${text}` : 'missing operand')
    return 0
  }
  return toNumber(v)
}

function write(ctx: ExecContext, ref: string, value: Scalar, id: string): void {
  if (!ctx.write) return
  if (!ctx.write(ref, value)) ctx.errors.set(id, ref.trim() ? `cannot write ${ref}` : 'missing operand')
}

function execSeries(s: Series, pin: boolean, ctx: ExecContext): boolean {
  let p = pin
  for (const e of s.items) p = execElement(e, p, ctx)
  return p
}

function execElement(e: Element, pin: boolean, ctx: ExecContext): boolean {
  switch (e.type) {
    case 'series':
      return execSeries(e, pin, ctx)
    case 'parallel': {
      let out = false
      for (const b of e.branches) out = execSeries(b, pin, ctx) || out
      ctx.trace.set(e.id, { in: pin, out, active: out })
      return out
    }
    case 'contact': {
      const v = readBool(ctx, e.tag, e.id)
      let pass: boolean
      if (e.kind === 'no') pass = v
      else if (e.kind === 'nc') pass = !v
      else {
        // Pure traces have no history: treat the previous value as the current one (no edge).
        const prev = ctx.write ? toBool(ctx.mem.get(e.id) ?? false) : v
        ctx.mem.set(e.id, v)
        pass = e.kind === 'rise' ? v && !prev : !v && prev
      }
      const out = pin && pass
      ctx.trace.set(e.id, { in: pin, out, active: pass })
      return out
    }
    case 'coil': {
      switch (e.kind) {
        case 'normal':
          write(ctx, e.tag, pin, e.id)
          break
        case 'negated':
          write(ctx, e.tag, !pin, e.id)
          break
        case 'set':
          if (pin) write(ctx, e.tag, true, e.id)
          break
        case 'reset':
          if (pin) write(ctx, e.tag, false, e.id)
          break
        case 'rise':
        case 'fall': {
          const prev = toBool(ctx.mem.get(e.id) ?? false)
          ctx.mem.set(e.id, pin)
          write(ctx, e.tag, e.kind === 'rise' ? pin && !prev : !pin && prev, e.id)
          break
        }
      }
      const active = readBool(ctx, e.tag, e.id)
      ctx.trace.set(e.id, { in: pin, out: pin, active })
      return pin
    }
    case 'box':
      return execBox(e, pin, ctx)
    case 'st':
      ctx.errors.set(e.id, 'ST box not simulated')
      ctx.trace.set(e.id, { in: pin, out: pin, active: pin })
      return pin
  }
}

function execBox(b: Box, pin: boolean, ctx: ExecContext): boolean {
  const op = (k: string) => b.operands[k] ?? ''
  const num = (k: string) => operandNum(ctx, op(k), b.id)
  const set = (ref: string, v: Scalar) => write(ctx, ref, v, b.id)
  const member = (t: string, m: string) => `${t.trim()}.${m}`
  const bit = (t: string, m: string) => {
    const v = ctx.read(member(t, m))
    if (v === undefined) {
      ctx.errors.set(b.id, t.trim() ? `unknown tag ${t}` : 'missing operand')
      return false
    }
    return toBool(v)
  }
  const acc = (t: string, m: string) => toNumber(ctx.read(member(t, m)) ?? 0)
  let out = pin
  let active = pin

  switch (b.instr) {
    case 'TON':
    case 'TOF':
    case 'RTO': {
      const t = op('timer')
      const pre = num('preset')
      const en = bit(t, 'EN')
      let dn = bit(t, 'DN')
      let a = acc(t, 'ACC')
      let tt: boolean
      if (!ctx.write) {
        ctx.trace.set(b.id, { in: pin, out: pin, active: en || dn })
        return pin
      }
      if (b.instr === 'TOF') {
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
        if (b.instr === 'TON') {
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
      const c = op('counter')
      const pre = num('preset')
      const edgeBit = b.instr === 'CTU' ? 'CU' : 'CD'
      const was = bit(c, edgeBit)
      if (!ctx.write) {
        ctx.trace.set(b.id, { in: pin, out: pin, active: bit(c, 'DN') })
        return pin
      }
      let a = acc(c, 'ACC')
      if (pin && !was) {
        if (b.instr === 'CTU') {
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
      const t = op('target')
      if (ctx.read(member(t, 'ACC')) === undefined) {
        ctx.errors.set(b.id, t.trim() ? `${t} is not a timer or counter` : 'missing operand')
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
      const s = op('storage')
      const stored = readBool(ctx, s, b.id)
      out = pin && !stored
      if (ctx.write) set(s, pin)
      active = out
      break
    }
    case 'OSR': {
      const s = op('storage')
      const stored = readBool(ctx, s, b.id)
      if (ctx.write) {
        set(op('output'), pin && !stored)
        set(s, pin)
      }
      active = readBool(ctx, op('output'), b.id)
      break
    }
    case 'EQU':
    case 'NEQ':
    case 'GRT':
    case 'GEQ':
    case 'LES':
    case 'LEQ': {
      const a = num('a')
      const c = num('b')
      const cmp = {
        EQU: a === c, NEQ: a !== c, GRT: a > c, GEQ: a >= c, LES: a < c, LEQ: a <= c,
      }[b.instr]
      out = pin && cmp
      active = cmp
      break
    }
    case 'ADD':
    case 'SUB':
    case 'MUL':
    case 'DIV': {
      const a = num('a')
      const c = num('b')
      if (pin && ctx.write) {
        if (b.instr === 'DIV' && c === 0) ctx.errors.set(b.id, 'divide by zero')
        else {
          const r = { ADD: a + c, SUB: a - c, MUL: a * c, DIV: a / c }[b.instr]
          set(op('dest'), r)
        }
      }
      break
    }
    case 'MOV': {
      const v = num('source')
      if (pin && ctx.write) set(op('dest'), v)
      break
    }
    case 'CPT': {
      if (pin && ctx.write) {
        try {
          set(op('dest'), evaluateExpression(op('expr'), ctx.read))
        } catch (err) {
          ctx.errors.set(b.id, err instanceof Error ? err.message : String(err))
        }
      }
      break
    }
    case 'JSR': {
      if (pin && ctx.jsr && !ctx.jsr(op('routine').trim())) {
        ctx.errors.set(b.id, `JSR: no routine ${op('routine')}`)
      }
      break
    }
  }
  ctx.trace.set(b.id, { in: pin, out, active })
  return out
}

/** Executes one rung and returns its rung-condition-out. Shared by the simulator and pure traces. */
export function executeRung(body: Series, ctx: ExecContext): boolean {
  return execSeries(body, true, ctx)
}

/**
 * Power flow from given values, writing nothing (ONLINE mode). Contacts read
 * the values, coils show their tag, compares are evaluated, timers and
 * counters show their EN/DN (or DN) bits.
 */
export function traceRoutine(routine: Routine, read: (ref: string) => Scalar | undefined): Trace {
  const ctx: ExecContext = { read, dtMs: 0, trace: new Map(), errors: new Map(), mem: new Map() }
  for (const r of routine.rungs) executeRung(r.body, ctx)
  return ctx.trace
}

const MAX_JSR_DEPTH = 8

/** True when `b` is an input instruction (it gates power). Used by tests and the UI. */
export function isConditionBox(b: Box): boolean {
  return BOX_SPECS[b.instr].role === 'input'
}

export class Simulator {
  readonly image: ProcessImage
  readonly tags: TagStore
  trace: Trace = new Map()
  errors = new Map<string, string>()
  scanCount = 0
  lastScanMs = 0
  /** The last scan ran out of budget and was cut short. */
  overrun = false
  overrunCount = 0
  private project: Project
  private mem = new Map<string, Scalar>()

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
    const { tasks, programs } = this.project
    if (tasks.length === 0) return programs
    const out: Program[] = []
    for (const t of tasks) {
      for (const name of t.programs) {
        const p = programs.find((x) => x.name === name)
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
    const errors = new Map<string, string>()
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
  snapshotTrace(): Array<[string, number]> {
    return packTrace(this.trace)
  }

  private runPrograms(dtMs: number, trace: Trace, errors: Map<string, string>, check: () => void): void {
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
        if (routine.kind !== 'ladder') return
        for (const r of routine.rungs) {
          check()
          executeRung(r.body, ctx)
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
      const main = program.routines.find((r) => r.name === program.main)
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

export function packTrace(trace: Trace): Array<[string, number]> {
  const out: Array<[string, number]> = []
  for (const [id, t] of trace) out.push([id, (t.in ? 1 : 0) | (t.out ? 2 : 0) | (t.active ? 4 : 0)])
  return out
}

export function unpackTrace(packed: Array<[string, number]>): Trace {
  const out: Trace = new Map()
  for (const [id, bits] of packed) out.set(id, { in: (bits & 1) !== 0, out: (bits & 2) !== 0, active: (bits & 4) !== 0 })
  return out
}
