// SPDX-License-Identifier: MPL-2.0
// Messages between the main thread and the simulator worker.

import type { Device } from '@/devices/manifest'
import type { Project } from '@/model'

export type Scalar = boolean | number
export type TagValue = Scalar | Record<string, Scalar>

export type ToWorker =
  | { type: 'init'; project: Project; device: Device; periodMs: number }
  | { type: 'project'; project: Project }
  | { type: 'start' }
  | { type: 'stop' }
  | { type: 'period'; periodMs: number }
  | { type: 'reset' }
  | { type: 'write'; ref: string; value: Scalar }
  | { type: 'force'; tag: string; value: Scalar | null }
  | { type: 'image'; address: string; value: Scalar }

export interface SimStats {
  running: boolean
  periodMs: number
  scans: number
  lastScanMs: number
  maxScanMs: number
  jitterMs: number
  overruns: number
  skipped: number
}

export interface Snapshot {
  type: 'snapshot'
  /** Full trace as [element id, bits] with in=1, out=2, active=4. */
  trace: [number, number][]
  /** Changed tag values only (lower-case tag name). */
  values: Record<string, TagValue>
  /** Tags that disappeared. */
  removed: string[]
  forces: Record<string, Scalar>
  errors: Record<string, string>
  image: { I: Uint8Array; Q: Uint8Array; M: Uint8Array }
  stats: SimStats
  /** Set when the last scan hit the per-scan time budget. */
  overrun: boolean
}

export type FromWorker = Snapshot | { type: 'error'; message: string }
