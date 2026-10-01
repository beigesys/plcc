// SPDX-License-Identifier: MPL-2.0
//
// Tag values for the preview simulator. Tags with a direct address (`%IX0.0`,
// `%MW3`) live in the process image, so the image is the one source of truth
// for them; every other tag lives in this store. TIMER and COUNTER tags are
// Logix-style structures (`RunTimer.DN`, `Parts.ACC`).

import { ProcessImage, parseAddress } from '@/model'
import type { Address, Project, Tag } from '@/model'

export type Scalar = boolean | number

export interface TimerValue {
  PRE: number
  ACC: number
  EN: boolean
  TT: boolean
  DN: boolean
}

export interface CounterValue {
  PRE: number
  ACC: number
  CU: boolean
  CD: boolean
  DN: boolean
  OV: boolean
  UN: boolean
}

export type StructValue = Record<string, Scalar>

const newTimer = (): TimerValue => ({ PRE: 0, ACC: 0, EN: false, TT: false, DN: false })
const newCounter = (): CounterValue => ({ PRE: 0, ACC: 0, CU: false, CD: false, DN: false, OV: false, UN: false })

/** Integer types: [bits, signed]. */
const INT_TYPES: Record<string, [number, boolean]> = {
  SINT: [8, true], INT: [16, true], DINT: [32, true], LINT: [64, true],
  USINT: [8, false], UINT: [16, false], UDINT: [32, false], ULINT: [64, false],
  BYTE: [8, false], WORD: [16, false], DWORD: [32, false], LWORD: [64, false],
  TIME: [32, true], LTIME: [64, true],
}

export function toNumber(v: Scalar): number {
  return typeof v === 'boolean' ? (v ? 1 : 0) : v
}

export function toBool(v: Scalar): boolean {
  return typeof v === 'boolean' ? v : v !== 0
}

/** Converts a value to what a tag of `type` can hold: BOOL coerces, integers truncate and wrap. */
export function coerce(type: string, v: Scalar): Scalar {
  const t = type.toUpperCase()
  if (t === 'BOOL') return toBool(v)
  const n = toNumber(v)
  if (t === 'REAL' || t === 'LREAL') return n
  const spec = INT_TYPES[t]
  if (!spec) return Number.isFinite(n) ? n : 0
  if (!Number.isFinite(n)) return 0
  const i = Math.trunc(n)
  const [bits, signed] = spec
  if (bits === 64) return i
  if (bits === 32) return signed ? i | 0 : i >>> 0
  const mod = 2 ** bits
  let w = ((i % mod) + mod) % mod
  if (signed && w >= mod / 2) w -= mod
  return w
}

const DURATION_PART = /(\d+(?:\.\d+)?)(ms|us|ns|d|h|m|s)/gi
const DURATION_MS: Record<string, number> = { d: 86400000, h: 3600000, m: 60000, s: 1000, ms: 1, us: 0.001, ns: 0.000001 }

/**
 * Parses an ST/Logix literal: `5000`, `-2`, `1.5`, `1e3`, `16#FF`, `2#1010`,
 * `TRUE`, `T#5s`, `T#250ms`, `INT#5`. Durations are in ms. Undefined if not a literal.
 */
export function parseLiteral(text: string): Scalar | undefined {
  const t = text.trim().replace(/_/g, '')
  if (t === '') return undefined
  if (/^TRUE$/i.test(t)) return true
  if (/^FALSE$/i.test(t)) return false
  if (/^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/.test(t)) return Number(t)
  const based = /^([+-]?)(2|8|16)#([0-9A-F]+)$/i.exec(t)
  if (based) {
    const n = parseInt(based[3], Number(based[2]))
    return based[1] === '-' ? -n : n
  }
  const dur = /^[+-]?(T|TIME|LT|LTIME)#(-?)(.+)$/i.exec(t)
  if (dur) {
    const body = dur[3].toLowerCase()
    let total = 0
    let consumed = 0
    for (const m of body.matchAll(DURATION_PART)) {
      total += Number(m[1]) * DURATION_MS[m[2].toLowerCase()]
      consumed += m[0].length
    }
    if (consumed !== body.length || consumed === 0) return undefined
    return dur[2] === '-' || t.startsWith('-') ? -total : total
  }
  const typed = /^([A-Z]+)#(.+)$/i.exec(t)
  if (typed) {
    const v = parseLiteral(typed[2])
    return v === undefined ? undefined : coerce(typed[1], v)
  }
  return undefined
}

interface Entry {
  tag: Tag
  addr?: Address
  /** Scalar tags without an address, and structures. Unused for addressed tags. */
  value: Scalar | StructValue
}

function isStructType(type: string): boolean {
  const t = type.toUpperCase()
  return t === 'TIMER' || t === 'COUNTER'
}

function initialValue(tag: Tag): Scalar | StructValue {
  const t = tag.data_type.toUpperCase()
  if (t === 'TIMER') return { ...newTimer(), ...structInit(tag.initial) }
  if (t === 'COUNTER') return { ...newCounter(), ...structInit(tag.initial) }
  const lit = parseLiteral(tag.initial ?? '')
  return coerce(t, lit ?? 0)
}

/** `(PRE := 150, ACC := 0)`, an L5X structure's initial members. */
function structInit(text: string | undefined): StructValue {
  const out: StructValue = {}
  if (!text) return out
  for (const m of text.replace(/^\(|\)$/g, '').split(',')) {
    const [k, v] = m.split(':=').map((x) => x.trim())
    const lit = v === undefined ? undefined : parseLiteral(v)
    if (k && lit !== undefined) out[k.toUpperCase()] = k.toUpperCase() === 'PRE' || k.toUpperCase() === 'ACC' ? toNumber(lit) : toBool(lit)
  }
  return out
}

/** Every tag the preview simulator knows: controller tags, then program tags. */
export function projectTags(project: Pick<Project, 'globals' | 'pous'>): Tag[] {
  return [...project.globals, ...project.pous.flatMap((p) => p.variables)]
}

export class TagStore {
  readonly image: ProcessImage
  readonly forces = new Map<string, Scalar>()
  private entries = new Map<string, Entry>()

  constructor(project: Project, image: ProcessImage) {
    this.image = image
    this.sync(project)
  }

  /** Adds new tags, drops removed ones, re-inits tags whose type or address changed; keeps the rest. */
  sync(project: Project): void {
    const next = new Map<string, Entry>()
    for (const tag of projectTags(project)) {
      const key = tag.name.toLowerCase()
      const old = this.entries.get(key)
      const addr = tag.address ? parseAddress(tag.address) : undefined
      if (old && old.tag.data_type.toUpperCase() === tag.data_type.toUpperCase() && old.tag.address === tag.address) {
        next.set(key, { ...old, tag })
        continue
      }
      const entry: Entry = { tag, addr, value: initialValue(tag) }
      next.set(key, entry)
      if (addr && addr.area !== 'I' && !isStructType(tag.data_type)) {
        const v = entry.value as Scalar
        if (toNumber(v) !== 0) this.image.write(addr, v, tag.data_type)
      }
    }
    this.entries = next
    for (const k of [...this.forces.keys()]) if (!this.entries.has(k.split('.')[0])) this.forces.delete(k)
    this.applyForces()
  }

  /** Cold restart: every tag back to its initial value. The caller clears the image. */
  reset(): void {
    for (const e of this.entries.values()) {
      e.value = initialValue(e.tag)
      if (e.addr && e.addr.area !== 'I' && !isStructType(e.tag.data_type)) {
        this.image.write(e.addr, e.value as Scalar, e.tag.data_type)
      }
    }
    this.applyForces()
  }

  has(name: string): boolean {
    return this.entries.has(name.toLowerCase())
  }

  tag(name: string): Tag | undefined {
    return this.entries.get(name.toLowerCase())?.tag
  }

  private raw(e: Entry): Scalar | StructValue {
    if (e.addr && !isStructType(e.tag.data_type)) return this.image.read(e.addr, e.tag.data_type)
    return e.value
  }

  /** Reads `Tag`, `Tag.MEMBER` or `Tag.<bit>`. Undefined when the tag or member does not exist. */
  read(ref: string): Scalar | undefined {
    const key = ref.trim().toLowerCase()
    const forced = this.forces.get(key)
    if (forced !== undefined) return forced
    const [base, ...rest] = key.split('.')
    const e = this.entries.get(base)
    if (!e) return undefined
    if (rest.length === 0) {
      const f = this.forces.get(base)
      if (f !== undefined) return f
      const v = this.raw(e)
      return typeof v === 'object' ? undefined : v
    }
    if (rest.length !== 1) return undefined
    const member = rest[0]
    const v = this.forces.get(base) ?? this.raw(e)
    if (typeof v === 'object') {
      const m = v[member.toUpperCase()]
      return m
    }
    if (/^\d+$/.test(member)) {
      const bit = Number(member)
      if (bit > 63) return undefined
      return Math.floor(toNumber(v) / 2 ** bit) % 2 === 1
    }
    return undefined
  }

  /** Writes a tag, member or bit. Returns false if the ref does not exist. Forced refs are left alone. */
  write(ref: string, value: Scalar): boolean {
    const key = ref.trim().toLowerCase()
    const [base, ...rest] = key.split('.')
    const e = this.entries.get(base)
    if (!e) return false
    if (this.forces.has(key) || this.forces.has(base)) return true
    if (rest.length === 0) {
      if (isStructType(e.tag.data_type)) return false
      const v = coerce(e.tag.data_type, value)
      if (e.addr) this.image.write(e.addr, v, e.tag.data_type)
      else e.value = v
      return true
    }
    if (rest.length !== 1) return false
    const member = rest[0]
    if (typeof e.value === 'object' && isStructType(e.tag.data_type)) {
      const m = member.toUpperCase()
      if (!(m in e.value)) return false
      e.value[m] = m === 'PRE' || m === 'ACC' ? coerce('DINT', value) : toBool(value)
      return true
    }
    if (/^\d+$/.test(member)) {
      const bit = Number(member)
      const cur = this.raw(e)
      if (typeof cur === 'object' || bit > 31) return false
      const n = toNumber(cur)
      const mask = 2 ** bit
      const has = Math.floor(n / mask) % 2 === 1
      const on = toBool(value)
      const next = on === has ? n : on ? n + mask : n - mask
      return this.write(base, coerce(e.tag.data_type, next))
    }
    return false
  }

  /** A literal, or the value of a tag reference. Undefined if neither. */
  evalOperand(text: string): Scalar | undefined {
    return parseLiteral(text) ?? this.read(text)
  }

  force(ref: string, value: Scalar): void {
    const key = ref.trim().toLowerCase()
    const e = this.entries.get(key.split('.')[0])
    this.forces.set(key, e && !key.includes('.') ? coerce(e.tag.data_type, value) : value)
    this.applyForces()
  }

  unforce(ref: string): void {
    this.forces.delete(ref.trim().toLowerCase())
  }

  /** Writes forced values of addressed tags into the image, so the image shows them. */
  applyForces(): void {
    for (const [key, v] of this.forces) {
      if (key.includes('.')) continue
      const e = this.entries.get(key)
      if (e?.addr && !isStructType(e.tag.data_type)) this.image.write(e.addr, v, e.tag.data_type)
    }
  }

  /** Plain values for display, keyed by tag name as written in the project. */
  snapshot(): Record<string, Scalar | StructValue> {
    const out: Record<string, Scalar | StructValue> = {}
    for (const [key, e] of this.entries) {
      const f = this.forces.get(key)
      const v = f ?? this.raw(e)
      out[e.tag.name] = typeof v === 'object' ? { ...v } : v
    }
    return out
  }
}
