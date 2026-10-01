// SPDX-License-Identifier: MPL-2.0
/// <reference types="node" />
//
// Download, end to end, against webdfu's fake STM32 bootloader: the demo
// project compiled for the Opta by the real browser compiler (skipped when
// it has not been built), linked into a program image (@plcc/plc-image),
// the board checked over its console, rebooted, the slot erased, written,
// verified, and the runtime started. Nothing outside the slot is touched.

import { existsSync, readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { gunzipSync } from 'node:zlib'
import { beforeAll, beforeEach, describe, expect, it } from 'vitest'
import { compile as compileWith } from '@plcc/plcc-compiler-wasm'
import { FakeDfuDevice } from '../../../packages/webdfu/test/fake-device'
import { catalogEntry, simulatorEntry } from '@/devices/catalog'
import { demoProject } from '@/model'
import type { DeviceIdentity } from '@/serial'
import { buildImage, continueWithUsb, flashImage, resetDownload, useDownload, type DownloadIo } from './download'

const here = dirname(fileURLToPath(import.meta.url))
const gz = join(here, '../../../packages/plcc-compiler-wasm/dist/plcc-compiler.wasm.gz')
const imageWasm = join(here, '../../../packages/plc-image/pkg/plcc_image.wasm')
const built = existsSync(gz) && existsSync(imageWasm)
const opta = catalogEntry('arduino-opta')!

let compiler: WebAssembly.Module
const build = (req: Parameters<typeof compileWith>[1]) => compileWith(compiler, req)

const RUNNING: DeviceIdentity = {
  device: 'arduino-opta',
  manifest: 2,
  runtime: 'plcc-arduino',
  abi: 1,
  image: { I: 18, Q: 1, M: 64 },
  program: { state: 'empty', image: null, reason: 'empty slot' },
}

function fakeIo(opts: { identity?: DeviceIdentity; granted?: boolean } = {}) {
  const dfu = new FakeDfuDevice()
  dfu.seedBootloader()
  // The runtime below the slot.
  for (let a = 0x08040000; a < 0x08060000; a += 4) dfu.flash.set(a, 0x5a)
  let inDfu = false
  const calls: string[] = []
  const port = {
    async open(o: { baudRate: number }) {
      calls.push(`open ${o.baudRate}`)
    },
    async setSignals(s: { dataTerminalReady?: boolean }) {
      calls.push(`dtr ${String(s.dataTerminalReady)}`)
    },
    async close() {
      calls.push('close')
      // The 1200-baud touch: the board reboots into its bootloader.
      if (calls.includes('open 1200')) inDfu = true
    },
    getInfo: () => ({ usbVendorId: 0x2341, usbProductId: 0x0264 }),
  }
  const io: DownloadIo = {
    serialPort: async () => port,
    identify: async () => {
      calls.push('info')
      return opts.identity ?? RUNNING
    },
    usb: { getDevices: async () => (inDfu && opts.granted !== false ? [dfu] : []) },
    requestUsb: async () => {
      calls.push('requestDevice')
      return dfu
    },
    sleep: async () => {},
  }
  return { io, dfu, calls }
}

describe.skipIf(!built)('Download (program image to the slot, fake bootloader)', () => {
  beforeAll(async () => {
    compiler = await WebAssembly.compile(gunzipSync(readFileSync(gz)))
  })
  beforeEach(() => resetDownload())

  it('compiles, links, checks the board, reboots it, writes and verifies the slot, starts the runtime', async () => {
    const image = await buildImage(demoProject(), opta.device, opta.text, build)
    expect(useDownload.getState().error).toBeNull()
    expect(image).not.toBeNull()
    expect(image!.address).toBe(0x08180000)
    expect(image!.bytes.length).toBeGreaterThan(128)
    const { io, dfu, calls } = fakeIo()
    expect(await flashImage(opta.device, image!, io)).toBe(true)
    expect(calls).toEqual(['info', 'open 1200', 'dtr false', 'close'])
    expect(calls).toContain('open 1200')
    // The slot holds the image, byte for byte; the runtime and bootloader are untouched.
    for (let i = 0; i < image!.bytes.length; i++) expect(dfu.read(0x08180000 + i)).toBe(image!.bytes[i])
    expect(dfu.read(0x08040000)).toBe(0x5a)
    expect(dfu.read(0x08000000)).toBe(0xb0)
    // Leaving DFU starts the runtime, not the slot.
    expect(dfu.leftTo).toBe(0x08040000)
    const steps = useDownload.getState().steps
    expect(steps.every((s) => s.state === 'ok')).toBe(true)
  })

  it('asks for access to the bootloader when the page has none, then flashes on the next click', async () => {
    const image = (await buildImage(demoProject(), opta.device, opta.text, build))!
    const { io, dfu, calls } = fakeIo({ granted: false })
    expect(await flashImage(opta.device, image, io)).toBe(false)
    expect(useDownload.getState().waitingFor).toBe('usb')
    expect(await continueWithUsb()).toBe(true)
    expect(calls).toContain('requestDevice')
    expect(dfu.read(0x08180000)).toBe(image.bytes[0])
  })

  it('leaves a board alone that runs a firmware without a program slot', async () => {
    const image = (await buildImage(demoProject(), opta.device, opta.text, build))!
    const old = { ...RUNNING, manifest: 1 }
    delete old.program
    const { io, dfu, calls } = fakeIo({ identity: old })
    expect(await flashImage(opta.device, image, io)).toBe(false)
    expect(useDownload.getState().error).toMatch(/does not load program images/)
    expect(calls).not.toContain('open 1200')
    expect(dfu.read(0x08180000)).toBe(0xff)
  })

  it('refuses a device without a program slot before compiling', async () => {
    const image = await buildImage(demoProject(), simulatorEntry().device, simulatorEntry().text, build)
    expect(image).toBeNull()
    expect(useDownload.getState().error).toMatch(/cannot take a program download/)
  })

  it('stops at compile errors with the diagnostics', async () => {
    const p = demoProject()
    p.globals = p.globals.filter((t) => t.name !== 'RunTimer')
    const image = await buildImage(p, opta.device, opta.text, build)
    expect(image).toBeNull()
    expect(useDownload.getState().problems.some((x) => x.message.includes('RunTimer'))).toBe(true)
  })
})
