// SPDX-License-Identifier: MPL-2.0
import { describe, expect, it } from 'vitest'
import { Simulator, evaluateExpression, packTrace, parseLiteral, traceRoutine, unpackTrace } from '@/engine'
import { demoProject, flatten, parseAddress, parseRung } from '@/model'
import type { Project, Tag } from '@/model'
import { ARDUINO_OPTA, SIMULATOR } from '@/devices/profiles'

function tag(name: string, type: string, address?: string, initial = ''): Tag {
  return { name, type, initial, address, comment: '' }
}

function project(rungs: string[], tags: Tag[]): Project {
  return {
    name: 't',
    devices: [],
    tasks: [{ name: 'T', intervalMs: 10, programs: ['P'] }],
    programs: [
      {
        name: 'P',
        main: 'Main',
        routines: [
          { name: 'Main', kind: 'ladder', rungs: rungs.map((t, i) => ({ id: `r${i}`, comment: '', body: parseRung(t) })) },
        ],
      },
    ],
    tags,
  }
}

function sim(rungs: string[], tags: Tag[]): Simulator {
  return new Simulator(project(rungs, tags), SIMULATOR)
}

const run = (s: Simulator, n: number, dt = 10) => {
  for (let i = 0; i < n; i++) s.scan(dt)
}

describe('literals', () => {
  it('parses numbers, bases, booleans and durations', () => {
    expect(parseLiteral('5000')).toBe(5000)
    expect(parseLiteral('-2')).toBe(-2)
    expect(parseLiteral('1.5')).toBe(1.5)
    expect(parseLiteral('16#FF')).toBe(255)
    expect(parseLiteral('2#1010')).toBe(10)
    expect(parseLiteral('TRUE')).toBe(true)
    expect(parseLiteral('false')).toBe(false)
    expect(parseLiteral('T#5s')).toBe(5000)
    expect(parseLiteral('T#250ms')).toBe(250)
    expect(parseLiteral('T#1m30s')).toBe(90000)
    expect(parseLiteral('INT#70000')).toBe(4464)
    expect(parseLiteral('Motor')).toBeUndefined()
  })
})

describe('seal-in (demo project)', () => {
  it('latches on a start pulse and drops on stop', () => {
    const s = new Simulator(demoProject(), ARDUINO_OPTA)
    const startPB = parseAddress('%MX0.0')!
    s.scan(10)
    expect(s.tags.read('Motor')).toBe(false)

    s.image.write(startPB, true)
    s.scan(10)
    expect(s.tags.read('Motor')).toBe(true)
    expect(s.image.Q[0] & 1).toBe(1)

    s.image.write(startPB, false)
    run(s, 3)
    expect(s.tags.read('Motor')).toBe(true)

    s.tags.write('StopPB', true)
    expect(s.image.M[2] & 1).toBe(1)
    s.scan(10)
    expect(s.tags.read('Motor')).toBe(false)
    s.tags.write('StopPB', false)
    run(s, 2)
    expect(s.tags.read('Motor')).toBe(false)
  })

  it('traces power through the parallel branch', () => {
    const p = demoProject()
    const s = new Simulator(p, ARDUINO_OPTA)
    s.tags.write('StartPB', true)
    s.scan(10)
    s.tags.write('StartPB', false)
    s.scan(10)
    const [par, start, motorC, stop, coil] = flatten(p.programs[0].routines[0].rungs[0].body)
    expect(s.trace.get(par.id)).toEqual({ in: true, out: true, active: true })
    expect(s.trace.get(start.id)).toEqual({ in: true, out: false, active: false })
    expect(s.trace.get(motorC.id)).toEqual({ in: true, out: true, active: true })
    expect(s.trace.get(stop.id)).toEqual({ in: true, out: true, active: true })
    expect(s.trace.get(coil.id)).toEqual({ in: true, out: true, active: true })
    // Every element of every rung is traced.
    for (const r of p.programs[0].routines[0].rungs) for (const e of flatten(r.body)) expect(s.trace.has(e.id)).toBe(true)
    expect(unpackTrace(s.snapshotTrace())).toEqual(s.trace)
    expect(packTrace(s.trace).find(([id]) => id === start.id)?.[1]).toBe(1)
  })

  it('GRT on Level (%IW2) via the image', () => {
    const s = new Simulator(demoProject(), ARDUINO_OPTA)
    s.image.write(parseAddress('%IW2')!, 1500, 'INT')
    s.scan(10)
    expect(s.tags.read('High')).toBe(false)
    s.image.write(parseAddress('%IW2')!, 2500, 'INT')
    s.scan(10)
    expect(s.tags.read('High')).toBe(true)
    expect(s.image.Q[0] & 0x10).toBe(0x10)
  })
})

describe('timers', () => {
  const tags = [tag('Go', 'BOOL'), tag('T', 'TIMER')]

  it('TON: done exactly at the preset, TT while timing, reset when false', () => {
    const s = sim(['XIC(Go)TON(T,5000,0);'], tags)
    s.tags.write('Go', true)
    run(s, 499)
    expect(s.tags.read('T.ACC')).toBe(4990)
    expect(s.tags.read('T.DN')).toBe(false)
    expect(s.tags.read('T.TT')).toBe(true)
    expect(s.tags.read('T.EN')).toBe(true)
    s.scan(10)
    expect(s.tags.read('T.DN')).toBe(true)
    expect(s.tags.read('T.TT')).toBe(false)
    expect(s.tags.read('T.ACC')).toBe(5000)
    expect(s.tags.read('T.PRE')).toBe(5000)
    run(s, 5)
    expect(s.tags.read('T.ACC')).toBe(5000)
    s.tags.write('Go', false)
    s.scan(10)
    expect(s.tags.read('T.DN')).toBe(false)
    expect(s.tags.read('T.ACC')).toBe(0)
    expect(s.tags.read('T.EN')).toBe(false)
  })

  it('TON: the demo RunTimer finishes 5 s after the motor starts', () => {
    const s = new Simulator(demoProject(), ARDUINO_OPTA)
    s.tags.write('StartPB', true)
    run(s, 499)
    expect(s.tags.read('RunTimer.DN')).toBe(false)
    s.scan(10)
    expect(s.tags.read('RunTimer.DN')).toBe(true)
  })

  it('TOF: DN on while true, times out after going false', () => {
    const s = sim(['XIC(Go)TOF(T,100,0);'], tags)
    s.scan(10)
    expect(s.tags.read('T.DN')).toBe(false)
    s.tags.write('Go', true)
    s.scan(10)
    expect(s.tags.read('T.DN')).toBe(true)
    s.tags.write('Go', false)
    run(s, 9)
    expect(s.tags.read('T.DN')).toBe(true)
    expect(s.tags.read('T.TT')).toBe(true)
    s.scan(10)
    expect(s.tags.read('T.DN')).toBe(false)
    expect(s.tags.read('T.TT')).toBe(false)
  })

  it('RTO retains ACC when false; RES clears it', () => {
    const s = sim(['XIC(Go)RTO(T,100,0);', 'XIC(Clr)RES(T);'], [...tags, tag('Clr', 'BOOL')])
    s.tags.write('Go', true)
    run(s, 5)
    s.tags.write('Go', false)
    run(s, 5)
    expect(s.tags.read('T.ACC')).toBe(50)
    s.tags.write('Go', true)
    run(s, 5)
    expect(s.tags.read('T.DN')).toBe(true)
    s.tags.write('Go', false)
    run(s, 3)
    expect(s.tags.read('T.DN')).toBe(true)
    s.tags.write('Clr', true)
    s.scan(10)
    expect(s.tags.read('T.ACC')).toBe(0)
    expect(s.tags.read('T.DN')).toBe(false)
  })
})

describe('bit instructions', () => {
  it('ONS passes exactly one scan per rising edge', () => {
    const s = sim(['XIC(Go)ONS(Stor)ADD(N,1,N);'], [tag('Go', 'BOOL'), tag('Stor', 'BOOL'), tag('N', 'DINT')])
    run(s, 3)
    expect(s.tags.read('N')).toBe(0)
    s.tags.write('Go', true)
    run(s, 10)
    expect(s.tags.read('N')).toBe(1)
    s.tags.write('Go', false)
    run(s, 2)
    s.tags.write('Go', true)
    run(s, 4)
    expect(s.tags.read('N')).toBe(2)
  })

  it('OSR sets its output for one scan', () => {
    const s = sim(['XIC(Go)OSR(Stor,Pulse);'], [tag('Go', 'BOOL'), tag('Stor', 'BOOL'), tag('Pulse', 'BOOL')])
    s.tags.write('Go', true)
    s.scan(10)
    expect(s.tags.read('Pulse')).toBe(true)
    s.scan(10)
    expect(s.tags.read('Pulse')).toBe(false)
  })

  it('OTL / OTU latch and unlatch', () => {
    const s = sim(['XIC(S)OTL(Q);', 'XIC(R)OTU(Q);'], [tag('S', 'BOOL'), tag('R', 'BOOL'), tag('Q', 'BOOL')])
    s.tags.write('S', true)
    s.scan(10)
    s.tags.write('S', false)
    run(s, 3)
    expect(s.tags.read('Q')).toBe(true)
    s.tags.write('R', true)
    s.scan(10)
    expect(s.tags.read('Q')).toBe(false)
  })

  it('edge contacts and edge / negated coils', () => {
    const s = sim(
      ['XICR(A)ADD(Up,1,Up);', 'XICF(A)ADD(Dn,1,Dn);', 'XIC(A)OTER(P);', 'XIC(A)OTEF(F);', 'XIC(A)OTEN(Nq);'],
      [tag('A', 'BOOL'), tag('Up', 'DINT'), tag('Dn', 'DINT'), tag('P', 'BOOL'), tag('F', 'BOOL'), tag('Nq', 'BOOL')],
    )
    s.scan(10)
    expect(s.tags.read('Nq')).toBe(true)
    s.tags.write('A', true)
    s.scan(10)
    expect(s.tags.read('P')).toBe(true)
    expect(s.tags.read('Nq')).toBe(false)
    run(s, 3)
    expect(s.tags.read('P')).toBe(false)
    s.tags.write('A', false)
    s.scan(10)
    expect(s.tags.read('F')).toBe(true)
    s.scan(10)
    expect(s.tags.read('F')).toBe(false)
    expect(s.tags.read('Up')).toBe(1)
    expect(s.tags.read('Dn')).toBe(1)
  })
})

describe('counters', () => {
  it('CTU counts rising edges and sets DN; RES clears', () => {
    const s = sim(['XIC(Go)CTU(C,3,0);', 'XIC(Clr)RES(C);'], [tag('Go', 'BOOL'), tag('Clr', 'BOOL'), tag('C', 'COUNTER')])
    for (let i = 0; i < 3; i++) {
      s.tags.write('Go', true)
      run(s, 3)
      s.tags.write('Go', false)
      run(s, 2)
      expect(s.tags.read('C.ACC')).toBe(i + 1)
    }
    expect(s.tags.read('C.DN')).toBe(true)
    s.tags.write('Clr', true)
    s.scan(10)
    expect(s.tags.read('C.ACC')).toBe(0)
    expect(s.tags.read('C.DN')).toBe(false)
  })

  it('CTD counts down', () => {
    const s = sim(['XIC(Go)CTD(C,0,0);'], [tag('Go', 'BOOL'), tag('C', 'COUNTER')])
    s.tags.write('Go', true)
    s.scan(10)
    expect(s.tags.read('C.ACC')).toBe(-1)
    expect(s.tags.read('C.DN')).toBe(false)
  })
})

describe('compare and math', () => {
  it('compares gate power', () => {
    const tags = [tag('A', 'DINT', undefined, '5'), tag('Q1', 'BOOL'), tag('Q2', 'BOOL'), tag('Q3', 'BOOL')]
    const s = sim(['EQU(A,5)OTE(Q1);', 'LES(A,5)OTE(Q2);', 'GEQ(A,5)NEQ(A,4)LEQ(A,5)OTE(Q3);'], tags)
    s.scan(10)
    expect([s.tags.read('Q1'), s.tags.read('Q2'), s.tags.read('Q3')]).toEqual([true, false, true])
  })

  it('math writes only when enabled, truncates integers, and reports divide by zero', () => {
    const tags = [
      tag('En', 'BOOL'), tag('A', 'DINT', undefined, '7'), tag('Z', 'DINT'), tag('Q', 'DINT', undefined, '99'),
      tag('R', 'REAL'), tag('M', 'DINT'), tag('C', 'DINT'),
    ]
    const s = sim(
      ['XIC(En)DIV(A,2,M);', 'XIC(En)DIV(A,2.0,R);', 'XIC(En)DIV(A,Z,Q);', 'XIC(En)CPT(C,(A+3)*2 MOD 7);', 'XIC(En)MOV(A,Z);'],
      tags,
    )
    s.scan(10)
    expect(s.tags.read('M')).toBe(0)
    s.tags.write('En', true)
    s.scan(10)
    expect(s.tags.read('M')).toBe(3)
    expect(s.tags.read('R')).toBe(3.5)
    expect(s.tags.read('Q')).toBe(99)
    expect([...s.errors.values()]).toContain('divide by zero')
    expect(s.tags.read('C')).toBe(6)
    expect(s.tags.read('Z')).toBe(7)
  })

  it('wraps integer types', () => {
    const s = sim(['ADD(A,1,A);'], [tag('A', 'INT', undefined, '32767')])
    s.scan(10)
    expect(s.tags.read('A')).toBe(-32768)
  })

  it('records unknown tags and ST boxes as errors', () => {
    const p = project(['XIC(Nope)OTE(Q);', 'ST("x := 1;");'], [tag('Q', 'BOOL')])
    const s = new Simulator(p, SIMULATOR)
    s.scan(10)
    const msgs = [...s.errors.values()]
    expect(msgs).toContain('unknown tag Nope')
    expect(msgs).toContain('ST box not simulated')
  })

  it('JSR runs another routine of the program', () => {
    const p = project(['XIC(Go)JSR(Sub);', 'JSR(Missing);'], [tag('Go', 'BOOL'), tag('N', 'DINT')])
    p.programs[0].routines.push({
      name: 'Sub', kind: 'ladder', rungs: [{ id: 's0', comment: '', body: parseRung('ADD(N,1,N);') }],
    })
    const s = new Simulator(p, SIMULATOR)
    s.scan(10)
    expect(s.tags.read('N')).toBe(0)
    s.tags.write('Go', true)
    run(s, 3)
    expect(s.tags.read('N')).toBe(3)
    expect([...s.errors.values()]).toContain('JSR: no routine Missing')
  })
})

describe('forces', () => {
  it('a forced input overrides the image; a forced output ignores writes', () => {
    const s = sim(['XIC(In)OTE(Out);'], [tag('In', 'BOOL', '%IX0.0'), tag('Out', 'BOOL', '%QX0.0')])
    s.tags.force('In', true)
    s.scan(10)
    expect(s.tags.read('Out')).toBe(true)
    expect(s.image.I[0] & 1).toBe(1)
    s.tags.force('Out', false)
    s.scan(10)
    expect(s.tags.read('Out')).toBe(false)
    expect(s.image.Q[0] & 1).toBe(0)
    s.tags.unforce('Out')
    s.scan(10)
    expect(s.tags.read('Out')).toBe(true)
    s.tags.unforce('In')
    s.image.I[0] = 0
    s.scan(10)
    expect(s.tags.read('Out')).toBe(false)
  })
})

describe('tag store', () => {
  it('reads bits of integers, keeps values across sync, and snapshots plain data', () => {
    const p = project([], [tag('W', 'INT', '%MW1', '5'), tag('T', 'TIMER')])
    const s = new Simulator(p, SIMULATOR)
    expect(s.tags.read('W')).toBe(5)
    expect(s.tags.read('w.0')).toBe(true)
    expect(s.tags.read('W.1')).toBe(false)
    s.tags.write('W.1', true)
    expect(s.tags.read('W')).toBe(7)
    s.tags.write('T.ACC', 42)
    s.setProject({ ...p, tags: [...p.tags, tag('New', 'BOOL')] })
    expect(s.tags.read('T.ACC')).toBe(42)
    expect(s.tags.read('New')).toBe(false)
    const snap = s.tags.snapshot()
    expect(snap).toEqual({ W: 7, T: { PRE: 0, ACC: 42, EN: false, TT: false, DN: false }, New: false })
    expect(structuredClone(snap)).toEqual(snap)
    s.reset()
    expect(s.tags.read('W')).toBe(5)
    expect(s.tags.read('T.ACC')).toBe(0)
  })
})

describe('traceRoutine (online mode)', () => {
  it('computes power flow from values without writing anything', () => {
    const p = demoProject()
    const s = new Simulator(p, ARDUINO_OPTA)
    s.tags.write('Motor', true)
    const before = JSON.stringify(s.tags.snapshot())
    const routine = p.programs[0].routines[0]
    const trace = traceRoutine(routine, (ref) => s.tags.read(ref))
    expect(JSON.stringify(s.tags.snapshot())).toBe(before)
    const [par, , , stop, coil] = flatten(routine.rungs[0].body)
    expect(trace.get(par.id)?.out).toBe(true)
    expect(trace.get(stop.id)?.active).toBe(true)
    expect(trace.get(coil.id)).toEqual({ in: true, out: true, active: true })
    const [grt] = flatten(routine.rungs[2].body)
    expect(trace.get(grt.id)?.active).toBe(false)
  })
})

describe('scan budget', () => {
  it('aborts an over-budget scan and flags the overrun', () => {
    const rungs = Array.from({ length: 20000 }, () => 'XIC(A)XIO(B)[XIC(C) ,XIC(D) ]ADD(N,1,N);')
    const s = sim(rungs, [tag('A', 'BOOL', undefined, 'TRUE'), tag('B', 'BOOL'), tag('C', 'BOOL', undefined, 'TRUE'), tag('D', 'BOOL'), tag('N', 'DINT')])
    s.scan(10, { budgetMs: 1 })
    expect(s.overrun).toBe(true)
    expect(s.overrunCount).toBe(1)
    expect(s.tags.read('N')).toBeLessThan(20000)
    expect(s.lastScanMs).toBeLessThan(50)

    const small = sim(['XIC(A)OTE(B);'], [tag('A', 'BOOL'), tag('B', 'BOOL')])
    small.scan(10, { budgetMs: 1000 })
    expect(small.overrun).toBe(false)
  })

  it('uses the injected clock', () => {
    let t = 0
    const s = sim(['ADD(N,1,N);', 'ADD(N,1,N);', 'ADD(N,1,N);'], [tag('N', 'DINT')])
    s.scan(10, { budgetMs: 1, now: () => (t += 0.6) })
    expect(s.overrun).toBe(true)
    expect(s.tags.read('N')).toBe(1)
  })
})

describe('CPT expressions', () => {
  const vals: Record<string, number | boolean> = { a: 3, b: 4, flag: true, 't.acc': 250 }
  const read = (r: string) => vals[r.toLowerCase()]
  it.each([
    ['1 + 2 * 3', 7],
    ['(1 + 2) * 3', 9],
    ['2 ** 3 ** 2', 512],
    ['-A + B', 1],
    ['SQRT(A ** 2 + B ** 2)', 5],
    ['10 MOD 4', 2],
    ['7 / 2', 3.5],
    ['7.0 / 2', 3.5],
    ['A < B AND Flag', 1],
    ['NOT Flag OR A = 3', 1],
    ['A <> 3', 0],
    ['16#F0 AND 16#3C', 0x30],
    ['5 XOR 3', 6],
    ['T.ACC + T#1s', 1250],
    ['ABS(-2.5) + TRUNC(1.9)', 3.5],
  ])('%s = %d', (src, want) => {
    expect(evaluateExpression(src, read)).toBe(want)
  })
  it('rejects bad input', () => {
    expect(() => evaluateExpression('1 +', read)).toThrow()
    expect(() => evaluateExpression('(1', read)).toThrow(/expected/)
    expect(() => evaluateExpression('Nope + 1', read)).toThrow(/unknown tag Nope/)
    expect(() => evaluateExpression('FOO(1)', read)).toThrow(/unknown function/)
    expect(() => evaluateExpression('1 / 0', read)).toThrow(/divide by zero/)
    expect(() => evaluateExpression('1 2', read)).toThrow()
  })
})
