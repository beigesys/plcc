// SPDX-License-Identifier: MPL-2.0
//
// Device manifests (docs/device-manifest.md) in the browser: parse the TOML,
// validate it, and expand `repeat` groups into the same JSON shape as
// `plcc_device::Device`. crates/plcc-device is the reference implementation;
// manifest.test.ts checks this loader against its golden expansions
// (crates/plcc-device/tests/data/*.expanded.json).
//
// A manifest is untrusted data: nothing here executes or fetches anything it
// names.

import { parse as parseToml, TomlError } from 'smol-toml'
// The leaf module, not '@/model': the model's demo project imports the catalog.
import { formatAddress, parseAddress, type Address } from '@/model/address'

export const SCHEMA_VERSION = 1

export type PointDir = 'in' | 'out' | 'mem'
export type PointKind = 'digital' | 'analog' | 'register'
export type ConsoleCommand = 'info' | 'img' | 'mw' | 'prog' | 'stop' | 'run'
const CONSOLE_COMMANDS: readonly ConsoleCommand[] = ['info', 'img', 'mw', 'prog', 'stop', 'run']

export interface IoPoint {
  /** Stable id within the manifest. */
  id: string
  /** Terminal name printed on the device. */
  terminal: string
  label: string
  /** Group heading in the I/O lists. */
  group: string
  dir: PointDir
  kind: PointKind
  /** IEC type. */
  type: string
  /** Canonical direct address. */
  address: string
  /** Raw range for analog points. */
  range?: [number, number]
  /** Engineering value at the ends of `range`. */
  eng?: [number, number]
  units?: string
  description?: string
}

export interface UsbId {
  vid: number
  pid: number
}

export interface DeviceInfo {
  id: string
  name: string
  vendor: string
  description: string
  version: number
  schema: number
  source?: string
  sha256?: string
}

export interface Target {
  triple: string
  cpu?: string
  features?: string[]
  float_abi?: 'soft' | 'softfp' | 'hard'
  runtime: { kind: string; abi: number }
  image: { I: number; Q: number; M: number }
}

export interface Flash {
  method: 'dfuse'
  usb: UsbId[]
  alt: number
  layout?: string
  address: number
  max_size: number
  leave: boolean
  reboot: 'none' | '1200-baud-touch'
  runtime_usb?: UsbId[]
  protected?: { start: number; size: number; reason?: string }[]
  /** The program slot (docs/program-image.md): programs downloaded on their own. */
  program?: ProgramSlot
}

/** `[flash.program]`: where program images go, and the RAM they get. */
export interface ProgramSlot {
  format: number
  address: number
  max_size: number
  ram: { start: number; size: number }
  services: number
}

/** The program image format plcc writes. */
export const PROGRAM_IMAGE_FORMAT = 1

export interface Console {
  transport: 'webserial'
  baud: number
  commands: ConsoleCommand[]
  img_format: 'hex-areas'
  img_m_bytes: number
}

export interface ModbusMap {
  table: 'coils' | 'discrete' | 'input' | 'holding'
  start: number
  count: number
  address: string
}

export interface Modbus {
  rtu?: {
    interface?: string
    unit: number
    baud: number
    data_bits: number
    parity: 'none' | 'even' | 'odd'
    stop_bits: number
  }
  map: ModbusMap[]
}

/** An expanded, validated manifest. */
export interface Device {
  device: DeviceInfo
  target: Target
  flash?: Flash
  console?: Console
  modbus?: Modbus
  io: IoPoint[]
}

export interface ManifestDiagnostic {
  severity: 'error' | 'warning'
  /** `io[3].address`; empty for syntax errors. */
  path: string
  message: string
  /** 1-based, when known. */
  line?: number
  col?: number
}

export interface LoadedManifest {
  device?: Device
  diagnostics: ManifestDiagnostic[]
}

// ---------------------------------------------------------------- expressions

/** Expands `{expr}` in a template (integer arithmetic on `n`; `{{` `}}` literal). */
export function expandTemplate(template: string, n: number): string {
  let out = ''
  for (let i = 0; i < template.length; i++) {
    const c = template[i]
    if (c === '{' && template[i + 1] === '{') {
      out += '{'
      i++
    } else if (c === '}' && template[i + 1] === '}') {
      out += '}'
      i++
    } else if (c === '{') {
      const end = template.indexOf('}', i + 1)
      if (end < 0) throw new Error(`\`${template}\`: \`{\` without a closing \`}\``)
      const expr = template.slice(i + 1, end)
      try {
        out += String(evalExpr(expr, n))
      } catch (e) {
        throw new Error(`\`${template}\`: in \`{${expr}}\`: ${e instanceof Error ? e.message : String(e)}`, { cause: e })
      }
      i = end
    } else if (c === '}') {
      throw new Error(`\`${template}\`: \`}\` without an opening \`{\` (write \`}}\` for a brace)`)
    } else out += c
  }
  return out
}

export function hasPlaceholder(s: string): boolean {
  return s.replaceAll('{{', '').includes('{')
}

type Tok = { num: number } | { n: true } | { op: string }

export function evalExpr(expr: string, n: number): number {
  const toks: Tok[] = []
  for (let i = 0; i < expr.length; ) {
    const c = expr[i]
    if (/\s/.test(c)) i++
    else if (/\d/.test(c)) {
      let j = i
      while (j < expr.length && /\d/.test(expr[j])) j++
      toks.push({ num: Number(expr.slice(i, j)) })
      i = j
    } else if (c === 'n' && !/[A-Za-z0-9_]/.test(expr[i + 1] ?? '')) {
      toks.push({ n: true })
      i++
    } else if ('+-*/%()'.includes(c)) {
      toks.push({ op: c })
      i++
    } else if (/[A-Za-z_]/.test(c)) throw new Error('the only variable is `n`')
    else throw new Error(`unexpected \`${c}\``)
  }
  if (toks.length === 0) throw new Error('empty expression')
  let pos = 0
  const op = () => {
    const t = toks[pos]
    return t && 'op' in t ? t.op : undefined
  }
  const show = (t: Tok) => ('num' in t ? String(t.num) : 'n' in t ? 'n' : t.op)
  const sum = (): number => {
    let v = product()
    for (let o = op(); o === '+' || o === '-'; o = op()) {
      pos++
      const r = product()
      v = o === '+' ? v + r : v - r
    }
    return v
  }
  const product = (): number => {
    let v = unary()
    for (let o = op(); o === '*' || o === '/' || o === '%'; o = op()) {
      pos++
      const r = unary()
      if (o === '*') v *= r
      else if (r === 0) throw new Error('division by zero')
      else v = o === '/' ? Math.trunc(v / r) : v % r
    }
    return v
  }
  const unary = (): number => {
    if (op() === '-') {
      pos++
      return -unary()
    }
    const t = toks[pos++]
    if (!t) throw new Error('expression ends early')
    if ('num' in t) return t.num
    if ('n' in t) return n
    if (t.op === '(') {
      const v = sum()
      if (op() !== ')') throw new Error('missing `)`')
      pos++
      return v
    }
    throw new Error(`unexpected \`${t.op}\``)
  }
  const v = sum()
  if (pos !== toks.length) throw new Error(`unexpected \`${show(toks[pos])}\``)
  return v + 0 // no -0
}

// ---------------------------------------------------------------- locating

/**
 * Line of a path such as `io[2].address` or `target.image.M`, found by
 * scanning table headers. Good enough for pointing at the offending line.
 */
export function locatePath(text: string, path: string): number | undefined {
  const segs = path.match(/[^.[\]]+|\[\d+\]/g) ?? []
  const lines = text.split('\n')
  // Header path of each line, with array indices.
  const counts = new Map<string, number>()
  let header = ''
  let best: number | undefined
  let bestLen = -1
  const want = segs.map((s) => (s.startsWith('[') ? s : `.${s}`)).join('').slice(1)
  for (let i = 0; i < lines.length; i++) {
    const l = lines[i].trim()
    let m = /^\[\[\s*([^\]]+?)\s*\]\]/.exec(l)
    if (m) {
      const k = m[1]
      const idx = counts.get(k) ?? 0
      counts.set(k, idx + 1)
      header = `${k}[${idx}]`
    } else if ((m = /^\[\s*([^\]]+?)\s*\]/.exec(l))) {
      header = m[1]
    } else {
      const km = /^([A-Za-z0-9_-]+)\s*=/.exec(l)
      if (!km) continue
      const full = header ? `${header}.${km[1]}` : km[1]
      if (want === full || want.startsWith(`${full}.`) || want.startsWith(`${full}[`)) {
        if (full.length > bestLen) {
          best = i + 1
          bestLen = full.length
        }
      }
      continue
    }
    if ((want === header || want.startsWith(`${header}.`) || want.startsWith(`${header}[`)) && header.length > bestLen) {
      best = i + 1
      bestLen = header.length
    }
  }
  return best
}

// ---------------------------------------------------------------- validation

type Obj = Record<string, unknown>
const isObj = (v: unknown): v is Obj => typeof v === 'object' && v !== null && !Array.isArray(v)

class Checker {
  diags: ManifestDiagnostic[] = []
  private readonly text?: string
  constructor(text?: string) {
    this.text = text
  }

  push(severity: 'error' | 'warning', path: string, message: string) {
    const line = this.text !== undefined ? locatePath(this.text, path) : undefined
    this.diags.push({ severity, path, message, line })
  }
  error(path: string, message: string) {
    this.push('error', path, message)
  }
  warn(path: string, message: string) {
    this.push('warning', path, message)
  }
  get failed() {
    return this.diags.some((d) => d.severity === 'error')
  }

  keys(o: Obj, path: string, allowed: string[]) {
    for (const k of Object.keys(o)) {
      if (!allowed.includes(k)) this.error(path ? `${path}.${k}` : k, `unknown field \`${k}\`, expected one of ${allowed.map((a) => `\`${a}\``).join(', ')}`)
    }
  }
  table(o: Obj, key: string, path: string, required = true): Obj | undefined {
    const v = o[key]
    if (v === undefined) {
      if (required) this.error(path, `missing field \`${key}\``)
      return undefined
    }
    if (!isObj(v)) {
      this.error(`${path}`, 'must be a table')
      return undefined
    }
    return v
  }
  str(o: Obj, key: string, path: string, fallback?: string): string {
    const v = o[key]
    if (v === undefined && fallback !== undefined) return fallback
    if (typeof v !== 'string') {
      this.error(`${path}.${key}`, v === undefined ? `missing field \`${key}\`` : 'must be a string')
      return ''
    }
    return v
  }
  optStr(o: Obj, key: string, path: string): string | undefined {
    return o[key] === undefined ? undefined : this.str(o, key, path)
  }
  int(o: Obj, key: string, path: string, min: number, max: number, fallback?: number): number {
    const v = o[key]
    if (v === undefined && fallback !== undefined) return fallback
    if (typeof v !== 'number' && typeof v !== 'bigint') {
      this.error(`${path}.${key}`, v === undefined ? `missing field \`${key}\`` : 'must be an integer')
      return 0
    }
    const n = Number(v)
    if (!Number.isInteger(n) || n < min || n > max) {
      this.error(`${path}.${key}`, `${String(v)} is not an integer in ${min}..${max}`)
      return 0
    }
    return n
  }
  oneOf<T extends string>(o: Obj, key: string, path: string, values: readonly T[], fallback?: T): T {
    const v = o[key]
    if (v === undefined && fallback !== undefined) return fallback
    if (typeof v !== 'string' || !(values as readonly string[]).includes(v)) {
      this.error(`${path}.${key}`, `${v === undefined ? 'missing' : JSON.stringify(v)}: expected one of ${values.map((x) => `\`${x}\``).join(', ')}`)
      return values[0]
    }
    return v as T
  }
  usbList(o: Obj, key: string, path: string): UsbId[] {
    const v = o[key]
    if (v === undefined) return []
    if (!Array.isArray(v)) {
      this.error(`${path}.${key}`, 'must be an array of { vid, pid }')
      return []
    }
    return v.map((u, i) => {
      const p = `${path}.${key}[${i}]`
      if (!isObj(u)) {
        this.error(p, 'must be { vid, pid }')
        return { vid: 0, pid: 0 }
      }
      this.keys(u, p, ['vid', 'pid'])
      const id = { vid: this.int(u, 'vid', p, 0, 0xffff), pid: this.int(u, 'pid', p, 0, 0xffff) }
      if (id.vid === 0) this.error(`${p}.vid`, 'USB vendor id 0 is not valid')
      return id
    })
  }
}

const U32 = 0xffffffff
const hex8 = (n: number) => n.toString(16).toUpperCase().padStart(8, '0')
const TYPES: Record<Address['size'], string[]> = {
  X: ['BOOL'],
  B: ['BYTE', 'SINT', 'USINT', 'CHAR'],
  W: ['WORD', 'INT', 'UINT', 'WCHAR'],
  D: ['DWORD', 'DINT', 'UDINT', 'REAL'],
  L: ['LWORD', 'LINT', 'ULINT', 'LREAL'],
}

function lineCol(e: unknown): { line?: number; col?: number } {
  if (e instanceof TomlError) return { line: e.line, col: e.column }
  return {}
}

/** Parse, validate and expand a manifest. */
export function loadManifest(text: string): LoadedManifest {
  let doc: Obj
  try {
    doc = parseToml(text) as Obj
  } catch (e) {
    const msg = e instanceof Error ? e.message.split('\n')[0] : String(e)
    return { diagnostics: [{ severity: 'error', path: '', message: `invalid TOML: ${msg}`, ...lineCol(e) }] }
  }
  return validateManifest(doc, text)
}

/** Validate an already-parsed manifest object (`text` only locates lines). */
export function validateManifest(doc: Obj, text?: string): LoadedManifest {
  const c = new Checker(text)
  c.keys(doc, '', ['device', 'target', 'flash', 'console', 'modbus', 'io'])

  // [device]
  const d = c.table(doc, 'device', 'device') ?? {}
  c.keys(d, 'device', ['id', 'name', 'vendor', 'description', 'version', 'schema', 'source', 'sha256'])
  const info: DeviceInfo = {
    id: c.str(d, 'id', 'device'),
    name: c.str(d, 'name', 'device'),
    vendor: c.str(d, 'vendor', 'device'),
    description: c.str(d, 'description', 'device', ''),
    version: c.int(d, 'version', 'device', 0, U32),
    schema: c.int(d, 'schema', 'device', 0, U32),
  }
  const source = c.optStr(d, 'source', 'device')
  const sha256 = c.optStr(d, 'sha256', 'device')
  if (source !== undefined) info.source = source
  if (sha256 !== undefined) {
    info.sha256 = sha256
    if (!/^[0-9a-fA-F]{64}$/.test(sha256)) c.error('device.sha256', 'must be 64 hex digits')
  }
  if (!/^[a-z0-9]+(-[a-z0-9]+)*$/.test(info.id)) {
    c.error('device.id', `\`${info.id}\` is not a device id: use lowercase letters, digits and single \`-\``)
  }
  if (d.name !== undefined && !info.name.trim()) c.error('device.name', 'must not be empty')
  if (d.schema !== undefined && info.schema !== SCHEMA_VERSION) {
    c.error('device.schema', `manifest format ${info.schema} is not supported; this studio reads format ${SCHEMA_VERSION}`)
  }
  if (d.version !== undefined && info.version === 0) c.error('device.version', 'manifest versions start at 1')

  // [target]
  const t = c.table(doc, 'target', 'target') ?? {}
  c.keys(t, 'target', ['triple', 'cpu', 'features', 'float_abi', 'runtime', 'image'])
  const rt = c.table(t, 'runtime', 'target.runtime') ?? {}
  c.keys(rt, 'target.runtime', ['kind', 'abi'])
  const img = c.table(t, 'image', 'target.image') ?? {}
  c.keys(img, 'target.image', ['I', 'Q', 'M'])
  const MAX = 16 << 20
  const target: Target = {
    triple: c.str(t, 'triple', 'target'),
    runtime: { kind: c.str(rt, 'kind', 'target.runtime'), abi: c.int(rt, 'abi', 'target.runtime', 0, U32) },
    image: {
      I: c.int(img, 'I', 'target.image', 0, MAX),
      Q: c.int(img, 'Q', 'target.image', 0, MAX),
      M: c.int(img, 'M', 'target.image', 0, MAX),
    },
  }
  const cpu = c.optStr(t, 'cpu', 'target')
  if (cpu !== undefined) target.cpu = cpu
  if (t.features !== undefined) {
    if (!Array.isArray(t.features) || !t.features.every((f) => typeof f === 'string')) {
      c.error('target.features', 'must be an array of strings')
    } else {
      t.features.forEach((f: string, i) => {
        if (!/^[+-][^, ]+$/.test(f)) c.error(`target.features[${i}]`, `\`${f}\` is not a target feature: write \`+name\` or \`-name\`, one per string`)
      })
      if (t.features.length) target.features = [...(t.features as string[])]
    }
  }
  if (t.float_abi !== undefined) {
    const abi = c.oneOf(t, 'float_abi', 'target', ['soft', 'softfp', 'hard'] as const)
    target.float_abi = abi
    const arm = /^(arm|thumb)/.test(target.triple)
    const hf = target.triple.endsWith('hf')
    if (!arm) c.error('target.float_abi', `float_abi applies to ARM targets only, not \`${target.triple}\``)
    else if (abi === 'hard' && !hf) c.error('target.float_abi', `\`hard\` needs an \`eabihf\` triple, not \`${target.triple}\``)
    else if (abi !== 'hard' && hf) c.error('target.float_abi', `\`${abi}\` passes floats in integer registers; \`${target.triple}\` is a hard-float triple`)
  }
  if (t.triple !== undefined && (!target.triple.includes('-') || target.triple.trim() !== target.triple)) {
    c.error('target.triple', `\`${target.triple}\` is not an LLVM target triple`)
  }
  if (rt.kind !== undefined && !target.runtime.kind.trim()) c.error('target.runtime.kind', 'must not be empty')
  if (rt.abi !== undefined && target.runtime.abi === 0) c.error('target.runtime.abi', 'runtime ABI versions start at 1')

  const out: Device = { device: info, target, io: [] }

  // [flash]
  const f = c.table(doc, 'flash', 'flash', false)
  if (f) {
    const p = 'flash'
    c.keys(f, p, ['method', 'usb', 'alt', 'layout', 'address', 'max_size', 'leave', 'reboot', 'runtime_usb', 'protected', 'program'])
    const flash: Flash = {
      method: c.oneOf(f, 'method', p, ['dfuse'] as const),
      usb: c.usbList(f, 'usb', p),
      alt: c.int(f, 'alt', p, 0, 255, 0),
      address: c.int(f, 'address', p, 0, U32),
      max_size: c.int(f, 'max_size', p, 0, U32),
      leave: f.leave === undefined ? true : f.leave === true,
      reboot: c.oneOf(f, 'reboot', p, ['none', '1200-baud-touch'] as const, 'none'),
    }
    if (f.leave !== undefined && typeof f.leave !== 'boolean') c.error('flash.leave', 'must be true or false')
    const layout = c.optStr(f, 'layout', p)
    if (layout !== undefined) flash.layout = layout
    const rusb = c.usbList(f, 'runtime_usb', p)
    if (rusb.length) flash.runtime_usb = rusb
    if (f.usb === undefined) c.error('flash.usb', 'missing field `usb`')
    else if (flash.usb.length === 0) c.error('flash.usb', "list the bootloader's USB ids")
    if (f.protected !== undefined) {
      if (!Array.isArray(f.protected)) c.error('flash.protected', 'must be an array of { start, size, reason }')
      else {
        flash.protected = f.protected.map((r, i) => {
          const rp = `flash.protected[${i}]`
          const o = isObj(r) ? r : {}
          c.keys(o, rp, ['start', 'size', 'reason'])
          const reg: { start: number; size: number; reason?: string } = {
            start: c.int(o, 'start', rp, 0, U32),
            size: c.int(o, 'size', rp, 0, U32),
          }
          const reason = c.optStr(o, 'reason', rp)
          if (reason) reg.reason = reason
          if (reg.size === 0) c.error(`${rp}.size`, 'must be more than 0')
          return reg
        })
        if (flash.protected.length === 0) delete flash.protected
      }
    }
    const end = flash.address + flash.max_size
    if (flash.max_size === 0) c.error('flash.max_size', 'must be more than 0')
    if (end > 2 ** 32) c.error('flash.max_size', 'runs past the 32-bit address space')
    for (const r of flash.protected ?? []) {
      if (flash.address < r.start + r.size && r.start < end) {
        c.error('flash.address', `the application area overlaps protected region 0x${r.start.toString(16)}..0x${(r.start + r.size).toString(16)}${r.reason ? ` (${r.reason})` : ''}`)
      }
    }
    if (flash.reboot === '1200-baud-touch' && !flash.runtime_usb) {
      c.error('flash.runtime_usb', "`1200-baud-touch` needs the running application's USB ids (`runtime_usb`)")
    }
    // [flash.program] (crates/plcc-device/src/validate.rs, program_slot)
    const pg = c.table(f, 'program', 'flash.program', false)
    if (pg) {
      const pp = 'flash.program'
      c.keys(pg, pp, ['format', 'address', 'max_size', 'ram', 'services'])
      const ram = c.table(pg, 'ram', `${pp}.ram`) ?? {}
      c.keys(ram, `${pp}.ram`, ['start', 'size'])
      const slot: ProgramSlot = {
        format: c.int(pg, 'format', pp, 0, U32),
        address: c.int(pg, 'address', pp, 0, U32),
        max_size: c.int(pg, 'max_size', pp, 0, U32),
        ram: { start: c.int(ram, 'start', `${pp}.ram`, 0, U32), size: c.int(ram, 'size', `${pp}.ram`, 0, U32) },
        services: c.int(pg, 'services', pp, 0, U32),
      }
      if (slot.format !== PROGRAM_IMAGE_FORMAT) c.error(`${pp}.format`, `program image format ${slot.format} is not one plcc writes (${PROGRAM_IMAGE_FORMAT})`)
      if (slot.max_size < 256) c.error(`${pp}.max_size`, 'must be at least 256 bytes (the image header is 128)')
      const pend = slot.address + slot.max_size
      if (pend > 2 ** 32) c.error(`${pp}.max_size`, `0x${slot.address.toString(16).toUpperCase()} + 0x${slot.max_size.toString(16).toUpperCase()} runs past the 32-bit address space`)
      if (slot.address % 32 !== 0) c.error(`${pp}.address`, 'must be 32-byte aligned (a flash word)')
      if (flash.address < pend && slot.address < end) {
        c.error(`${pp}.address`, `the program slot 0x${hex8(slot.address)}..0x${hex8(pend)} overlaps the application (runtime) area 0x${hex8(flash.address)}..0x${hex8(end)}`)
      }
      for (const r of flash.protected ?? []) {
        if (slot.address < r.start + r.size && r.start < pend) {
          c.error(`${pp}.address`, `the program slot 0x${hex8(slot.address)}..0x${hex8(pend)} overlaps protected region 0x${hex8(r.start)}..0x${hex8(r.start + r.size)}${r.reason ? ` (${r.reason})` : ''}`)
        }
      }
      if (slot.ram.size < 64) c.error(`${pp}.ram.size`, 'must be at least 64 bytes')
      if (slot.ram.start + slot.ram.size > 2 ** 32) c.error(`${pp}.ram.size`, 'runs past the 32-bit address space')
      if (slot.ram.start % 8 !== 0) c.error(`${pp}.ram.start`, 'must be 8-byte aligned')
      if (slot.services < 3) c.error(`${pp}.services`, 'a runtime provides at least plcc_monotonic_ns, plcc_print and plcc_fault (3 services)')
      flash.program = slot
    }
    out.flash = flash
  }

  // [console]
  const co = c.table(doc, 'console', 'console', false)
  if (co) {
    const p = 'console'
    c.keys(co, p, ['transport', 'baud', 'commands', 'img_format', 'img_m_bytes'])
    const cmds: ConsoleCommand[] = []
    if (!Array.isArray(co.commands)) c.error('console.commands', `must be an array of ${CONSOLE_COMMANDS.map((x) => `\`${x}\``).join(', ')}`)
    else {
      co.commands.forEach((x, i) => {
        if (!(CONSOLE_COMMANDS as readonly unknown[]).includes(x)) c.error(`console.commands[${i}]`, `${JSON.stringify(x)}: expected one of ${CONSOLE_COMMANDS.map((y) => `\`${y}\``).join(', ')}`)
        else if (cmds.includes(x)) c.error(`console.commands[${i}]`, `\`${x}\` is listed twice`)
        else cmds.push(x)
      })
      if (co.commands.length === 0) c.error('console.commands', 'list the commands the runtime answers')
    }
    const con: Console = {
      transport: c.oneOf(co, 'transport', p, ['webserial'] as const),
      baud: c.int(co, 'baud', p, 1, U32),
      commands: cmds,
      img_format: c.oneOf(co, 'img_format', p, ['hex-areas'] as const),
      img_m_bytes: c.int(co, 'img_m_bytes', p, 0, U32),
    }
    if (con.img_m_bytes > target.image.M) c.error('console.img_m_bytes', `${con.img_m_bytes} bytes, but the %M area has only ${target.image.M}`)
    out.console = con
  }

  // [modbus]
  const mb = c.table(doc, 'modbus', 'modbus', false)
  if (mb) {
    c.keys(mb, 'modbus', ['rtu', 'map'])
    const modbus: Modbus = { map: [] }
    const r = c.table(mb, 'rtu', 'modbus.rtu', false)
    if (r) {
      const p = 'modbus.rtu'
      c.keys(r, p, ['interface', 'unit', 'baud', 'data_bits', 'parity', 'stop_bits'])
      modbus.rtu = {
        unit: c.int(r, 'unit', p, 0, 255),
        baud: c.int(r, 'baud', p, 0, U32),
        data_bits: c.int(r, 'data_bits', p, 0, 255, 8),
        parity: c.oneOf(r, 'parity', p, ['none', 'even', 'odd'] as const),
        stop_bits: c.int(r, 'stop_bits', p, 0, 255, 1),
      }
      const iface = c.optStr(r, 'interface', p)
      if (iface !== undefined) modbus.rtu = { interface: iface, ...modbus.rtu }
      if (modbus.rtu.unit < 1 || modbus.rtu.unit > 247) c.error('modbus.rtu.unit', `server address ${modbus.rtu.unit} is outside 1..247`)
    }
    const maps = mb.map === undefined ? [] : Array.isArray(mb.map) ? mb.map : (c.error('modbus.map', 'must be an array of tables'), [])
    maps.forEach((e, i) => {
      const p = `modbus.map[${i}]`
      const o = isObj(e) ? e : {}
      c.keys(o, p, ['table', 'start', 'count', 'address'])
      const m: ModbusMap = {
        table: c.oneOf(o, 'table', p, ['coils', 'discrete', 'input', 'holding'] as const),
        start: c.int(o, 'start', p, 0, 0xffff, 0),
        count: c.int(o, 'count', p, 0, 0xffff),
        address: c.str(o, 'address', p),
      }
      const a = parseAddress(m.address)
      if (!a) c.error(`${p}.address`, `\`${m.address}\` is not a direct address`)
      else {
        const bits = m.table === 'coils' || m.table === 'discrete'
        if ((a.size === 'X') !== bits || (!bits && a.size !== 'W')) c.error(`${p}.address`, `${m.table} need a %_${bits ? 'X' : 'W'} address`)
        if ((m.table === 'coils' || m.table === 'holding') && a.area === 'I') c.error(`${p}.address`, `${m.table} are written by the master; %I is written only by the runtime`)
        const endByte = bits ? Math.ceil((a.byte * 8 + a.bit + m.count) / 8) : a.byte + 2 * m.count
        if (endByte > target.image[a.area]) c.error(`${p}.count`, `${m.count} items from ${formatAddress(a)} run past the ${target.image[a.area]}-byte %${a.area} area`)
        m.address = formatAddress(a)
      }
      if (m.count === 0) c.error(`${p}.count`, 'must be more than 0')
      modbus.map.push(m)
    })
    out.modbus = modbus
  }

  // [[io]]
  const io = doc.io === undefined ? [] : Array.isArray(doc.io) ? doc.io : (c.error('io', 'must be an array of tables'), [])
  const ids = new Map<string, number>()
  const addrs = new Map<string, string>()
  io.forEach((raw, i) => {
    const p = `io[${i}]`
    if (!isObj(raw)) return c.error(p, 'must be a table')
    c.keys(raw, p, ['repeat', 'from', 'id', 'terminal', 'label', 'group', 'dir', 'kind', 'type', 'address', 'range', 'eng', 'units', 'description'])
    const fields = {
      id: c.str(raw, 'id', p),
      terminal: c.str(raw, 'terminal', p),
      label: c.str(raw, 'label', p),
      group: c.str(raw, 'group', p),
      address: c.str(raw, 'address', p),
    }
    const units = c.optStr(raw, 'units', p)
    const description = c.optStr(raw, 'description', p)
    const dir = c.oneOf(raw, 'dir', p, ['in', 'out', 'mem'] as const)
    const kind = c.oneOf(raw, 'kind', p, ['digital', 'analog', 'register'] as const)
    const type = c.str(raw, 'type', p).trim().toUpperCase()
    const pair = (key: 'range' | 'eng'): [number, number] | undefined => {
      const v = raw[key]
      if (v === undefined) return undefined
      if (!Array.isArray(v) || v.length !== 2 || !v.every((x) => typeof x === 'number')) {
        c.error(`${p}.${key}`, 'must be [min, max]')
        return undefined
      }
      return [Number(v[0]), Number(v[1])]
    }
    const range = pair('range')
    const eng = pair('eng')
    if (range && range[0] >= range[1]) c.error(`${p}.range`, `[${range[0]}, ${range[1]}]: the minimum must be below the maximum`)
    if (eng && !range) c.error(`${p}.eng`, '`eng` scales the raw `range`; give `range` too')
    if (eng && eng[0] === eng[1]) c.error(`${p}.eng`, 'must be two different numbers')
    const templated = [...Object.values(fields), units ?? '', description ?? ''].some(hasPlaceholder)
    let ns: number[]
    if (raw.repeat === undefined) {
      if (templated) return c.error(p, 'uses `{…}` but has no `repeat`')
      if (raw.from !== undefined) c.error(`${p}.from`, '`from` needs `repeat`')
      ns = [1]
    } else {
      const r = c.int(raw, 'repeat', p, 0, U32)
      if (r === 0) return c.error(`${p}.repeat`, 'must be at least 1')
      if (r > 4096) return c.error(`${p}.repeat`, `${r} points is more than the 4096 one entry may expand to`)
      if (r > 1 && !hasPlaceholder(fields.id)) return c.error(`${p}.id`, "a repeated entry's id must contain `{n}`")
      const from = c.int(raw, 'from', p, -(2 ** 53), 2 ** 53, 1)
      ns = Array.from({ length: r }, (_, k) => from + k)
    }
    for (const n of ns) {
      const at = raw.repeat !== undefined ? ` (n = ${n})` : ''
      let pt: IoPoint
      try {
        pt = {
          id: expandTemplate(fields.id, n),
          terminal: expandTemplate(fields.terminal, n),
          label: expandTemplate(fields.label, n),
          group: expandTemplate(fields.group, n),
          dir,
          kind,
          type,
          address: expandTemplate(fields.address, n),
        }
        if (range) pt.range = range
        if (eng) pt.eng = eng
        if (units !== undefined) pt.units = expandTemplate(units, n)
        if (description !== undefined) pt.description = expandTemplate(description, n)
      } catch (e) {
        c.error(p, e instanceof Error ? e.message : String(e))
        break
      }
      const first = ids.get(pt.id)
      if (first !== undefined) c.error(`${p}.id`, `id \`${pt.id}\`${at} is already used by io[${first}]`)
      else ids.set(pt.id, i)
      const a = parseAddress(pt.address)
      if (!a) {
        c.error(`${p}.address`, `\`${pt.address}\`${at} is not a direct address`)
        continue
      }
      const want = dir === 'in' ? 'I' : dir === 'out' ? 'Q' : 'M'
      if (a.area !== want) c.error(`${p}.address`, `\`${pt.address}\`${at} is in %${a.area}, but dir = "${dir}" points live in %${want}`)
      if (!TYPES[a.size].includes(type)) c.error(`${p}.type`, `${type} does not fit \`${pt.address}\`${at}; a %_${a.size} address holds ${TYPES[a.size].join(' / ')}`)
      if (kind === 'digital' && a.size !== 'X') c.error(`${p}.kind`, `a digital point needs a bit address (%_X), not \`${pt.address}\``)
      if (kind === 'analog' && a.size === 'X') c.error(`${p}.kind`, `an analog point needs a byte, word or larger address, not \`${pt.address}\``)
      const size = target.image[a.area]
      if (a.byte + a.width > size) c.error(`${p}.address`, `\`${pt.address}\`${at} ends at byte ${a.byte + a.width}, past the ${size}-byte %${a.area} area (target.image.${a.area})`)
      pt.address = formatAddress(a)
      const other = addrs.get(pt.address)
      if (other !== undefined) c.warn(`${p}.address`, `\`${pt.id}\` and \`${other}\` share ${pt.address}`)
      else addrs.set(pt.address, pt.id)
      out.io.push(pt)
    }
  })

  return c.failed ? { diagnostics: c.diags } : { device: out, diagnostics: c.diags }
}

/** `file:line: error: path: message` */
export function formatDiagnostic(d: ManifestDiagnostic, file = 'manifest'): string {
  const where = d.line ? `${file}:${d.line}${d.col ? `:${d.col}` : ''}` : file
  return `${where}: ${d.severity}: ${d.path ? `${d.path}: ` : ''}${d.message}`
}
