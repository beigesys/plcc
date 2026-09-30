// SPDX-License-Identifier: MPL-2.0
//
// Online mode: an OnlineSession over WebSerial (or the demo device) feeding
// the live store. Everything here is async and event driven; the session
// polls `img` on a timer and never waits in a loop.

import { Simulator } from '@/engine'
import type { Device } from '@/devices/manifest'
import type { Project } from '@/model'
import { FakeConsoleTransport, OnlineSession, WebSerialTransport, identifyDevice, isWebSerialSupported, matchCatalog, type Detection } from '@/serial'
import { useLive, valuesFromImage } from './live'

let session: OnlineSession | null = null
let demo: DemoDevice | null = null
let projectRef: Project | null = null
let frame = 0

/**
 * The demo device: a fake console for the project's device with the preview
 * simulator running the project against its image, so Online mode can be
 * tried without hardware.
 */
class DemoDevice {
  readonly transport: FakeConsoleTransport
  private sim: Simulator
  private timer: ReturnType<typeof setInterval>

  constructor(project: Project, device: Device) {
    this.transport = new FakeConsoleTransport(device)
    this.sim = new Simulator(project, device)
    this.timer = setInterval(() => this.step(), 20)
  }

  private step() {
    const t = this.transport
    const img = this.sim.image
    img.I.set(t.I.subarray(0, img.I.length))
    img.M.set(t.M.subarray(0, img.M.length))
    this.sim.scan(20, { budgetMs: 4 })
    t.Q.set(img.Q.subarray(0, t.Q.length))
    t.M.set(img.M.subarray(0, t.M.length))
  }

  setProject(p: Project) {
    this.sim.setProject(p)
  }

  stop() {
    clearInterval(this.timer)
  }
}

function publish() {
  frame = 0
  const s = session
  if (!s) return
  const p = projectRef
  useLive.setState({
    source: 'online',
    trace: null,
    values: p ? valuesFromImage(p, s.image) : {},
    image: { I: s.image.I.slice(), Q: s.image.Q.slice(), M: s.image.M.slice() },
    online: {
      state: s.state,
      error: s.error,
      fault: s.fault,
      lastUpdate: s.lastUpdate,
      transport: demo ? 'fake' : 'webserial',
      mKnown: s.knownM.length,
      identity: s.identity,
      mismatch: s.identityCheck?.mismatch,
      notes: s.identityCheck?.notes,
    },
  })
}

function schedule() {
  if (!frame) frame = requestAnimationFrame(publish)
}

export function onlineSupported(): boolean {
  return isWebSerialSupported()
}

export async function connectOnline(project: Project, device: Device, kind: 'webserial' | 'fake') {
  await disconnectOnline()
  projectRef = project
  if (!device.console) {
    useLive.setState({
      source: 'online',
      online: {
        state: 'disconnected',
        error: `${device.device.name}'s manifest has no [console]; there is nothing to connect to`,
        lastUpdate: 0,
        transport: kind,
        mKnown: 0,
      },
    })
    return
  }
  let transport
  if (kind === 'fake') {
    demo = new DemoDevice(project, device)
    transport = demo.transport
  } else {
    transport = new WebSerialTransport({ baudRate: device.console.baud })
  }
  session = new OnlineSession(transport, device, { pollMs: 100 })
  session.subscribe(schedule)
  useLive.setState({
    source: 'online',
    values: {},
    trace: null,
    online: { state: 'connecting', lastUpdate: 0, transport: kind, mKnown: 0 },
  })
  try {
    await session.connect()
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e)
    useLive.setState({ online: { state: 'disconnected', error: msg, lastUpdate: 0, transport: kind, mKnown: 0 } })
    demo?.stop()
    demo = null
    session = null
  }
}

export async function disconnectOnline() {
  const s = session
  session = null
  demo?.stop()
  demo = null
  if (frame) cancelAnimationFrame(frame)
  frame = 0
  if (s) await s.disconnect()
  if (useLive.getState().source === 'online') {
    useLive.setState({
      source: 'none',
      values: {},
      trace: null,
      image: null,
      online: { state: 'disconnected', lastUpdate: 0, transport: null, mKnown: 0 },
    })
  }
}

export function setOnlineProject(p: Project) {
  projectRef = p
  demo?.setProject(p)
  schedule()
}

export function onlineCanWrite(address: string): boolean {
  return !!session && session.canWrite(address)
}

export async function onlineForceBit(address: string, on: boolean) {
  if (!session) throw new Error('not connected')
  await session.forceBit(address, on)
}

export async function onlineWriteWord(n: number, value: number) {
  if (!session) throw new Error('not connected')
  await session.writeWord(n, value)
}

/** Demo device only: drive an input terminal. */
export function demoSetInput(address: string, value: boolean | number) {
  demo?.transport.setAddress(address, value)
}

/**
 * "Add device → Detect": asks the device on a user-picked serial port for
 * `info` and matches it to the catalog. Not while Online holds the port.
 */
export async function detectOverSerial(baudRate = 115200): Promise<Detection> {
  if (session) throw new Error('Disconnect Online first; it holds the serial port')
  return matchCatalog(await identifyDevice(new WebSerialTransport({ baudRate })))
}

export function isDemoDevice(): boolean {
  return demo !== null
}
