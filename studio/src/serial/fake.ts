// SPDX-License-Identifier: MPL-2.0
//
// A stand-in for a device running a plcc runtime: answers the console
// commands its manifest lists exactly like runtime.ino (`info`, `img` with
// `console.img_m_bytes` of %M, `mw`). Used by tests and by the "Demo device"
// connection in the UI.

import { catalogEntry } from '@/devices/catalog'
import type { Device } from '@/devices/manifest'
import { parseAddress, ProcessImage } from '@/model'
import { formatImgLine, formatInfoLine, type ProgramStatus } from './protocol'
import { Listeners } from './transport'
import type { SerialTransport } from './transport'

function defaultDevice(): Device {
  const e = catalogEntry('arduino-opta')
  if (!e) throw new Error('the Opta manifest is missing from the build')
  return e.device
}

export class FakeConsoleTransport implements SerialTransport {
  readonly I: Uint8Array
  readonly Q: Uint8Array
  readonly M: Uint8Array
  readonly device: Device
  /** Every line written to the device, for tests. */
  readonly sent: string[] = []
  /**
   * The `info` reply. Default: what a runtime built for `device` prints; set a
   * string to impersonate another device, or null for a runtime without `info`.
   */
  get infoReply(): string | null {
    return this.info_ === 'auto' ? formatInfoLine(this.device, this.program ?? undefined) : this.info_
  }
  set infoReply(v: string | null) {
    this.info_ = v
  }
  private info_: string | null | 'auto'
  /** A program-image runtime's program (manifests whose console has `prog`). */
  program: ProgramStatus | null
  private open_ = false
  private muted = false
  private lines = new Listeners<[string]>()
  private closes = new Listeners<[string | undefined]>()
  private readonly mReport: number

  constructor(device: Device = defaultDevice()) {
    this.device = device
    this.I = new Uint8Array(device.target.image.I)
    this.Q = new Uint8Array(device.target.image.Q)
    this.M = new Uint8Array(device.target.image.M)
    this.mReport = device.console?.img_m_bytes ?? this.M.length
    this.program = device.console?.commands.includes('prog') ? { state: 'empty', image: null, reason: 'empty slot' } : null
    this.info_ = device.console?.commands.includes('info') ? 'auto' : null
  }

  private get commands() {
    return this.device.console?.commands ?? []
  }

  /** As if a program image were downloaded and started. */
  loadProgram(image: { build: string; size: number; crc: string; version: number }): void {
    this.program = { state: 'run', image }
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
    const info = this.infoReply
    if (line === 'info' && info !== null) return info
    const prog = this.program
    if (prog && this.commands.includes('prog')) {
      if (line === 'prog') {
        return prog.image
          ? JSON.stringify({ valid: true, why: 'ok', format: 1, target: this.device.device.id, version: prog.image.version, abi: this.device.target.runtime.abi, services: 1, size: prog.image.size, build: prog.image.build, body_crc: prog.image.crc })
          : JSON.stringify({ valid: false, why: prog.reason ?? 'empty slot' })
      }
      if (line === 'stop') {
        if (prog.state === 'run') prog.state = 'stop'
        this.Q.fill(0)
        return `ok ${prog.state}`
      }
      if (line === 'run') {
        if (!prog.image) return `error: ${prog.reason ?? 'empty slot'}`
        prog.state = 'run'
        delete prog.fault
        return 'ok run'
      }
    }
    return `? commands: ${info !== null ? 'info | ' : ''}img | mw <n> <value>${prog ? ' | prog | stop | run' : ''}`
  }

  /** Writes an input (or any) address, like wiring the terminal. */
  setAddress(address: string, value: boolean | number): void {
    const a = parseAddress(address)
    if (a) ProcessImage.over({ I: this.I, Q: this.Q, M: this.M }).write(a, value)
  }

  /** Sets digital input I<n> (1-based) of the Opta map, %IX0.(n-1). */
  setInput(n: number, on: boolean): void {
    this.setAddress(`%IX0.${n - 1}`, on)
  }

  /** Sets the raw analog reading of I<n> (1-based), 0..4095, as %IW<n>. */
  setAnalog(n: number, raw: number): void {
    this.setAddress(`%IW${n}`, raw)
  }

  setButton(pressed: boolean): void {
    this.setAddress('%IX1.0', pressed)
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

/** The Opta-flavoured name the tests and UI grew up with. */
export { FakeConsoleTransport as FakeOptaTransport }
