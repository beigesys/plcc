// SPDX-License-Identifier: MPL-2.0
// Dedicated worker running the simulator off the UI thread: the preview
// engine, then plcc's compiled program (simWorker.ts).

import type { FromWorker, ToWorker } from './messages'
import { SimWorker } from './simWorker'

const host = new SimWorker((m: FromWorker) => postMessage(m))

self.onmessage = (e: MessageEvent<ToWorker>) => void host.handle(e.data)
