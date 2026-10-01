// SPDX-License-Identifier: MPL-2.0
// WebSerialTransport against a stand-in for the WebSerial API: the exact
// bytes it writes, how it splits what it reads, DTR, disconnects. With
// FakeConsoleTransport (which prints what the firmware prints, byte for byte)
// this pins the browser side of the console protocol.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { OnlineSession } from './session'
import { WebSerialTransport } from './webserial'
import { catalogEntry } from '@/devices/catalog'

class FakePort extends EventTarget {
  written: string[] = []
  signals: SerialOutputSignals[] = []
  opened?: SerialOptions
  closed = false
  private controller!: ReadableStreamDefaultController<Uint8Array>
  readable: ReadableStream<Uint8Array> | null = null
  writable: WritableStream<Uint8Array> | null = null
  /** What the device answers per command line. */
  reply: (line: string) => string | null = () => null

  async open(o: SerialOptions) {
    this.opened = o
    this.readable = new ReadableStream<Uint8Array>({ start: (c) => (this.controller = c) })
    this.writable = new WritableStream<Uint8Array>({
      write: (chunk) => {
        const text = new TextDecoder().decode(chunk)
        this.written.push(text)
        for (const line of text.split('\n').slice(0, -1)) {
          const r = this.reply(line)
          if (r !== null) this.send(`${r}\r\n`)
        }
      },
    })
  }
  async setSignals(s: SerialOutputSignals) {
    this.signals.push(s)
  }
  /** Bytes from the device, possibly split anywhere. */
  send(text: string) {
    this.controller.enqueue(new TextEncoder().encode(text))
  }
  unplug() {
    this.controller.error(new Error('The device has been lost.'))
  }
  async close() {
    this.closed = true
  }
}

function install(port: FakePort) {
  const serial = new EventTarget() as EventTarget & { requestPort(): Promise<FakePort> }
  serial.requestPort = async () => port
  vi.stubGlobal('navigator', { serial })
  return serial
}

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('WebSerialTransport', () => {
  it('opens 115200 8N1, asserts DTR, and sends each command with a single "\\n"', async () => {
    const port = new FakePort()
    install(port)
    const t = new WebSerialTransport({ baudRate: 115200 })
    await t.open()
    expect(port.opened).toMatchObject({ baudRate: 115200, dataBits: 8, stopBits: 1, parity: 'none', flowControl: 'none' })
    expect(port.signals).toEqual([{ dataTerminalReady: true, requestToSend: true }])
    await t.writeLine('mw 3 1234')
    await t.writeLine('img')
    expect(port.written).toEqual(['mw 3 1234\n', 'img\n'])
    await t.close()
    expect(port.closed).toBe(true)
  })

  it('splits replies into lines across chunks, dropping "\\r"', async () => {
    const port = new FakePort()
    install(port)
    const t = new WebSerialTransport()
    const lines: string[] = []
    t.onLine((l) => lines.push(l))
    await t.open()
    port.send('I: 1 0  Q: 1')
    port.send('1  M: 0\r\nok %MW3 := 12')
    port.send('34\r\n{"device":"arduino-opta"}\r')
    port.send('\n')
    await new Promise((r) => setTimeout(r, 10))
    expect(lines).toEqual(['I: 1 0  Q: 11  M: 0', 'ok %MW3 := 1234', '{"device":"arduino-opta"}'])
    await t.close()
  })

  it('reports an unplugged device as a close with the reason', async () => {
    const port = new FakePort()
    install(port)
    const t = new WebSerialTransport()
    const closed: (string | undefined)[] = []
    t.onClose((r) => closed.push(r))
    await t.open()
    port.unplug()
    await new Promise((r) => setTimeout(r, 10))
    expect(closed).toEqual(['The device has been lost.'])
    expect(t.isOpen).toBe(false)
  })

  it('runs an Online session end to end against firmware replies', async () => {
    const port = new FakePort()
    install(port)
    const opta = catalogEntry('arduino-opta')!.device
    port.reply = (line) => {
      if (line === 'info') return '{"device":"arduino-opta","manifest":2,"runtime":"plcc-arduino","abi":1,"image":{"I":18,"Q":1,"M":64},"state":"run","program":{"build":"00","size":8,"crc":"0","version":2}}'
      if (line === 'img') return `I: ${'0 '.repeat(17)}0  Q: 1  M:${' 0'.repeat(64)}`
      if (line === 'stop') return 'ok stop'
      return '? commands: info | img | mw <n> <value> | prog | stop | run'
    }
    const s = new OnlineSession(new WebSerialTransport(), opta, { pollMs: 50, infoEveryMs: 10_000 })
    await s.connect()
    await new Promise((r) => setTimeout(r, 30))
    expect(s.state).toBe('online')
    expect(s.identity?.program?.state).toBe('run')
    expect(s.image.Q[0]).toBe(1)
    expect(await s.stopProgram()).toBe('ok stop')
    expect(port.written.every((w) => w.endsWith('\n') && !w.includes('\r'))).toBe(true)
    await s.disconnect()
  })
})
