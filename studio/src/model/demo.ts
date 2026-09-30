// SPDX-License-Identifier: MPL-2.0

import { newId } from './ids'
import { parseRung } from './rungtext'
import type { Project, Rung } from './types'

function rung(comment: string, text: string): Rung {
  return { id: newId('r'), comment, body: parseRung(text) }
}

/** The seeded demo: seal-in motor start, run timer, level alarm, on an Arduino Opta. */
export function demoProject(): Project {
  return {
    name: 'Demo Opta',
    devices: [{ name: 'Opta', profile: 'arduino-opta' }],
    tasks: [{ name: 'MainTask', intervalMs: 10, programs: ['MainProgram'] }],
    programs: [
      {
        name: 'MainProgram',
        main: 'MainRoutine',
        routines: [
          {
            name: 'MainRoutine',
            kind: 'ladder',
            rungs: [
              rung('Motor start/stop with seal-in', '[XIC(StartPB) ,XIC(Motor) ]XIO(StopPB)OTE(Motor);'),
              rung('Run timer: done 5 s after the motor starts', 'XIC(Motor)TON(RunTimer,5000,0);'),
              rung('High level alarm on the USER LED', 'GRT(Level,2000)OTE(High);'),
            ],
          },
        ],
      },
    ],
    tags: [
      { name: 'StartPB', type: 'BOOL', initial: 'FALSE', address: '%MX0.0', comment: 'Start pushbutton (Modbus HR0 bit 0)' },
      { name: 'StopPB', type: 'BOOL', initial: 'FALSE', address: '%MX2.0', comment: 'Stop pushbutton (Modbus HR1 bit 0)' },
      { name: 'Motor', type: 'BOOL', initial: 'FALSE', address: '%QX0.0', comment: 'Motor contactor, relay 1' },
      { name: 'Level', type: 'INT', initial: '0', address: '%IW2', comment: 'Tank level, I2 analog 0..4095' },
      { name: 'High', type: 'BOOL', initial: 'FALSE', address: '%QX0.4', comment: 'High level, USER LED' },
      { name: 'RunTimer', type: 'TIMER', initial: '', comment: 'Motor run timer' },
    ],
  }
}

export function emptyProject(name: string): Project {
  return {
    name,
    devices: [{ name: 'Simulator', profile: 'simulator' }],
    tasks: [{ name: 'MainTask', intervalMs: 10, programs: ['MainProgram'] }],
    programs: [{ name: 'MainProgram', main: 'MainRoutine', routines: [{ name: 'MainRoutine', kind: 'ladder', rungs: [] }] }],
    tags: [],
  }
}
