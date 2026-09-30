// SPDX-License-Identifier: MPL-2.0
//
// The plcc runtime console (docs/device-manifest.md, "Console protocol";
// runtime.ino on the Opta):
//
//   info           ->  {"device":"arduino-opta","manifest":1,"runtime":"plcc-arduino","abi":1,"image":{"I":18,"Q":1,"M":64}}
//   img            ->  I: 1 0 0 0 D2 7 0 0  Q: 11  M: 1 0 0 0 0 0 0 0 ...
//   mw <n> <v>     ->  ok %MW3 := 257
//   anything else  ->  ? commands: info | img | mw <n> <value>
//
// Bytes are printed in hex WITHOUT zero padding. `img` reports the first
// `console.img_m_bytes` bytes of %M (the manifest says how many).

import type { Address } from '@/model'
import { formatAddress } from '@/model'
import type { Device } from '@/devices/manifest'

/** What a runtime's `info` reports about itself. */
export interface DeviceIdentity {
  device: string
  manifest: number
  runtime: string
  abi: number
  image: { I: number; Q: number; M: number }
}

/** Parses an `info` reply; undefined for any other line. */
export function parseInfoLine(line: string): DeviceIdentity | undefined {
  const t = line.trim()
  if (!t.startsWith('{')) return undefined
  let v: unknown
  try {
    v = JSON.parse(t)
  } catch {
    return undefined
  }
  if (typeof v !== 'object' || v === null) return undefined
  const o = v as Record<string, unknown>
  const img = o.image as Record<string, unknown> | undefined
  const num = (x: unknown) => (typeof x === 'number' && Number.isInteger(x) && x >= 0 ? x : undefined)
  if (typeof o.device !== 'string' || typeof o.runtime !== 'string') return undefined
  const manifest = num(o.manifest)
  const abi = num(o.abi)
  const I = num(img?.I)
  const Q = num(img?.Q)
  const M = num(img?.M)
  if (manifest === undefined || abi === undefined || I === undefined || Q === undefined || M === undefined) return undefined
  return { device: o.device, manifest, runtime: o.runtime, abi, image: { I, Q, M } }
}

/** The `info` line a runtime built for `device` prints. */
export function formatInfoLine(d: Pick<Device, 'device' | 'target'>): string {
  const { I, Q, M } = d.target.image
  return JSON.stringify({
    device: d.device.id,
    manifest: d.device.version,
    runtime: d.target.runtime.kind,
    abi: d.target.runtime.abi,
    image: { I, Q, M },
  })
}

export interface IdentityCheck {
  /** The device is a different kind or speaks another runtime ABI: values would be misread. */
  mismatch: string[]
  /** Worth knowing, but the image layout matches (e.g. an older manifest version). */
  notes: string[]
}

/** Compares what a device reports with the project's manifest. */
export function checkIdentity(id: DeviceIdentity, d: Device): IdentityCheck {
  const mismatch: string[] = []
  const notes: string[] = []
  if (id.device !== d.device.id) mismatch.push(`The connected device is "${id.device}", but the project's device is "${d.device.id}".`)
  if (id.abi !== d.target.runtime.abi) mismatch.push(`The runtime speaks ABI ${id.abi}; the manifest expects ABI ${d.target.runtime.abi}.`)
  if (id.runtime !== d.target.runtime.kind) mismatch.push(`The runtime is "${id.runtime}", the manifest expects "${d.target.runtime.kind}".`)
  const a = id.image
  const b = d.target.image
  if (a.I !== b.I || a.Q !== b.Q || a.M !== b.M) {
    mismatch.push(`The runtime's image is I=${a.I} Q=${a.Q} M=${a.M}; the manifest says I=${b.I} Q=${b.Q} M=${b.M}.`)
  }
  if (id.device === d.device.id && id.manifest !== d.device.version) {
    notes.push(`The runtime was built for manifest version ${id.manifest}; the project has version ${d.device.version}.`)
  }
  return { mismatch, notes }
}

export interface ImgFrame {
  I: Uint8Array
  Q: Uint8Array
  M: Uint8Array
}

function hexBytes(text: string): Uint8Array | undefined {
  const parts = text.trim().split(/\s+/).filter(Boolean)
  const out = new Uint8Array(parts.length)
  for (let i = 0; i < parts.length; i++) {
    if (!/^[0-9a-f]{1,2}$/i.test(parts[i])) return undefined
    out[i] = parseInt(parts[i], 16)
  }
  return out
}

export function parseImgLine(line: string): ImgFrame | undefined {
  const m = /^\s*I:(.*?)\s+Q:(.*?)\s+M:(.*?)\s*$/i.exec(line.replace(/\r/g, ''))
  if (!m) return undefined
  const I = hexBytes(m[1])
  const Q = hexBytes(m[2])
  const M = hexBytes(m[3])
  if (!I || !Q || !M) return undefined
  return { I, Q, M }
}

/** Formats a frame the way the firmware prints it. */
export function formatImgLine(f: ImgFrame): string {
  const hex = (b: Uint8Array) => [...b].map((x) => ` ${x.toString(16).toUpperCase()}`).join('')
  return `I:${hex(f.I)}  Q:${hex(f.Q)}  M:${hex(f.M)}`
}

export function mwCommand(n: number, value: number): string {
  if (!Number.isInteger(n) || n < 0) throw new Error(`bad register number ${n}`)
  return `mw ${n} ${Math.trunc(value) & 0xffff}`
}

export function parseMwAck(line: string): { n: number; value: number } | undefined {
  const m = /^\s*ok\s+%MW(\d+)\s*:=\s*(\d+)\s*$/i.exec(line)
  return m ? { n: Number(m[1]), value: Number(m[2]) } : undefined
}

/** `PLC STOP: fault ...` lines from plcc_fault(). */
export function parseFaultLine(line: string): string | undefined {
  const m = /^\s*PLC STOP:\s*(.*?)\s*$/.exec(line)
  return m ? m[1] : undefined
}

/**
 * The `mw` write that sets or clears one %M bit, computed from the known %M
 * bytes. %MXb.x lives in byte b; %MWn covers bytes 2n..2n+1 little-endian, so
 * the word is n = floor(b/2) and the bit inside it is (b%2)*8 + x.
 */
export function bitForceWrite(M: Uint8Array, address: Address, on: boolean): { n: number; value: number } {
  if (address.area !== 'M' || address.size !== 'X') {
    throw new Error(`${formatAddress(address)} is not a %M bit; only %MX addresses can be forced over the console`)
  }
  const n = Math.floor(address.byte / 2)
  if (2 * n + 1 >= M.length) {
    throw new Error(
      `${formatAddress(address)} is in %MW${n}, outside the ${M.length} %M bytes the device reports; cannot read-modify-write it`,
    )
  }
  const word = M[2 * n] | (M[2 * n + 1] << 8)
  const bit = (address.byte % 2) * 8 + address.bit
  const value = on ? word | (1 << bit) : word & ~(1 << bit) & 0xffff
  return { n, value }
}
