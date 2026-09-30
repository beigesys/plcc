// SPDX-License-Identifier: MPL-2.0
//
// Live values for Simulate and Online modes. The simulator runs in a worker
// and posts throttled snapshots; they are merged here at most once per
// animation frame. Online values are decoded from the device's process image.

import { create } from 'zustand'
import { unpackTrace, type Trace } from '@/engine'
import { parseAddress, ProcessImage, type Project } from '@/model'
import type { FromWorker, Scalar, SimStats, Snapshot, TagValue, ToWorker } from '@/runtime/messages'

export type LiveSource = 'none' | 'sim' | 'online'
export type OnlineState = 'disconnected' | 'connecting' | 'online' | 'error'

export interface LiveState {
  source: LiveSource
  trace: Trace | null
  /** Lower-case tag name to value. Online: only tags with an %I/%Q/%M address. */
  values: Record<string, TagValue>
  forces: Record<string, Scalar>
  errors: Record<string, string>
  image: { I: Uint8Array; Q: Uint8Array; M: Uint8Array } | null
  stats: SimStats | null
  overrun: boolean
  online: {
    state: OnlineState
    error?: string
    fault?: string
    lastUpdate: number
    transport: 'webserial' | 'fake' | null
    /** Bytes of %M the device reports. */
    mKnown: number
  }
}

const initial: LiveState = {
  source: 'none',
  trace: null,
  values: {},
  forces: {},
  errors: {},
  image: null,
  stats: null,
  overrun: false,
  online: { state: 'disconnected', lastUpdate: 0, transport: null, mKnown: 0 },
}

export const useLive = create<LiveState>(() => ({ ...initial }))

/** Reads `Tag`, `Tag.MEMBER` or `Tag.<bit>` from a value map. */
export function readRef(values: Record<string, TagValue>, ref: string): Scalar | undefined {
  const parts = ref.trim().split('.')
  const base = values[parts[0].toLowerCase()]
  if (base === undefined) return undefined
  if (parts.length === 1) return typeof base === 'object' ? undefined : base
  const member = parts[1]
  if (typeof base === 'object') {
    const key = Object.keys(base).find((k) => k.toLowerCase() === member.toLowerCase())
    return key === undefined ? undefined : base[key]
  }
  if (/^\d+$/.test(member) && typeof base === 'number') return ((base >> Number(member)) & 1) === 1
  return undefined
}

export function formatValue(v: TagValue | undefined, type = ''): string {
  if (v === undefined) return '—'
  if (typeof v === 'boolean') return v ? '1' : '0'
  if (typeof v === 'number') {
    if (/REAL/i.test(type)) return Number.isInteger(v) ? v.toFixed(1) : v.toPrecision(6).replace(/\.?0+$/, '')
    return String(v)
  }
  const pre = v.PRE ?? v.pre
  const acc = v.ACC ?? v.acc
  const dn = v.DN ?? v.dn
  if (acc !== undefined) return `ACC ${Number(acc)} / ${Number(pre)}${dn ? '  DN' : ''}`
  return JSON.stringify(v)
}

// ---------------------------------------------------------------- simulator

let worker: Worker | null = null
let pending: Snapshot | null = null
let frame = 0

function mergeSnapshot() {
  frame = 0
  const s = pending
  pending = null
  if (!s) return
  const prev = useLive.getState()
  let values = prev.values
  if (Object.keys(s.values).length || s.removed.length) {
    values = { ...prev.values, ...s.values }
    for (const k of s.removed) delete values[k]
  }
  useLive.setState({
    source: 'sim',
    trace: unpackTrace(s.trace),
    values,
    forces: s.forces,
    errors: s.errors,
    image: s.image,
    stats: s.stats,
    overrun: s.overrun,
  })
}

function onWorkerMessage(e: MessageEvent<FromWorker>) {
  const m = e.data
  if (m.type === 'error') {
    console.error('simulator:', m.message)
    return
  }
  // Values are diffs: fold an unrendered snapshot's changes into the next one.
  if (pending) m.values = { ...pending.values, ...m.values }
  pending = m
  if (!frame) frame = requestAnimationFrame(mergeSnapshot)
}

export function simSend(msg: ToWorker) {
  worker?.postMessage(msg)
}

export function startSimulator(project: Project, profileId: string, periodMs = 10) {
  stopSimulator()
  worker = new Worker(new URL('../runtime/sim.worker.ts', import.meta.url), { type: 'module' })
  worker.onmessage = onWorkerMessage
  worker.onerror = (e) => console.error('simulator worker failed:', e.message)
  useLive.setState({ ...initial, source: 'sim' })
  simSend({ type: 'init', project, profileId, periodMs })
  simSend({ type: 'start' })
}

export function stopSimulator() {
  if (worker) {
    worker.terminate()
    worker = null
  }
  if (frame) cancelAnimationFrame(frame)
  frame = 0
  pending = null
  if (useLive.getState().source === 'sim') useLive.setState({ ...initial })
}

export function isSimulatorRunning() {
  return worker !== null
}

// ---------------------------------------------------------------- online values

/** Values of every addressed tag, decoded from a device image. */
export function valuesFromImage(project: Project, img: ProcessImage): Record<string, TagValue> {
  const out: Record<string, TagValue> = {}
  for (const t of project.tags) {
    if (!t.address) continue
    const a = parseAddress(t.address)
    if (!a || !img.inRange(a)) continue
    out[t.name.toLowerCase()] = img.read(a, t.type)
  }
  return out
}
