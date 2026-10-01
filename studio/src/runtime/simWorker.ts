// SPDX-License-Identifier: MPL-2.0
//
// The simulator worker's dispatcher: the preview engine (SimHost) until a
// compiled program arrives, then plcc's program (PlcHost). No Worker API
// here, so it runs under vitest too.

import type { LoopClock } from './loop'
import { PlcHost } from './plcHost'
import { SimHost } from './simHost'
import type { FromWorker, ToWorker } from './messages'

export class SimWorker {
  private readonly preview: SimHost
  private plc: PlcHost | null = null
  private readonly post: (m: FromWorker) => void
  private readonly clock?: LoopClock

  constructor(post: (m: FromWorker) => void, opts: { clock?: LoopClock } = {}) {
    this.post = post
    this.clock = opts.clock
    this.preview = new SimHost(post, { clock: opts.clock })
  }

  get engine() {
    return this.plc ? 'plcc' : 'preview'
  }

  async handle(msg: ToWorker): Promise<void> {
    try {
      await this.apply(msg)
    } catch (e) {
      this.post({ type: 'error', message: e instanceof Error ? e.message : String(e) })
    }
  }

  private async apply(msg: ToWorker) {
    const plc = this.plc
    switch (msg.type) {
      case 'init':
        plc?.stop()
        this.plc = null
        this.preview.handle(msg)
        return
      case 'program': {
        this.preview.handle({ type: 'stop' })
        const next = plc ?? new PlcHost(this.post, { clock: this.clock })
        await next.load({ module: msg.module, symbols: msg.symbols, project: msg.project })
        this.plc = next
        return
      }
      case 'project':
        if (plc) plc.setProject(msg.project)
        else this.preview.handle(msg)
        return
      case 'write':
        if (plc) {
          plc.write(msg.ref, msg.value)
          plc.flush()
        } else this.preview.handle(msg)
        return
      case 'force':
        if (plc) plc.force(msg.tag, msg.value)
        else this.preview.handle(msg)
        return
      case 'image':
        if (plc) {
          plc.writeImage(msg.address, msg.value)
          plc.flush()
        } else this.preview.handle(msg)
        return
      case 'reset':
        if (plc) plc.restart()
        else this.preview.handle(msg)
        return
      case 'stop':
        plc?.stop()
        this.preview.handle(msg)
        return
      default:
        if (!plc) this.preview.handle(msg)
    }
  }
}
