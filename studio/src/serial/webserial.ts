// SPDX-License-Identifier: MPL-2.0
//
// SerialTransport over the WebSerial API (Chrome / Edge, secure context).
//
// The wire format is the one the plcc runtimes' console reads
// (runtimes/arduino-opta/loader/loader.ino, `console()`): 115200 8N1, each
// command ends in a single "\n" — never "\r\n": the runtime ends a line at
// either character, so "\r\n" would be a second, empty command answered with
// `? commands: ...`, which the session would take as the reply to the next
// command. Replies end in "\r\n" (Serial.println); the "\r" is dropped. DTR
// is asserted after opening: the Arduino core's USB serial only sends while
// the host holds DTR (a 1200-baud open with DTR dropped is the bootloader
// touch, webdfu's touch1200, never this transport).

import { Listeners } from './transport'
import type { SerialTransport } from './transport'

export function isWebSerialSupported(): boolean {
  return typeof navigator !== 'undefined' && 'serial' in navigator
}

export interface WebSerialOptions {
  baudRate?: number
  filters?: SerialPortFilter[]
  /** Reuse a port (e.g. one from navigator.serial.getPorts()) instead of prompting. */
  port?: SerialPort
}

export class WebSerialTransport implements SerialTransport {
  private port?: SerialPort
  private reader?: ReadableStreamDefaultReader<string>
  private readDone?: Promise<void>
  private lines = new Listeners<[string]>()
  private closes = new Listeners<[string | undefined]>()
  private open_ = false
  private closing = false
  private readonly opts: WebSerialOptions
  private readonly onDisconnect = (e: Event) => {
    if (e.target === this.port) void this.shutdown('device disconnected')
  }

  constructor(opts: WebSerialOptions = {}) {
    this.opts = opts
  }

  get isOpen(): boolean {
    return this.open_
  }

  async open(): Promise<void> {
    if (!isWebSerialSupported()) {
      throw new Error('WebSerial is not available; use Chrome or Edge over https or localhost')
    }
    const port = this.opts.port ?? (await navigator.serial.requestPort({ filters: this.opts.filters ?? [] }))
    await port.open({ baudRate: this.opts.baudRate ?? 115200, dataBits: 8, stopBits: 1, parity: 'none', flowControl: 'none' })
    try {
      await port.setSignals({ dataTerminalReady: true, requestToSend: true })
    } catch {
      // Some platforms do not support it; the OS asserts DTR on open there.
    }
    this.port = port
    this.open_ = true
    this.closing = false
    navigator.serial.addEventListener('disconnect', this.onDisconnect)
    this.readDone = this.readLoop(port)
  }

  private async readLoop(port: SerialPort): Promise<void> {
    if (!port.readable) return
    const decoder = new TextDecoderStream()
    const piped = port.readable.pipeTo(decoder.writable as WritableStream<Uint8Array>).catch(() => {})
    const reader = decoder.readable.getReader()
    this.reader = reader
    let buf = ''
    let reason: string | undefined
    try {
      for (;;) {
        const { value, done } = await reader.read()
        if (done) break
        buf += value
        let nl: number
        while ((nl = buf.indexOf('\n')) >= 0) {
          const line = buf.slice(0, nl).replace(/\r$/, '')
          buf = buf.slice(nl + 1)
          this.lines.emit(line)
        }
      }
    } catch (e) {
      reason = e instanceof Error ? e.message : String(e)
    } finally {
      reader.releaseLock()
    }
    await piped
    if (!this.closing) await this.shutdown(reason ?? 'device closed the connection', true)
  }

  async writeLine(text: string): Promise<void> {
    const w = this.port?.writable?.getWriter()
    if (!w) throw new Error('port is not open')
    try {
      await w.write(new TextEncoder().encode(`${text}\n`))
    } finally {
      w.releaseLock()
    }
  }

  onLine(cb: (line: string) => void): () => void {
    return this.lines.add(cb)
  }

  onClose(cb: (reason?: string) => void): () => void {
    return this.closes.add(cb)
  }

  async close(): Promise<void> {
    await this.shutdown(undefined)
  }

  private async shutdown(reason: string | undefined, fromReadLoop = false): Promise<void> {
    if (this.closing || !this.open_) return
    this.closing = true
    this.open_ = false
    navigator.serial.removeEventListener('disconnect', this.onDisconnect)
    try {
      await this.reader?.cancel()
    } catch {
      // The stream may already be errored.
    }
    // The read loop cannot wait for itself.
    if (!fromReadLoop) await this.readDone?.catch(() => {})
    try {
      await this.port?.close()
    } catch {
      // Already closed or unplugged.
    }
    this.port = undefined
    this.closes.emit(reason)
  }
}
