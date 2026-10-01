// SPDX-License-Identifier: MPL-2.0
//
// Download: compile the project for its device in the browser, link a
// program image for the device's program slot (@plcc/plc-image), and write
// it with WebUSB DFU (@plcc/webdfu, the slot-only profile). Every step runs
// here; hardware access goes through `DownloadIo`, so tests drive the whole
// flow against webdfu's fake bootloader.
//
// Safety, on top of webdfu's own (built-in per-bootloader floors, every
// erase and write re-checked, no mass erase):
// - only the manifest's program slot is written ([flash.program],
//   programProfileFromManifest), never the runtime or the bootloader;
// - before rebooting the board, its console must answer `info` as a
//   program-image runtime of the same device: a board still running a
//   firmware that does not load images is left alone.

import { create } from 'zustand'
import { buildDeviceImage, type DeviceImage } from '@plcc/plc-image'
import {
  DfuseDevice, ManifestError, SafetyError, isRuntimePort, programProfileFromManifest, touch1200, waitForDfuDevice,
  type DeviceProfile, type Progress, type SerialPortLike, type UsbDeviceLike, type UsbLike,
} from '@plcc/webdfu'
import type { Device } from '@/devices/manifest'
import { toLadderJson, type Project } from '@/model'
import { compile, type CompileRequest, type CompileResult } from '@/plcc/compiler'
import type { DeviceIdentity } from '@/serial'
import { toProblem, type Problem } from './problems'

export type StepId = 'compile' | 'image' | 'identify' | 'reboot' | 'dfu' | 'flash' | 'done'
export type StepState = 'pending' | 'active' | 'ok' | 'failed' | 'skipped'

export interface Step {
  id: StepId
  title: string
  state: StepState
  detail?: string
}

export interface DownloadState {
  steps: Step[]
  /** The step that needs a click (a browser permission prompt needs a user gesture). */
  waitingFor: 'serial' | 'usb' | null
  progress: Progress | null
  image: DeviceImage | null
  problems: Problem[]
  error: string | null
  running: boolean
}

const STEPS: { id: StepId; title: string }[] = [
  { id: 'compile', title: 'Compile for the device' },
  { id: 'image', title: 'Link the program image' },
  { id: 'identify', title: 'Check the device (console info)' },
  { id: 'reboot', title: 'Reboot into the bootloader (1200-baud touch)' },
  { id: 'dfu', title: 'Open the bootloader (WebUSB DFU)' },
  { id: 'flash', title: 'Erase, write and verify the program slot' },
  { id: 'done', title: 'Start the runtime' },
]

const initial = (): DownloadState => ({
  steps: STEPS.map((s) => ({ ...s, state: 'pending' })),
  waitingFor: null,
  progress: null,
  image: null,
  problems: [],
  error: null,
  running: false,
})

export const useDownload = create<DownloadState>(() => initial())

function step(id: StepId, state: StepState, detail?: string) {
  useDownload.setState((s) => ({ steps: s.steps.map((x) => (x.id === id ? { ...x, state, ...(detail !== undefined ? { detail } : {}) } : x)) }))
}

export function resetDownload() {
  useDownload.setState(initial())
}

/** The device's program slot profile, or why there is none. */
export function slotProfile(device: Device): DeviceProfile {
  if (!device.flash) throw new ManifestError(`${device.device.name} cannot be downloaded to: its manifest has no [flash] section`)
  return programProfileFromManifest(device.flash, device.device.name)
}

/** Steps 1-2: compile the project for `device` and link its program image. Nothing touches hardware. */
export async function buildImage(
  project: Project,
  device: Device,
  manifestText: string,
  build: (req: CompileRequest) => Promise<CompileResult> = compile,
): Promise<DeviceImage | null> {
  useDownload.setState({ ...initial(), running: true })
  try {
    if (!device.flash?.program) {
      step('compile', 'failed', `${device.device.name} has no program slot ([flash.program]): its runtime does not load program images`)
      useDownload.setState({ error: `${device.device.name} cannot take a program download`, running: false })
      return null
    }
    slotProfile(device) // refuse a manifest webdfu would refuse, before compiling
    step('compile', 'active', `${device.target.triple}${device.target.cpu ? `, ${device.target.cpu}` : ''}`)
    const t0 = performance.now()
    const r = await build({ files: { 'project.json': toLadderJson(project) }, entry: ['project.json'], device: manifestText, opt_level: 2 })
    if (!r.ok || !r.object) {
      const problems = r.diagnostics.map((d) => toProblem(d as Parameters<typeof toProblem>[0], 'compile'))
      step('compile', 'failed', problems.find((p) => p.severity === 'error')?.message ?? 'no object')
      useDownload.setState({ problems, error: 'the project does not compile for the device', running: false })
      return null
    }
    step('compile', 'ok', `${r.object.length} bytes of ${r.target} in ${Math.round(performance.now() - t0)} ms`)
    step('image', 'active')
    const image = await buildDeviceImage(device, r.object)
    step('image', 'ok', `${image.size} bytes at 0x${image.address.toString(16).toUpperCase()}, build ${image.buildId.slice(0, 8)}`)
    useDownload.setState({ image, running: false })
    return image
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e)
    const cur = useDownload.getState().steps.find((s) => s.state === 'active')
    if (cur) step(cur.id, 'failed', msg)
    useDownload.setState({ error: msg, running: false })
    return null
  }
}

/** What the flow needs from the browser (WebSerial, WebUSB); tests pass fakes. */
export interface DownloadIo {
  /** A serial port of the board running its runtime: one already granted, else a prompt (needs a user gesture). */
  serialPort(profile: DeviceProfile): Promise<SerialPortLike>
  /** `info` over that port. */
  identify(port: SerialPortLike): Promise<DeviceIdentity>
  usb: UsbLike
  /** Grant access to the bootloader (navigator.usb.requestDevice; needs a user gesture). */
  requestUsb(profile: DeviceProfile): Promise<UsbDeviceLike>
  sleep?: (ms: number) => Promise<void>
}

/**
 * Steps 3-7: check the device, reboot it into DFU, write the program slot,
 * verify, start the runtime. Call from a click handler (WebSerial and
 * WebUSB prompts need a user gesture); when the bootloader has not been
 * granted to the page yet, `waitingFor` becomes 'usb' and `continueWithUsb`
 * (from the next click) carries on.
 */
export async function flashImage(device: Device, image: DeviceImage, io: DownloadIo): Promise<boolean> {
  const profile = slotProfile(device)
  useDownload.setState({ running: true, error: null })
  try {
    step('identify', 'active')
    const port = await io.serialPort(profile)
    if (!isRuntimePort(port, profile) && port.getInfo) {
      const i = port.getInfo()
      if (i.usbVendorId !== undefined) throw new SafetyError(`that serial port (USB ${hex4(i.usbVendorId)}:${hex4(i.usbProductId ?? 0)}) is not ${device.device.name} running its runtime`)
    }
    const id = await io.identify(port)
    if (id.device !== device.device.id) throw new SafetyError(`the board reports it is "${id.device}", not ${device.device.id}`)
    if (!id.program) {
      throw new SafetyError(
        `${device.device.name} runs a firmware that does not load program images (manifest version ${id.manifest}); ` +
          'flash the program-image runtime once first (runtimes/arduino-opta/README.md)',
      )
    }
    if (id.abi !== device.target.runtime.abi) throw new SafetyError(`the runtime speaks ABI ${id.abi}, the program needs ABI ${device.target.runtime.abi}`)
    step('identify', 'ok', `${id.device}, runtime ${id.runtime} ABI ${id.abi}, program ${id.program.state}`)

    step('reboot', 'active')
    await touch1200(port, io.sleep)
    step('reboot', 'ok')
    return await openAndFlash(profile, image, io)
  } catch (e) {
    return fail(e)
  }
}

async function openAndFlash(profile: DeviceProfile, image: DeviceImage, io: DownloadIo): Promise<boolean> {
  step('dfu', 'active', 'waiting for the bootloader')
  let usb: UsbDeviceLike
  try {
    usb = await waitForDfuDevice(io.usb, profile, { timeoutMs: 8000, sleep: io.sleep })
  } catch {
    // Not granted to this page yet: the next click asks.
    useDownload.setState({ waitingFor: 'usb', running: false })
    step('dfu', 'active', 'allow access to the bootloader')
    pendingImage = { profile, image, io }
    return false
  }
  return writeSlot(usb, profile, image, io)
}

let pendingImage: { profile: DeviceProfile; image: DeviceImage; io: DownloadIo } | null = null

/** After `waitingFor: 'usb'`: from a click, grant the bootloader and flash. */
export async function continueWithUsb(): Promise<boolean> {
  const p = pendingImage
  if (!p) return false
  pendingImage = null
  useDownload.setState({ waitingFor: null, running: true })
  try {
    const usb = await p.io.requestUsb(p.profile)
    return await writeSlot(usb, p.profile, p.image, p.io)
  } catch (e) {
    return fail(e)
  }
}

async function writeSlot(usb: UsbDeviceLike, profile: DeviceProfile, image: DeviceImage, io: DownloadIo): Promise<boolean> {
  const dev = await DfuseDevice.open(usb, { profile, sleep: io.sleep })
  step('dfu', 'ok', `${dev.layoutText.trim()}`)
  step('flash', 'active')
  await dev.flash(image.bytes, {
    address: image.address,
    verify: true,
    onProgress: (p) => {
      useDownload.setState({ progress: p })
      if (p.phase === 'manifest') {
        step('flash', 'ok', `${image.size} bytes written and verified at 0x${image.address.toString(16).toUpperCase()}`)
        step('done', 'active')
      }
    },
  })
  step('done', 'ok', `the runtime starts the program (build ${image.buildId.slice(0, 8)})`)
  useDownload.setState({ running: false, progress: null })
  return true
}

function fail(e: unknown): false {
  const msg = e instanceof Error ? e.message : String(e)
  const cur = useDownload.getState().steps.find((s) => s.state === 'active')
  if (cur) step(cur.id, 'failed', msg)
  useDownload.setState({ error: msg, running: false, waitingFor: null })
  return false
}

function hex4(n: number) {
  return n.toString(16).padStart(4, '0')
}

// ---------------------------------------------------------------- the browser

/** WebSerial + WebUSB, as the page has them. */
export async function browserIo(): Promise<DownloadIo> {
  const nav = navigator as Navigator & {
    serial?: Serial
    usb?: UsbLike & { requestDevice(o: { filters: { vendorId: number; productId: number }[] }): Promise<unknown> }
  }
  if (!nav.serial || !nav.usb) throw new Error('Downloading needs WebSerial and WebUSB: use Chrome or Edge')
  const { WebSerialTransport, identifyDevice } = await import('@/serial')
  const { disconnectOnline } = await import('./online')
  return {
    async serialPort(profile) {
      // Online holds the port: let it go first.
      await disconnectOnline()
      const ports = await nav.serial!.getPorts()
      const known = ports.find((p) => isRuntimePort(p as unknown as SerialPortLike, profile))
      if (known) return known as unknown as SerialPortLike
      return (await nav.serial!.requestPort({ filters: profile.runtimeFilters })) as unknown as SerialPortLike
    },
    async identify(port) {
      return identifyDevice(new WebSerialTransport({ port: port as unknown as SerialPort, baudRate: 115200 }))
    },
    usb: nav.usb as unknown as UsbLike,
    async requestUsb(profile) {
      return (await nav.usb!.requestDevice({ filters: profile.dfuFilters })) as unknown as UsbDeviceLike
    },
  }
}
