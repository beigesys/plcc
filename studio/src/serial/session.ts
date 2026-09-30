// SPDX-License-Identifier: MPL-2.0
//
// An online connection to a device's console: identifies the device with
// `info`, polls `img`, decodes the process image, and writes %M words / bits
// with `mw`. Commands go out one at a time. The project's device manifest says
// which commands exist and how much of %M `img` reports.

import type { Device } from '@/devices/manifest'
import { ProcessImage, parseAddress } from '@/model'
import {
  bitForceWrite, checkIdentity, mwCommand, parseFaultLine, parseImgLine, parseInfoLine, parseMwAck,
  type DeviceIdentity, type IdentityCheck,
} from './protocol'
import type { ImgFrame } from './protocol'
import type { SerialTransport } from './transport'

export type SessionState = 'disconnected' | 'connecting' | 'online' | 'error'

export interface SessionOptions {
  /** Poll period for `img`, ms. */
  pollMs?: number
  /** A command with no answer after this long is a miss, ms. */
  timeoutMs?: number
  /** Consecutive missed polls before the state becomes 'error'. */
  maxMisses?: number
}

type Expect = 'img' | 'mw' | 'info'

interface Command {
  text: string
  expect: Expect
  resolve: (line: string) => void
  reject: (err: Error) => void
}

export class OnlineSession {
  readonly transport: SerialTransport
  readonly device: Device
  readonly image: ProcessImage
  /** What the device said about itself (`info`), once connected. */
  identity?: DeviceIdentity
  /** The identity compared with the manifest; `mismatch` entries are warnings to show. */
  identityCheck?: IdentityCheck
  state: SessionState = 'disconnected'
  error?: string
  /** Last `PLC STOP: ...` message from the device, if any. */
  fault?: string
  /** %M bytes as last reported by the device (the firmware sends only the first few). */
  knownM: Uint8Array = new Uint8Array(0)
  lastUpdate = 0
  rxCount = 0
  misses = 0
  /** Bumped on every notification; handy for React's useSyncExternalStore. */
  version = 0

  private readonly pollMs: number
  private readonly timeoutMs: number
  private readonly maxMisses: number
  private listeners = new Set<(s: OnlineSession) => void>()
  private queue: Command[] = []
  private current?: { cmd: Command; timer: ReturnType<typeof setTimeout> }
  private pollTimer?: ReturnType<typeof setInterval>
  private unsubs: (() => void)[] = []

  constructor(transport: SerialTransport, device: Device, opts: SessionOptions = {}) {
    this.transport = transport
    this.device = device
    this.image = new ProcessImage(device.target.image)
    this.pollMs = opts.pollMs ?? 100
    this.timeoutMs = opts.timeoutMs ?? 1000
    this.maxMisses = opts.maxMisses ?? 3
  }

  subscribe(listener: (s: OnlineSession) => void): () => void {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  private notify(): void {
    this.version++
    for (const l of [...this.listeners]) l(this)
  }

  private setState(state: SessionState, error?: string): void {
    if (this.state === state && this.error === error) return
    this.state = state
    this.error = error
    this.notify()
  }

  async connect(): Promise<void> {
    if (this.state === 'online' || this.state === 'connecting') return
    this.fault = undefined
    this.misses = 0
    this.setState('connecting')
    try {
      await this.transport.open()
    } catch (e) {
      this.setState('disconnected', e instanceof Error ? e.message : String(e))
      throw e
    }
    this.unsubs.push(
      this.transport.onLine((line) => this.onLine(line)),
      this.transport.onClose((reason) => this.onTransportClose(reason)),
    )
    if (this.device.console?.commands.includes('info')) {
      // Not awaited: polling starts at once; the identity arrives first
      // because commands go out in order.
      this.identify().catch(() => {})
    }
    this.pollTimer = setInterval(() => this.poll(), this.pollMs)
    this.poll()
  }

  /** Sends `info` and compares the answer with the manifest. */
  async identify(): Promise<DeviceIdentity | undefined> {
    try {
      const line = await this.send('info', 'info')
      const id = parseInfoLine(line)
      this.identity = id
      this.identityCheck = id
        ? checkIdentity(id, this.device)
        : { mismatch: [`The device's info reply is not understood: ${line.slice(0, 80)}`], notes: [] }
    } catch (e) {
      this.identity = undefined
      const why = e instanceof Error ? e.message : String(e)
      this.identityCheck = {
        mismatch: [],
        notes: [`The device did not identify itself (${why}); its runtime may predate the \`info\` command, so the manifest cannot be checked.`],
      }
    }
    this.notify()
    return this.identity
  }

  async disconnect(): Promise<void> {
    this.teardown('disconnected')
    this.setState('disconnected')
    try {
      await this.transport.close()
    } catch {
      // Already gone.
    }
  }

  private teardown(reason: string): void {
    if (this.pollTimer !== undefined) clearInterval(this.pollTimer)
    this.pollTimer = undefined
    for (const u of this.unsubs) u()
    this.unsubs = []
    if (this.current) {
      clearTimeout(this.current.timer)
      this.current.cmd.reject(new Error(reason))
      this.current = undefined
    }
    for (const c of this.queue) c.reject(new Error(reason))
    this.queue = []
  }

  private onTransportClose(reason?: string): void {
    if (this.state === 'disconnected') return
    this.teardown(reason ?? 'connection closed')
    this.setState('disconnected', reason ?? 'connection closed')
  }

  private poll(): void {
    const pending = this.current?.cmd.expect === 'img' || this.queue.some((c) => c.expect === 'img')
    if (pending) return
    this.send('img', 'img').then(
      () => {},
      () => {},
    )
  }

  private send(text: string, expect: Expect): Promise<string> {
    return new Promise<string>((resolve, reject) => {
      this.queue.push({ text, expect, resolve, reject })
      this.pump()
    })
  }

  private pump(): void {
    if (this.current || this.queue.length === 0 || !this.transport.isOpen) return
    const cmd = this.queue.shift()
    if (!cmd) return
    const timer = setTimeout(() => this.onTimeout(), this.timeoutMs)
    this.current = { cmd, timer }
    this.transport.writeLine(cmd.text).catch((e: unknown) => {
      this.finish(undefined, e instanceof Error ? e : new Error(String(e)))
    })
  }

  private finish(line: string | undefined, err?: Error): void {
    const cur = this.current
    if (!cur) return
    clearTimeout(cur.timer)
    this.current = undefined
    if (err) cur.cmd.reject(err)
    else cur.cmd.resolve(line ?? '')
    this.pump()
  }

  private onTimeout(): void {
    const cur = this.current
    if (!cur) return
    if (cur.cmd.expect === 'img') {
      this.misses++
      if (this.misses >= this.maxMisses) this.setState('error', 'device not responding')
      else this.notify()
    }
    this.finish(undefined, new Error(`no answer to "${cur.cmd.text}"`))
  }

  private onLine(line: string): void {
    if (this.current?.cmd.expect === 'info' && parseInfoLine(line)) {
      this.finish(line)
      return
    }
    const frame = parseImgLine(line)
    if (frame) {
      this.applyFrame(frame)
      if (this.current?.cmd.expect === 'img') this.finish(line)
      return
    }
    const ack = parseMwAck(line)
    if (ack) {
      if (this.current?.cmd.expect === 'mw') this.finish(line)
      return
    }
    const fault = parseFaultLine(line)
    if (fault !== undefined) {
      this.fault = fault
      this.notify()
      return
    }
    if (line.startsWith('?') && this.current) {
      this.finish(undefined, new Error(`device rejected "${this.current.cmd.text}": ${line.trim()}`))
    }
    // Anything else (plcc_print output, boot messages) is ignored.
  }

  private applyFrame(f: ImgFrame): void {
    this.image.I.set(f.I.subarray(0, this.image.I.length))
    this.image.Q.set(f.Q.subarray(0, this.image.Q.length))
    this.image.M.set(f.M.subarray(0, this.image.M.length))
    this.knownM = f.M.slice()
    this.lastUpdate = Date.now()
    this.rxCount++
    this.misses = 0
    if (this.state !== 'online') this.setState('online')
    else this.notify()
  }

  private requireOpen(): void {
    if (this.state !== 'online' && this.state !== 'error') throw new Error('not connected')
  }

  private requireCommand(cmd: 'mw' | 'img'): void {
    if (!this.device.console?.commands.includes(cmd)) {
      throw new Error(`${this.device.device.name}'s console has no \`${cmd}\` command (see its manifest)`)
    }
  }

  /** Writes %MWn (a Modbus holding register on the Opta). */
  async writeWord(n: number, value: number): Promise<void> {
    this.requireOpen()
    this.requireCommand('mw')
    const v = Math.trunc(value) & 0xffff
    await this.send(mwCommand(n, v), 'mw')
    this.applyWord(n, v)
  }

  private applyWord(n: number, v: number): void {
    if (2 * n + 1 < this.image.M.length) {
      this.image.M[2 * n] = v & 0xff
      this.image.M[2 * n + 1] = v >> 8
    }
    if (2 * n + 1 < this.knownM.length) {
      this.knownM[2 * n] = v & 0xff
      this.knownM[2 * n + 1] = v >> 8
    }
    this.notify()
  }

  /** Sets or clears a %MX bit by read-modify-writing its containing %MW word. */
  async forceBit(address: string, on: boolean): Promise<void> {
    this.requireOpen()
    this.requireCommand('mw')
    const a = parseAddress(address)
    if (!a) throw new Error(`"${address}" is not a direct address`)
    const { n, value } = bitForceWrite(this.knownM, a, on)
    await this.send(mwCommand(n, value), 'mw')
    this.applyWord(n, value)
  }

  /** Whether `address` can be written over this console. */
  canWrite(address: string): boolean {
    const a = parseAddress(address)
    if (!a || a.area !== 'M' || !this.device.console?.commands.includes('mw')) return false
    if (a.size === 'X') return 2 * Math.floor(a.byte / 2) + 1 < Math.max(this.knownM.length, this.device.console?.img_m_bytes ?? 0)
    return a.size === 'W'
  }
}
