// SPDX-License-Identifier: MPL-2.0
// Dedicated worker running the preview simulator off the UI thread.

import type { FromWorker, ToWorker } from './messages'
import { SimHost } from './simHost'

const host = new SimHost((m: FromWorker) => postMessage(m))

self.onmessage = (e: MessageEvent<ToWorker>) => host.handle(e.data)
