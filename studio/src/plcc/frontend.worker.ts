// SPDX-License-Identifier: MPL-2.0
// plcc's front end (@plcc/plcc-wasm, ~2 MB of wasm) in a worker: check,
// convert, device manifests. A Rust panic poisons the instance; the client
// then starts a fresh worker.

import { catalog, check, convert, load, loadDevice, locateLadder, poisonedReason, validateDevice, version } from '@plcc/plcc-wasm'
import type { FrontendOp, FrontendRequest, FrontendResponse } from './frontendProtocol'

const ops: Record<FrontendOp, (...args: never[]) => Promise<unknown>> = {
  check: check as never,
  convert: convert as never,
  loadDevice: loadDevice as never,
  validateDevice: validateDevice as never,
  locateLadder: locateLadder as never,
  catalog: catalog as never,
  version: version as never,
}

self.onmessage = async (e: MessageEvent<FrontendRequest>) => {
  const { id, op, args } = e.data
  let res: FrontendResponse
  try {
    await load()
    const result = await (ops[op] as (...a: unknown[]) => Promise<unknown>)(...args)
    res = { id, ok: true, result, poisoned: poisonedReason() }
  } catch (err) {
    res = { id, ok: false, error: err instanceof Error ? err.message : String(err), poisoned: poisonedReason() }
  }
  postMessage(res)
}
