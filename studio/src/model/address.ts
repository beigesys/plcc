// SPDX-License-Identifier: MPL-2.0
//
// IEC direct representation (`%IX0.0`, `%QX0.4`, `%IW2`, `%MW3`) and the
// process image it points into. Byte layout follows plcc's process image
// (docs/process-image.md): `%<area>X<b>.<x>` is bit x of byte b, and a sized
// address n covers bytes n*size .. n*size+size-1, little-endian. So `%MW3`
// is bytes 6..7 and `%MX6.0` is its low bit.

export type Area = 'I' | 'Q' | 'M'
export type Size = 'X' | 'B' | 'W' | 'D' | 'L'

export interface Address {
  area: Area
  size: Size
  /** Byte offset of the first byte. */
  byte: number
  /** Bit within the byte (X only). */
  bit: number
  /** Width in bytes (X counts as 1). */
  width: number
  /** The number as written: `3` in `%MW3`. */
  index: number
}

const WIDTH: Record<Size, number> = { X: 1, B: 1, W: 2, D: 4, L: 8 }

export function parseAddress(text: string): Address | undefined {
  const m = /^%([IQM])([XBWDL]?)(\d+)(?:\.(\d+))?$/i.exec(text.trim())
  if (!m) return undefined
  const area = m[1].toUpperCase() as Area
  let size = (m[2] || '').toUpperCase() as Size | ''
  const n = Number(m[3])
  const bitText = m[4]
  if (size === '') size = 'X'
  if (size === 'X') {
    if (bitText === undefined) return undefined
    const bit = Number(bitText)
    if (bit > 7) return undefined
    return { area, size, byte: n, bit, width: 1, index: n }
  }
  if (bitText !== undefined) return undefined
  const width = WIDTH[size]
  return { area, size, byte: n * width, bit: 0, width, index: n }
}

export function formatAddress(a: Address): string {
  return a.size === 'X' ? `%${a.area}X${a.byte}.${a.bit}` : `%${a.area}${a.size}${a.index}`
}

/** The IEC types an address size can hold, first one preferred. */
export function typesForSize(size: Size): string[] {
  switch (size) {
    case 'X':
      return ['BOOL']
    case 'B':
      return ['BYTE', 'SINT', 'USINT']
    case 'W':
      return ['INT', 'UINT', 'WORD']
    case 'D':
      return ['DINT', 'UDINT', 'DWORD', 'REAL']
    case 'L':
      return ['LINT', 'ULINT', 'LWORD', 'LREAL']
  }
}

/** A warning when `type` does not fit `address`, else undefined. */
export function addressTypeMismatch(address: string, type: string): string | undefined {
  const a = parseAddress(address)
  if (!a) return `"${address}" is not a direct address (like %IX0.0, %QX0.4, %IW2, %MW3)`
  const ok = typesForSize(a.size)
  if (!ok.includes(type.toUpperCase())) {
    return `${type} does not fit ${formatAddress(a)} (${a.size === 'X' ? 'a bit' : `${a.width * 8} bits`}); use ${ok.join(' / ')}`
  }
  return undefined
}

export class ProcessImage {
  readonly I: Uint8Array
  readonly Q: Uint8Array
  readonly M: Uint8Array

  constructor(sizes: { I: number; Q: number; M: number } = { I: 32, Q: 8, M: 64 }) {
    this.I = new Uint8Array(sizes.I)
    this.Q = new Uint8Array(sizes.Q)
    this.M = new Uint8Array(sizes.M)
  }

  /** An image over existing byte arrays (no copy). */
  static over(bytes: { I: Uint8Array; Q: Uint8Array; M: Uint8Array }): ProcessImage {
    const img = new ProcessImage({ I: 0, Q: 0, M: 0 })
    ;(img as { I: Uint8Array }).I = bytes.I
    ;(img as { Q: Uint8Array }).Q = bytes.Q
    ;(img as { M: Uint8Array }).M = bytes.M
    return img
  }

  area(a: Area): Uint8Array {
    return a === 'I' ? this.I : a === 'Q' ? this.Q : this.M
  }

  inRange(a: Address): boolean {
    return a.byte + a.width <= this.area(a.area).length
  }

  read(a: Address, type = ''): number | boolean {
    const buf = this.area(a.area)
    if (!this.inRange(a)) return a.size === 'X' ? false : 0
    if (a.size === 'X') return ((buf[a.byte] >> a.bit) & 1) === 1
    const dv = new DataView(buf.buffer, buf.byteOffset + a.byte, a.width)
    const t = type.toUpperCase()
    switch (a.size) {
      case 'B':
        return t === 'SINT' ? dv.getInt8(0) : dv.getUint8(0)
      case 'W':
        return t === 'INT' ? dv.getInt16(0, true) : dv.getUint16(0, true)
      case 'D':
        if (t === 'REAL') return dv.getFloat32(0, true)
        return t === 'DINT' ? dv.getInt32(0, true) : dv.getUint32(0, true)
      case 'L':
        if (t === 'LREAL') return dv.getFloat64(0, true)
        return Number(t === 'LINT' ? dv.getBigInt64(0, true) : dv.getBigUint64(0, true))
    }
  }

  write(a: Address, value: number | boolean, type = ''): void {
    const buf = this.area(a.area)
    if (!this.inRange(a)) return
    if (a.size === 'X') {
      if (value) buf[a.byte] |= 1 << a.bit
      else buf[a.byte] &= ~(1 << a.bit) & 0xff
      return
    }
    const n = typeof value === 'boolean' ? (value ? 1 : 0) : value
    const dv = new DataView(buf.buffer, buf.byteOffset + a.byte, a.width)
    const t = type.toUpperCase()
    switch (a.size) {
      case 'B':
        dv.setUint8(0, Math.trunc(n) & 0xff)
        return
      case 'W':
        dv.setUint16(0, Math.trunc(n) & 0xffff, true)
        return
      case 'D':
        if (t === 'REAL') dv.setFloat32(0, n, true)
        else dv.setUint32(0, Math.trunc(n) >>> 0, true)
        return
      case 'L':
        if (t === 'LREAL') dv.setFloat64(0, n, true)
        else dv.setBigInt64(0, BigInt(Math.trunc(n)), true)
        return
    }
  }
}
