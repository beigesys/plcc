// SPDX-License-Identifier: MPL-2.0
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ARDUINO_OPTA } from '@/devices/profiles'
import { parseAddress } from '@/model'
import type { Address } from '@/model'
import {
  FakeOptaTransport, OnlineSession, bitForceWrite, formatImgLine, mwCommand, parseFaultLine, parseImgLine, parseMwAck,
} from '@/serial'

const addr = (s: string): Address => {
  const a = parseAddress(s)
  if (!a) throw new Error(s)
  return a
}

describe('img line', () => {
  it('parses the firmware format (unpadded hex)', () => {
    const f = parseImgLine('I: 1 0 0 0 D2 7 0 0  Q: 11  M: 1 0 0 0 0 0 0 0')
    expect(f).toBeDefined()
    expect([...f!.I]).toEqual([1, 0, 0, 0, 0xd2, 7, 0, 0])
    expect([...f!.Q]).toEqual([0x11])
    expect([...f!.M]).toEqual([1, 0, 0, 0, 0, 0, 0, 0])
  })

  it('accepts empty sections, padding, lower case, CR and extra spaces', () => {
    expect(parseImgLine('I:  Q:  M:')).toEqual({ I: new Uint8Array(0), Q: new Uint8Array(0), M: new Uint8Array(0) })
    const f = parseImgLine('  i: 01 0a ff   q: 0f  m: 00 10\r')
    expect([...f!.I]).toEqual([1, 10, 255])
    expect([...f!.Q]).toEqual([15])
    expect([...f!.M]).toEqual([0, 16])
    expect(parseImgLine('I: 1  Q:  M: 2')).toBeDefined()
  })

  it('rejects other lines', () => {
    expect(parseImgLine('ok %MW1 := 3')).toBeUndefined()
    expect(parseImgLine('I: 1 ZZ  Q: 0  M: 0')).toBeUndefined()
    expect(parseImgLine('I: 100  Q: 0  M: 0')).toBeUndefined()
    expect(parseImgLine('? commands: mw <n> <value> | img')).toBeUndefined()
  })

  it('formats like the firmware and parses back', () => {
    const line = formatImgLine({ I: new Uint8Array([1, 0, 0xd2]), Q: new Uint8Array([0x11]), M: new Uint8Array([]) })
    expect(line).toBe('I: 1 0 D2  Q: 11  M:')
    expect([...parseImgLine(line)!.I]).toEqual([1, 0, 0xd2])
  })
})

describe('mw', () => {
  it('formats and masks to 16 bits', () => {
    expect(mwCommand(3, 257)).toBe('mw 3 257')
    expect(mwCommand(0, -1)).toBe('mw 0 65535')
    expect(mwCommand(1, 0x12345)).toBe('mw 1 9029')
    expect(() => mwCommand(-1, 0)).toThrow()
  })

  it('parses acks and faults', () => {
    expect(parseMwAck('ok %MW3 := 257\r')).toEqual({ n: 3, value: 257 })
    expect(parseMwAck('img')).toBeUndefined()
    expect(parseFaultLine('PLC STOP: fault 1 (division by zero) at Main')).toBe('fault 1 (division by zero) at Main')
  })
})

describe('bitForceWrite', () => {
  const M = new Uint8Array([0x00, 0x80, 0x00, 0x00, 0, 0, 0, 0])

  it('%MX0.0 is bit 0 of %MW0', () => {
    expect(bitForceWrite(M, addr('%MX0.0'), true)).toEqual({ n: 0, value: 0x8001 })
  })
  it('%MX2.0 is bit 0 of %MW1', () => {
    expect(bitForceWrite(M, addr('%MX2.0'), true)).toEqual({ n: 1, value: 1 })
  })
  it('%MX1.3 is bit 11 of %MW0 and keeps the other bits', () => {
    expect(bitForceWrite(M, addr('%MX1.3'), true)).toEqual({ n: 0, value: 0x8800 })
    expect(bitForceWrite(M, addr('%MX1.7'), false)).toEqual({ n: 0, value: 0 })
  })
  it('refuses words beyond the known bytes and non-%MX addresses', () => {
    expect(() => bitForceWrite(M, addr('%MX8.0'), true)).toThrow(/outside the 8 %M bytes/)
    expect(() => bitForceWrite(new Uint8Array(0), addr('%MX0.0'), true)).toThrow(/outside/)
    expect(() => bitForceWrite(M, addr('%QX0.0'), true)).toThrow(/not a %M bit/)
    expect(() => bitForceWrite(M, addr('%MW0'), true)).toThrow(/not a %M bit/)
  })
})

describe('OnlineSession against the fake Opta', () => {
  let fake: FakeOptaTransport
  let session: OnlineSession

  beforeEach(() => {
    vi.useFakeTimers()
    fake = new FakeOptaTransport()
    session = new OnlineSession(fake, ARDUINO_OPTA, { pollMs: 100, timeoutMs: 1000, maxMisses: 3 })
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  it('connects, polls img and decodes the image', async () => {
    fake.setInput(1, true)
    fake.setAnalog(2, 2500)
    fake.Q[0] = 0x11
    const states: string[] = []
    session.subscribe((s) => states.push(s.state))
    await session.connect()
    expect(session.state).toBe('connecting')
    await vi.advanceTimersByTimeAsync(1)
    expect(session.state).toBe('online')
    expect(states).toEqual(expect.arrayContaining(['connecting', 'online']))
    expect(session.image.read(addr('%IX0.0'))).toBe(true)
    expect(session.image.read(addr('%IX0.1'))).toBe(false)
    expect(session.image.read(addr('%IW2'), 'INT')).toBe(2500)
    expect(session.image.read(addr('%QX0.4'))).toBe(true)
    expect(session.knownM.length).toBe(8)

    fake.setInput(1, false)
    await vi.advanceTimersByTimeAsync(100)
    expect(session.image.read(addr('%IX0.0'))).toBe(false)
    expect(session.rxCount).toBeGreaterThanOrEqual(2)
    expect(fake.sent.filter((l) => l === 'img').length).toBe(session.rxCount)
  })

  it('forces a %M bit with a read-modify-write of its word', async () => {
    fake.M[2] = 0x40 // unrelated bit in %MW1
    await session.connect()
    await vi.advanceTimersByTimeAsync(1)
    const p = session.forceBit('%MX2.0', true)
    await vi.advanceTimersByTimeAsync(1)
    await p
    expect(fake.sent).toContain('mw 1 65')
    expect(fake.M[2]).toBe(0x41)
    expect(session.image.read(addr('%MX2.0'))).toBe(true)
    const q = session.forceBit('%MX2.0', false)
    await vi.advanceTimersByTimeAsync(1)
    await q
    expect(fake.M[2]).toBe(0x40)
    await expect(session.forceBit('%MX20.0', true)).rejects.toThrow(/outside/)
  })

  it('writes words', async () => {
    await session.connect()
    await vi.advanceTimersByTimeAsync(1)
    const p = session.writeWord(3, 1234)
    await vi.advanceTimersByTimeAsync(1)
    await p
    expect(fake.M[6] | (fake.M[7] << 8)).toBe(1234)
    expect(session.image.read(addr('%MW3'), 'INT')).toBe(1234)
  })

  it('goes to error when the device stops answering, and recovers', async () => {
    await session.connect()
    await vi.advanceTimersByTimeAsync(1)
    expect(session.state).toBe('online')
    fake.mute(true)
    await vi.advanceTimersByTimeAsync(2100)
    expect(session.state).toBe('online')
    await vi.advanceTimersByTimeAsync(1100)
    expect(session.state).toBe('error')
    expect(session.error).toBe('device not responding')
    fake.mute(false)
    await vi.advanceTimersByTimeAsync(1200)
    expect(session.state).toBe('online')
    expect(session.error).toBeUndefined()
  })

  it('handles an unplugged device and a deliberate disconnect', async () => {
    await session.connect()
    await vi.advanceTimersByTimeAsync(1)
    fake.simulateDisconnect()
    expect(session.state).toBe('disconnected')
    expect(session.error).toBe('device disconnected')
    const sentBefore = fake.sent.length
    await vi.advanceTimersByTimeAsync(500)
    expect(fake.sent.length).toBe(sentBefore)
    await expect(session.writeWord(0, 1)).rejects.toThrow(/not connected/)

    await session.connect()
    await vi.advanceTimersByTimeAsync(1)
    expect(session.state).toBe('online')
    await session.disconnect()
    expect(session.state).toBe('disconnected')
    expect(fake.isOpen).toBe(false)
  })

  it('surfaces fault messages and ignores noise', async () => {
    await session.connect()
    await vi.advanceTimersByTimeAsync(1)
    fake.emit('hello from setup()')
    fake.emit('PLC STOP: fault 1 (division by zero) at Main')
    expect(session.fault).toBe('fault 1 (division by zero) at Main')
    expect(session.state).toBe('online')
  })
})
