// SPDX-License-Identifier: MPL-2.0
//
// The page's handle on plcc's front end. Calls go to a worker
// (frontend.worker.ts), started on first use; tests swap in a direct
// implementation with `useFrontendDirect`.

import type {
  CheckRequest, CheckResult, ConvertRequest, ConvertResult, DeviceResult, LadderDialect, LadderRef,
} from '@plcc/plcc-wasm'
import type { FrontendOp, FrontendRequest, FrontendResponse } from './frontendProtocol'

export type { CheckResult, ConvertResult, Diagnostic, DeviceResult, LadderRef } from '@plcc/plcc-wasm'

export interface Frontend {
  check(req: CheckRequest): Promise<CheckResult>
  convert(req: ConvertRequest): Promise<ConvertResult>
  loadDevice(text: string, file?: string): Promise<DeviceResult>
  validateDevice(text: string, file?: string): Promise<DeviceResult>
  locateLadder(model: string, line: number, col: number): Promise<LadderRef | null>
  catalog(dialect: LadderDialect): Promise<unknown>
  version(): Promise<string>
}

let direct: Frontend | null = null
let worker: Worker | null = null
let nextId = 1
const pending = new Map<number, { resolve(v: unknown): void; reject(e: Error): void }>()

/** Tests: run the front end in this thread. */
export function useFrontendDirect(impl: Frontend | null) {
  direct = impl
}

function start(): Worker {
  const w = new Worker(new URL('./frontend.worker.ts', import.meta.url), { type: 'module' })
  w.onmessage = (e: MessageEvent<FrontendResponse>) => {
    const r = e.data
    const p = pending.get(r.id)
    pending.delete(r.id)
    if (r.poisoned && worker === w) {
      // An internal compiler error aborted the instance: the next call gets a fresh one.
      w.terminate()
      worker = null
    }
    if (!p) return
    if (r.ok) p.resolve(r.result)
    else p.reject(new Error(r.error))
  }
  w.onerror = (e) => {
    for (const [id, p] of pending) {
      p.reject(new Error(`plcc front end failed to load: ${e.message}`))
      pending.delete(id)
    }
    if (worker === w) worker = null
  }
  return w
}

function call<T>(op: FrontendOp, ...args: unknown[]): Promise<T> {
  if (direct) return (direct[op] as (...a: unknown[]) => Promise<T>)(...args)
  worker ??= start()
  const id = nextId++
  return new Promise<T>((resolve, reject) => {
    pending.set(id, { resolve: resolve as (v: unknown) => void, reject })
    worker!.postMessage({ id, op, args } satisfies FrontendRequest)
  })
}

export const plcc: Frontend = {
  check: (req) => call('check', req),
  convert: (req) => call('convert', req),
  loadDevice: (text, file) => call('loadDevice', text, file),
  validateDevice: (text, file) => call('validateDevice', text, file),
  locateLadder: (model, line, col) => call('locateLadder', model, line, col),
  catalog: (d) => call('catalog', d),
  version: () => call('version'),
}
