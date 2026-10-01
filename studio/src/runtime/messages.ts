// SPDX-License-Identifier: MPL-2.0
// Messages between the main thread and the simulator worker.

import type { SymbolTable } from '@plcc/plc-wasm'
import type { Device } from '@/devices/manifest'
import type { Project } from '@/model'

export type Scalar = boolean | number
export type TagValue = Scalar | Record<string, Scalar>

export type ToWorker =
  /** Start the preview simulator (the TypeScript engine) on the project. */
  | { type: 'init'; project: Project; device: Device; periodMs: number }
  /** Run plcc's compiled program instead (cold start). */
  | { type: 'program'; module: Uint8Array; symbols: SymbolTable; project: Project }
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

/** Which engine runs the project: plcc's compiled program, or the TypeScript preview. */
export type Engine = 'plcc' | 'preview'

export interface Snapshot {
  type: 'snapshot'
  engine: Engine
  /** Full trace as [element id, bits] with in=1, out=2, active=4. */
  trace: [number, number][]
  /** Changed tag values only (lower-case tag name). */
  values: Record<string, TagValue>
  /** Tags that disappeared. */
  removed: string[]
  forces: Record<string, Scalar>
  /** Preview engine: problems by element id. */
  errors: Record<string, string>
  /** plcc program: the fault that stopped it (`file:line:col: POU` in `where`). */
  fault: { code: number; where: string; message: string } | null
  image: { I: Uint8Array; Q: Uint8Array; M: Uint8Array }
  stats: SimStats
  /** Set when the last scan hit the per-scan time budget. */
  overrun: boolean
}

export type FromWorker =
  | Snapshot
  | { type: 'error'; message: string }
  | { type: 'print'; message: string }
  | { type: 'fault'; code: number; where: string; message: string }
  | { type: 'loaded'; engine: Engine; tasks: { name: string; intervalMs: number }[] }
