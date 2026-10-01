// SPDX-License-Identifier: MPL-2.0
//
// The page's handle on plcc's compiler (@plcc/plcc-compiler-wasm). It is
// big, so nothing loads it until a build is needed (Simulate, Download); the
// worker then downloads it once (progress in useCompiler), keeps it in Cache
// Storage keyed by its SHA-256, and compiles off the UI thread.

import { create } from 'zustand'
import type { CompileRequest, CompileResult, LoadProgress } from '@plcc/plcc-compiler-wasm'
import type { CompilerEvent, CompilerRequest } from '@plcc/plcc-compiler-wasm/worker'

export type { CompileRequest, CompileResult } from '@plcc/plcc-compiler-wasm'

export type CompilerStatus = 'idle' | 'loading' | 'ready' | 'unavailable' | 'error'

export interface CompilerState {
  status: CompilerStatus
  progress: LoadProgress | null
  /** Where the module came from, how long loading took, its size. */
  loaded: { source: 'network' | 'cache'; ms: number; bytes: number; version: string } | null
  error: string | null
  /** A build is running. */
  busy: boolean
}

export const useCompiler = create<CompilerState>(() => ({ status: 'idle', progress: null, loaded: null, error: null, busy: false }))

/** Where the build put plcc-compiler.json and plcc-compiler.wasm.gz. */
export function compilerBaseUrl(): string {
  const base = new URL(import.meta.env.BASE_URL, location.href)
  return new URL('plcc-compiler/', base).href
}

let worker: Worker | null = null
let nextId = 1
const pending = new Map<number, { resolve(v: unknown): void; reject(e: Error): void }>()

function start(): Worker {
  const w = new Worker(new URL('./compiler.worker.ts', import.meta.url), { type: 'module' })
  w.onmessage = (e: MessageEvent<CompilerEvent>) => {
    const m = e.data
    if (m.type === 'progress') {
      useCompiler.setState({ progress: m.progress, status: 'loading' })
      return
    }
    const p = pending.get(m.id)
    pending.delete(m.id)
    if (m.type === 'loaded') {
      useCompiler.setState({ status: 'ready', loaded: { source: m.source, ms: m.ms, bytes: m.bytes, version: m.version }, progress: null, error: null })
      p?.resolve(undefined)
    } else if (m.type === 'result') {
      useCompiler.setState({ status: 'ready', progress: null })
      p?.resolve(m.result)
    } else if (m.type === 'error') {
      const unavailable = /not available here|404/.test(m.message)
      useCompiler.setState({ status: unavailable ? 'unavailable' : 'error', error: m.message, progress: null })
      p?.reject(new Error(m.message))
    }
  }
  w.onerror = (e) => {
    useCompiler.setState({ status: 'error', error: e.message, progress: null })
    for (const [id, p] of pending) {
      p.reject(new Error(`the compiler worker failed: ${e.message}`))
      pending.delete(id)
    }
    worker = null
  }
  return w
}

type WithoutId<T> = T extends unknown ? Omit<T, 'id'> : never

function send<T>(req: WithoutId<CompilerRequest>): Promise<T> {
  worker ??= start()
  const id = nextId++
  return new Promise<T>((resolve, reject) => {
    pending.set(id, { resolve: resolve as (v: unknown) => void, reject })
    worker!.postMessage({ ...req, id } as CompilerRequest)
  })
}

/** Downloads (or takes from the cache) and compiles the compiler module. */
export function loadCompiler(): Promise<void> {
  const s = useCompiler.getState()
  if (s.status === 'ready') return Promise.resolve()
  useCompiler.setState({ status: 'loading', error: null })
  return send({ type: 'load', baseUrl: compilerBaseUrl() })
}

/** One build. Loads the compiler first if needed. */
export async function compile(request: CompileRequest): Promise<CompileResult> {
  useCompiler.setState({ busy: true })
  try {
    if (useCompiler.getState().status !== 'ready') await loadCompiler()
    return await send<CompileResult>({ type: 'compile', baseUrl: compilerBaseUrl(), request })
  } finally {
    useCompiler.setState({ busy: false })
  }
}
