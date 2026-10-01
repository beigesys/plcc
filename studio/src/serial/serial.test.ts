// SPDX-License-Identifier: MPL-2.0
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { catalogEntry } from '@/devices/catalog'
import { parseAddress } from '@/model'
import type { Address } from '@/model'
import {
  FakeOptaTransport, OnlineSession, bitForceWrite, checkIdentity, describeProgram, formatImgLine, formatInfoLine, identifyDevice, matchCatalog, mwCommand,
  parseFaultLine, parseImgLine, parseInfoLine, parseMwAck,
} from '@/serial'

const ARDUINO_OPTA = catalogEntry('arduino-opta')!.device
const SIMULATOR = catalogEntry('simulator')!.device
/** The Opta as manifest version 1 described it: the linked runtime, no program commands. */
const OPTA_V1 = {
  ...ARDUINO_OPTA,
  device: { ...ARDUINO_OPTA.device, version: 1 },
  console: { ...ARDUINO_OPTA.console!, commands: ['info', 'img', 'mw'] as ('info' | 'img' | 'mw')[] },
}

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
    expect(session.knownM.length).toBe(64)

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
    await expect(session.forceBit('%MX64.0', true)).rejects.toThrow(/outside/)
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

describe('info: which device is on the port', () => {
  afterEach(() => {
    vi.useRealTimers()
  })

  it('formats and parses the runtime info line', () => {
    const line = formatInfoLine(ARDUINO_OPTA)
    expect(line).toBe('{"device":"arduino-opta","manifest":2,"runtime":"plcc-arduino","abi":1,"image":{"I":18,"Q":1,"M":64}}')
    expect(parseInfoLine(line)).toEqual({ device: 'arduino-opta', manifest: 2, runtime: 'plcc-arduino', abi: 1, image: { I: 18, Q: 1, M: 64 } })
    expect(parseInfoLine('I: 0  Q: 0  M: 0')).toBeUndefined()
    expect(parseInfoLine('{"device":"x"}')).toBeUndefined()
    expect(parseInfoLine('{nope')).toBeUndefined()
  })

  it('compares an identity with a manifest', () => {
    const id = parseInfoLine(formatInfoLine(ARDUINO_OPTA))!
    expect(checkIdentity(id, ARDUINO_OPTA)).toEqual({ mismatch: [], notes: [] })
    const sim = checkIdentity(id, SIMULATOR)
    expect(sim.mismatch.join(' ')).toMatch(/connected device is "arduino-opta", but the project's device is "simulator"/)
    const older = checkIdentity({ ...id, manifest: 0 }, ARDUINO_OPTA)
    expect(older.mismatch).toEqual([])
    expect(older.notes[0]).toMatch(/built for manifest version 0/)
    expect(checkIdentity({ ...id, abi: 2 }, ARDUINO_OPTA).mismatch[0]).toMatch(/ABI 2/)
  })

  it('identifies the device when Online connects', async () => {
    vi.useFakeTimers()
    const fake = new FakeOptaTransport()
    const session = new OnlineSession(fake, ARDUINO_OPTA, { pollMs: 100 })
    await session.connect()
    await vi.advanceTimersByTimeAsync(5)
    expect(fake.sent[0]).toBe('info')
    expect(session.identity?.device).toBe('arduino-opta')
    expect(session.identityCheck).toEqual({ mismatch: [], notes: [] })
    expect(session.state).toBe('online')
    await session.disconnect()
  })

  it('warns when the device is not the project device', async () => {
    vi.useFakeTimers()
    const fake = new FakeOptaTransport()
    fake.infoReply = formatInfoLine({ ...ARDUINO_OPTA, device: { ...ARDUINO_OPTA.device, id: 'other-board' } })
    const session = new OnlineSession(fake, ARDUINO_OPTA, { pollMs: 100 })
    await session.connect()
    await vi.advanceTimersByTimeAsync(5)
    expect(session.identityCheck?.mismatch[0]).toMatch(/"other-board"/)
    // Polling goes on: a warning, not a refusal.
    expect(session.state).toBe('online')
    await session.disconnect()
  })

  it('notes a runtime without info and keeps working', async () => {
    vi.useFakeTimers()
    const fake = new FakeOptaTransport()
    fake.infoReply = null
    const session = new OnlineSession(fake, ARDUINO_OPTA, { pollMs: 100 })
    await session.connect()
    await vi.advanceTimersByTimeAsync(5)
    expect(session.identity).toBeUndefined()
    expect(session.identityCheck?.notes[0]).toMatch(/did not identify itself/)
    expect(session.state).toBe('online')
    await session.disconnect()
  })

  it('does not send info to a device whose manifest lacks it', async () => {
    vi.useFakeTimers()
    const noInfo = { ...ARDUINO_OPTA, console: { ...ARDUINO_OPTA.console!, commands: ['img', 'mw'] as ('img' | 'mw')[] } }
    const fake = new FakeOptaTransport(noInfo)
    const session = new OnlineSession(fake, noInfo, { pollMs: 100 })
    await session.connect()
    await vi.advanceTimersByTimeAsync(5)
    expect(fake.sent).not.toContain('info')
    await session.disconnect()
  })

  it('detects a device and matches the catalog', async () => {
    const fake = new FakeOptaTransport()
    const id = await identifyDevice(fake, 500)
    expect(id.device).toBe('arduino-opta')
    expect(fake.isOpen).toBe(false)
    const det = matchCatalog(id)
    expect(det.entry?.id).toBe('arduino-opta')
    expect(det.check?.mismatch).toEqual([])
    const old = new FakeOptaTransport()
    old.infoReply = null
    await expect(identifyDevice(old, 500)).rejects.toThrow(/does not know "info"/)
    expect(matchCatalog({ ...id, device: 'unknown-board' }).entry).toBeUndefined()
  })
})

/** Lines recorded from an Opta running the generic runtime (chaser_io.st), 2026-09-30. */
const HW_INFO = '{"device":"arduino-opta","manifest":1,"runtime":"plcc-arduino","abi":1,"image":{"I":18,"Q":1,"M":64}}'
const HW_IMG =
  'I: 0 0 2 0 0 0 2 0 0 0 0 0 0 0 3 0 0 0  Q: 12  M: 1 0 E8 3' + ' 0'.repeat(60)
const HW_HELP = '? commands: info | img | mw <n> <value>'

describe('the real runtime', () => {
  it('prints exactly what the fake console prints', () => {
    expect(formatInfoLine(OPTA_V1)).toBe(HW_INFO)
    const f = parseImgLine(HW_IMG)!
    expect(formatImgLine(f)).toBe(HW_IMG)
    expect(new FakeOptaTransport(OPTA_V1)['handle']('bogus')).toBe(HW_HELP)
    // The program-image runtime (runtimes/arduino-opta/loader/loader.ino).
    expect(new FakeOptaTransport()['handle']('bogus')).toBe('? commands: info | img | mw <n> <value> | prog | stop | run')
  })

  it('decodes a recorded info and img against the catalog manifest', () => {
    const det = matchCatalog(parseInfoLine(HW_INFO)!)
    expect(det.entry?.id).toBe('arduino-opta')
    // Recorded from the linked runtime of manifest version 1: same image, an older manifest.
    expect(det.check?.mismatch).toEqual([])
    expect(det.check?.notes.join(' ')).toMatch(/built for manifest version 1; the project has version 2/)
    const f = parseImgLine(HW_IMG)!
    expect([f.I.length, f.Q.length, f.M.length]).toEqual([18, 1, 64])
    // chaser_io.st: chase on (%MX0.0), period 1000 ms (%MW1), relay 2 and the LED lit.
    expect(f.M[0] & 1).toBe(1)
    expect(f.M[2] | (f.M[3] << 8)).toBe(1000)
    expect(f.Q[0]).toBe(0x12)
  })
})

describe('program-image runtimes: program state, stop and run', () => {
  afterEach(() => {
    vi.useRealTimers()
  })

  it('parses the program state from info, as the loader prints it', () => {
    const empty = parseInfoLine('{"device":"arduino-opta","manifest":2,"runtime":"plcc-arduino","abi":1,"image":{"I":18,"Q":1,"M":64},"state":"empty","program":null,"reason":"empty slot"}')!
    expect(empty.program).toEqual({ state: 'empty', image: null, reason: 'empty slot' })
    expect(describeProgram(empty.program!)).toBe('no program (empty slot)')
    const fault = parseInfoLine(
      '{"device":"arduino-opta","manifest":2,"runtime":"plcc-arduino","abi":1,"image":{"I":18,"Q":1,"M":64},"state":"fault","program":{"build":"59f1e2d3c4b5a6978877665544332211","size":1632,"crc":"1a2b3c4d","version":2},"fault":{"code":1,"where":"project.json:45:24: lx__P_Main","pc":"0x8180123"}}',
    )!
    expect(fault.program?.image?.size).toBe(1632)
    expect(fault.program?.fault).toEqual({ code: 1, where: 'project.json:45:24: lx__P_Main', pc: '0x8180123' })
    expect(describeProgram(fault.program!)).toBe('program faulted: division by zero at project.json:45:24: lx__P_Main')
  })

  it('stops and runs the program over the console, re-reading info', async () => {
    vi.useFakeTimers()
    const fake = new FakeOptaTransport()
    fake.loadProgram({ build: 'ab'.repeat(16), size: 1024, crc: '01020304', version: 2 })
    const session = new OnlineSession(fake, ARDUINO_OPTA)
    await session.connect()
    await vi.advanceTimersByTimeAsync(5)
    expect(session.hasProgramCommands).toBe(true)
    expect(session.identity?.program?.state).toBe('run')
    const stop = session.stopProgram()
    await vi.advanceTimersByTimeAsync(5)
    expect(await stop).toBe('ok stop')
    expect(session.identity?.program?.state).toBe('stop')
    const run = session.runProgram()
    await vi.advanceTimersByTimeAsync(5)
    expect(await run).toBe('ok run')
    expect(session.identity?.program?.state).toBe('run')
    // An empty slot cannot run.
    fake.program = { state: 'empty', image: null, reason: 'empty slot' }
    const bad = session.runProgram()
    const caught = bad.catch((e: Error) => e.message)
    await vi.advanceTimersByTimeAsync(5)
    expect(await caught).toBe('empty slot')
    await session.disconnect()
  })
})
