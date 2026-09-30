// SPDX-License-Identifier: MPL-2.0
//
// Online mode: an OnlineSession over WebSerial (or the demo device) feeding
// the live store. Everything here is async and event driven; the session
// polls `img` on a timer and never waits in a loop.

import { Simulator } from '@/engine'
import { getProfile, type DeviceProfile } from '@/devices/profiles'
import type { Project } from '@/model'
import { FakeOptaTransport, OnlineSession, WebSerialTransport, isWebSerialSupported } from '@/serial'
import { useLive, valuesFromImage } from './live'

let session: OnlineSession | null = null
let demo: DemoDevice | null = null
let projectRef: Project | null = null
let frame = 0

/**
 * The demo device: the fake Opta console with the preview simulator running
 * the project against its image, so Online mode can be tried without hardware.
 */
class DemoDevice {
  readonly transport: FakeOptaTransport
  private sim: Simulator
  private timer: ReturnType<typeof setInterval>

  constructor(project: Project, profile: DeviceProfile) {
    this.transport = new FakeOptaTransport(profile)
    this.sim = new Simulator(project, profile)
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
    },
  })
}

function schedule() {
  if (!frame) frame = requestAnimationFrame(publish)
}

export function onlineSupported(): boolean {
  return isWebSerialSupported()
}

export async function connectOnline(project: Project, profileId: string, kind: 'webserial' | 'fake') {
  await disconnectOnline()
  projectRef = project
  const profile = getProfile(profileId)
  let transport
  if (kind === 'fake') {
    demo = new DemoDevice(project, profile)
    transport = demo.transport
  } else {
    transport = new WebSerialTransport({ baudRate: profile.transport?.baudRate ?? 115200 })
  }
  session = new OnlineSession(transport, profile, { pollMs: 100 })
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
  const t = demo?.transport
  if (!t) return
  const m = /^%IX0\.(\d)$/.exec(address)
  if (m) return t.setInput(Number(m[1]) + 1, !!value)
  if (address === '%IX1.0') return t.setButton(!!value)
  const w = /^%IW(\d+)$/.exec(address)
  if (w) t.setAnalog(Number(w[1]), Number(value))
}

export function isDemoDevice(): boolean {
  return demo !== null
}
