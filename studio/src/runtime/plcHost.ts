// SPDX-License-Identifier: MPL-2.0
//
// Worker-side host for the real simulator: plcc's own wasm32 build of the
// project (compiled in the browser by @plcc/plcc-compiler-wasm), run by
// @plcc/plc-wasm with its task scheduler. It speaks the same protocol as the
// preview simulator's host (simHost.ts): commands in, throttled snapshots
// of changed values, power flow and stats out.
//
// Power flow is computed from the model and the program's live tag values
// (the same pure trace Online mode uses); plcc does not expose rung-internal
// power.

import { PlcModule, ScanCycle, type Clock, type PlcFault, type SymbolTable, type TagValue as PlcValue } from '@plcc/plc-wasm'
import { traceRoutine, coerce, packTrace, toBool, toNumber, type Trace } from '@/engine'
import { parseAddress, ProcessImage, type Project, type Tag } from '@/model'
import { projectTags } from '@/engine/values'
import { FixedPeriodLoop, realClock, type LoopClock } from './loop'
import type { FromWorker, Scalar, SimStats, TagValue } from './messages'

export interface PlcProgram {
  module: Uint8Array
  symbols: SymbolTable
  project: Project
}

const STRUCT_MEMBERS = ['PRE', 'ACC', 'EN', 'TT', 'DN', 'CU', 'CD', 'OV', 'UN']

function num(v: PlcValue): Scalar {
  if (typeof v === 'boolean') return v
  if (typeof v === 'bigint') return Number(v)
  if (typeof v === 'number') return v
  return 0
}

interface TagInfo {
  tag: Tag
  /** Symbol path of the variable (`GLOBAL.Motor`, `GLOBAL.MainProgram.Count`). */
  path?: string
  /** Members of a structure (TIMER, COUNTER): name → path. */
  members?: Map<string, string>
  addr?: ReturnType<typeof parseAddress>
}

export class PlcHost {
  private plc: PlcModule | null = null
  private cycle: ScanCycle | null = null
  private loop: FixedPeriodLoop | null = null
  private project: Project | null = null
  private tags = new Map<string, TagInfo>()
  private forces = new Map<string, Scalar>()
  private sent = new Map<string, string>()
  private lastPost = -Infinity
  private maxScanMs = 0
  private lastScanMs = 0
  private passes = 0
  private periodMs = 1
  private fault: { code: number; where: string; message: string } | null = null
  private prints: string[] = []
  private readonly post: (m: FromWorker) => void
  private readonly clock: LoopClock
  private readonly postEveryMs: number

  constructor(post: (m: FromWorker) => void, opts: { clock?: LoopClock; postEveryMs?: number } = {}) {
    this.post = post
    this.clock = opts.clock ?? realClock
    this.postEveryMs = opts.postEveryMs ?? 33
  }

  /** Loads a compiled program (cold start) and starts it. */
  async load(p: PlcProgram): Promise<void> {
    this.stop()
    const plcClock: Clock = { nowNs: () => BigInt(Math.round(this.clock.now() * 1e6)) }
    this.plc = await PlcModule.load(p.module as BufferSource, {
      clock: plcClock,
      symbols: p.symbols,
      onPrint: (m) => {
        this.prints.push(m)
        this.post({ type: 'print', message: m })
      },
    })
    this.project = p.project
    this.indexTags(p.symbols)
    this.fault = null
    this.cycle = new ScanCycle(this.plc, plcClock, {
      latchInputs: () => this.applyForces(),
      flushOutputs: () => this.applyForces(),
      onFault: (f: PlcFault) => {
        this.fault = { code: f.code, where: f.where, message: f.message }
        this.post({ type: 'fault', code: f.code, where: f.where, message: f.message })
        this.flush()
      },
    })
    this.cycle.start()
    // Poll at 1 ms; the scheduler runs each task when it is due.
    const intervals = this.plc.tasks.map((t) => Number(t.intervalNs) / 1e6).filter((x) => x > 0)
    this.periodMs = Math.max(1, Math.min(10, ...(intervals.length ? intervals : [10])))
    this.loop = new FixedPeriodLoop(1, () => this.step(), this.clock, { maxCatchUp: 4 })
    this.sent.clear()
    this.maxScanMs = 0
    this.passes = 0
    this.loop.start()
    this.flush()
  }

  /** The project changed but the program did not (yet): power flow and tags follow. */
  setProject(p: Project): void {
    this.project = p
  }

  private indexTags(symbols: SymbolTable) {
    this.tags.clear()
    const byPath = new Map(symbols.variables.map((v) => [v.path.toUpperCase(), v.path]))
    const find = (...cands: string[]) => cands.map((c) => byPath.get(c.toUpperCase())).find((x) => x !== undefined)
    for (const pou of this.project?.pous ?? []) {
      for (const t of pou.variables) this.addTag(t, find, [`GLOBAL.${pou.name}.${t.name}`, `${pou.name}.${t.name}`], byPath)
    }
    for (const t of this.project?.globals ?? []) this.addTag(t, find, [`GLOBAL.${t.name}`], byPath)
  }

  private addTag(t: Tag, find: (...c: string[]) => string | undefined, cands: string[], byPath: Map<string, string>) {
    const info: TagInfo = { tag: t, path: find(...cands) }
    const type = t.data_type.toUpperCase()
    if (type === 'TIMER' || type === 'COUNTER') {
      info.members = new Map()
      for (const m of STRUCT_MEMBERS) {
        const p = cands.map((c) => byPath.get(`${c}.${m}`.toUpperCase())).find((x) => x !== undefined)
        if (p) info.members.set(m, p)
      }
    } else if (t.address) info.addr = parseAddress(t.address)
    this.tags.set(t.name.toLowerCase(), info)
  }

  private image(): ProcessImage | null {
    const plc = this.plc
    if (!plc) return null
    return ProcessImage.over({ I: plc.image('I'), Q: plc.image('Q'), M: plc.image('M') })
  }

  /** Reads `Tag`, `Tag.MEMBER` or `Tag.<bit>`. */
  read(ref: string): Scalar | undefined {
    const key = ref.trim().toLowerCase()
    const f = this.forces.get(key)
    if (f !== undefined) return f
    const [base, member] = key.split('.')
    const info = this.tags.get(base)
    if (!info || !this.plc) return undefined
    try {
      if (member !== undefined) {
        if (info.members) {
          const p = info.members.get(member.toUpperCase())
          return p ? num(this.plc.read(p)) : undefined
        }
        if (/^\d+$/.test(member)) {
          const v = this.read(base)
          return v === undefined ? undefined : Math.floor(toNumber(v) / 2 ** Number(member)) % 2 === 1
        }
        return undefined
      }
      if (info.members) return undefined
      if (info.addr) {
        const img = this.image()
        if (img?.inRange(info.addr)) return img.read(info.addr, info.tag.data_type)
      }
      return info.path ? num(this.plc.read(info.path)) : undefined
    } catch {
      return undefined
    }
  }

  /** Writes a tag, member or bit; addressed tags through the process image. */
  write(ref: string, value: Scalar): boolean {
    const key = ref.trim().toLowerCase()
    const [base, member] = key.split('.')
    const info = this.tags.get(base)
    if (!info || !this.plc) return false
    try {
      if (member !== undefined) {
        if (info.members) {
          const p = info.members.get(member.toUpperCase())
          if (!p) return false
          const isBool = !['PRE', 'ACC'].includes(member.toUpperCase())
          this.plc.write(p, isBool ? toBool(value) : Math.trunc(toNumber(value)))
          return true
        }
        if (/^\d+$/.test(member)) {
          const cur = toNumber(this.read(base) ?? 0)
          const mask = 2 ** Number(member)
          const has = Math.floor(cur / mask) % 2 === 1
          const on = toBool(value)
          return this.write(base, on === has ? cur : on ? cur + mask : cur - mask)
        }
        return false
      }
      const v = coerce(info.tag.data_type, value)
      if (info.addr) {
        const img = this.image()
        if (img?.inRange(info.addr)) {
          img.write(info.addr, v, info.tag.data_type)
          // %Q / %M bound tags: the program reads the tag, copied from %M before
          // it runs (%M) or written by it (%Q); write the tag too.
          if (info.path && info.addr.area !== 'I') this.plc.write(info.path, typeof v === 'boolean' ? v : v)
          return true
        }
      }
      if (!info.path) return false
      this.plc.write(info.path, info.tag.data_type.toUpperCase() === 'BOOL' ? toBool(v) : v)
      return true
    } catch {
      return false
    }
  }

  writeImage(address: string, value: Scalar) {
    const a = parseAddress(address)
    const img = this.image()
    if (a && img?.inRange(a)) img.write(a, value)
  }

  force(ref: string, value: Scalar | null) {
    const key = ref.trim().toLowerCase()
    if (value === null) this.forces.delete(key)
    else {
      this.forces.set(key, value)
      this.write(key, value)
    }
    this.flush()
  }

  private applyForces() {
    for (const [k, v] of this.forces) this.write(k, v)
  }

  private step() {
    const cycle = this.cycle
    if (!cycle || cycle.state !== 'running') return
    const t0 = this.clock.now()
    const ran = cycle.step()
    if (ran.length) {
      this.lastScanMs = this.clock.now() - t0
      this.maxScanMs = Math.max(this.maxScanMs, this.lastScanMs)
      this.passes++
    }
    if (this.clock.now() - this.lastPost >= this.postEveryMs) this.flush()
  }

  restart() {
    if (!this.cycle) return
    this.fault = null
    this.cycle.restart()
    this.applyForces()
    this.flush()
  }

  stop() {
    this.loop?.stop()
    this.loop = null
    this.cycle?.stop()
  }

  stats(): SimStats {
    const ls = this.loop?.stats
    return {
      running: this.cycle?.state === 'running',
      periodMs: this.periodMs,
      scans: this.passes,
      lastScanMs: this.lastScanMs,
      maxScanMs: this.maxScanMs,
      jitterMs: ls?.jitterMs ?? 0,
      overruns: 0,
      skipped: ls?.skipped ?? 0,
    }
  }

  private trace(): Trace {
    const out: Trace = new Map()
    for (const p of this.project?.pous ?? []) {
      for (const r of p.routines) for (const [id, t] of traceRoutine(r, (ref) => this.read(ref))) out.set(id, t)
    }
    return out
  }

  private values(): Record<string, TagValue> {
    const out: Record<string, TagValue> = {}
    for (const t of this.project ? projectTags(this.project) : []) {
      const info = this.tags.get(t.name.toLowerCase())
      if (!info) continue
      if (info.members) {
        const o: Record<string, Scalar> = {}
        for (const m of info.members.keys()) {
          const v = this.read(`${t.name}.${m}`)
          if (v !== undefined) o[m] = v
        }
        out[t.name] = o
      } else {
        const v = this.read(t.name)
        if (v !== undefined) out[t.name] = v
      }
    }
    return out
  }

  /** Posts a snapshot now. */
  flush() {
    if (!this.plc) return
    this.lastPost = this.clock.now()
    const snap = this.values()
    const values: Record<string, TagValue> = {}
    for (const [name, v] of Object.entries(snap)) {
      const k = name.toLowerCase()
      const j = JSON.stringify(v)
      if (this.sent.get(k) !== j) {
        values[k] = v
        this.sent.set(k, j)
      }
    }
    const lower = new Set(Object.keys(snap).map((n) => n.toLowerCase()))
    const removed: string[] = []
    for (const k of this.sent.keys()) {
      if (!lower.has(k)) {
        removed.push(k)
        this.sent.delete(k)
      }
    }
    const forces: Record<string, Scalar> = {}
    for (const [k, v] of this.forces) forces[k] = v
    this.post({
      type: 'snapshot',
      engine: 'plcc',
      trace: packTrace(this.trace()),
      values,
      removed,
      forces,
      errors: {},
      fault: this.fault,
      image: { I: this.plc.image('I').slice(), Q: this.plc.image('Q').slice(), M: this.plc.image('M').slice() },
      stats: this.stats(),
      overrun: false,
    })
  }
}
