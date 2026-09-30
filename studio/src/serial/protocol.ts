// SPDX-License-Identifier: MPL-2.0
//
// The plcc Opta runtime's USB console (runtime.ino):
//
//   img            ->  I: 1 0 0 0 D2 7 0 0  Q: 11  M: 1 0 0 0 0 0 0 0
//   mw <n> <v>     ->  ok %MW3 := 257
//   anything else  ->  ? commands: mw <n> <value> | img
//
// Bytes are printed in hex WITHOUT zero padding. Only the first 8 %M bytes
// are reported.

import type { Address } from '@/model'
import { formatAddress } from '@/model'

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
