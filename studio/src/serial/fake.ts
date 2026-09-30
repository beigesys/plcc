// SPDX-License-Identifier: MPL-2.0
//
// A stand-in for an Opta running the plcc runtime: answers the USB console
// commands exactly like runtime.ino. Used by tests and by the "Fake Opta"
// connection in the UI.

import { ARDUINO_OPTA } from '@/devices/profiles'
import type { DeviceProfile } from '@/devices/profiles'
import { formatImgLine } from './protocol'
import { Listeners } from './transport'
import type { SerialTransport } from './transport'

export class FakeOptaTransport implements SerialTransport {
  readonly I: Uint8Array
  readonly Q: Uint8Array
  readonly M: Uint8Array
  /** Every line written to the device, for tests. */
  readonly sent: string[] = []
  private open_ = false
  private muted = false
  private lines = new Listeners<[string]>()
  private closes = new Listeners<[string | undefined]>()
  private readonly mReport: number

  constructor(profile: DeviceProfile = ARDUINO_OPTA) {
    this.I = new Uint8Array(profile.imageSizes.I)
    this.Q = new Uint8Array(profile.imageSizes.Q)
    this.M = new Uint8Array(profile.imageSizes.M)
    this.mReport = profile.transport?.imgMBytes ?? 8
  }

  get isOpen(): boolean {
    return this.open_
  }

  async open(): Promise<void> {
    this.open_ = true
  }

  async close(): Promise<void> {
    if (!this.open_) return
    this.open_ = false
    this.closes.emit(undefined)
  }

  onLine(cb: (line: string) => void): () => void {
    return this.lines.add(cb)
  }

  onClose(cb: (reason?: string) => void): () => void {
    return this.closes.add(cb)
  }

  async writeLine(text: string): Promise<void> {
    if (!this.open_) throw new Error('port is closed')
    this.sent.push(text)
    if (this.muted) return
    const reply = this.handle(text)
    setTimeout(() => {
      if (this.open_ && !this.muted) this.lines.emit(reply)
    }, 0)
  }

  private handle(line: string): string {
    const mw = /^mw (\d+) (-?\d+)$/.exec(line)
    if (mw && Number(mw[1]) < this.M.length / 2) {
      const n = Number(mw[1])
      const w = Number(mw[2]) & 0xffff
      this.M[2 * n] = w & 0xff
      this.M[2 * n + 1] = w >> 8
      return `ok %MW${n} := ${w}`
    }
    if (line === 'img') {
      return formatImgLine({ I: this.I, Q: this.Q, M: this.M.subarray(0, this.mReport) })
    }
    return '? commands: mw <n> <value> | img'
  }

  /** Sets digital input I<n> (1-based), like wiring 24 V to it. */
  setInput(n: number, on: boolean): void {
    const bit = n - 1
    if (on) this.I[0] |= 1 << bit
    else this.I[0] &= ~(1 << bit) & 0xff
  }

  /** Sets the raw analog reading of I<n> (1-based), 0..4095, as %IW<n>. */
  setAnalog(n: number, raw: number): void {
    this.I[2 * n] = raw & 0xff
    this.I[2 * n + 1] = (raw >> 8) & 0xff
  }

  setButton(pressed: boolean): void {
    this.I[1] = pressed ? 1 : 0
  }

  /** Stops answering (a hung or busy device). */
  mute(on: boolean): void {
    this.muted = on
  }

  /** The cable was pulled. */
  simulateDisconnect(reason = 'device disconnected'): void {
    if (!this.open_) return
    this.open_ = false
    this.closes.emit(reason)
  }

  /** Emits an arbitrary line, e.g. a fault message. */
  emit(line: string): void {
    this.lines.emit(line)
  }
}
