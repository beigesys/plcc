// SPDX-License-Identifier: MPL-2.0

import { catalogEntry, simulatorEntry } from '@/devices/catalog'
import { newId } from './ids'
import { newTag } from './ops'
import { parseRung } from './rungtext'
import type { Project, Rung } from './types'

function rung(comment: string, text: string): Rung {
  return { id: newId(), comment, ...parseRung(text) }
}

/** A catalog manifest as a project device: its ref and its file. */
function catalogDevice(id: string, name: string): Pick<Project, 'devices' | 'deviceFiles'> {
  const e = catalogEntry(id) ?? simulatorEntry()
  const path = `devices/${e.id}.toml`
  return { devices: [{ name, manifest: path }], deviceFiles: { [path]: e.text } }
}

/** The seeded demo: seal-in motor start, run timer, level alarm, on an Arduino Opta. */
export function demoProject(): Project {
  return {
    dialect: 'logix',
    name: 'Demo Opta',
    ...catalogDevice('arduino-opta', 'Opta'),
    tasks: [{ name: 'MainTask', interval_ms: 10, programs: ['MainProgram'] }],
    declarations: [],
    pous: [
      {
        id: newId(),
        name: 'MainProgram',
        kind: 'program',
        variables: [],
        routines: [
          {
            id: newId(),
            name: 'MainRoutine',
            rungs: [
              rung('Motor start/stop with seal-in', '[XIC(StartPB) ,XIC(Motor) ]XIO(StopPB)OTE(Motor);'),
              rung('Run timer: done 5 s after the motor starts', 'XIC(Motor)TON(RunTimer,5000,0);'),
              rung('High level alarm on the USER LED', 'GRT(Level,2000)OTE(High);'),
            ],
          },
        ],
      },
    ],
    globals: [
      newTag('StartPB', 'BOOL', { address: '%MX0.0', comment: 'Start pushbutton (Modbus HR0 bit 0)' }),
      newTag('StopPB', 'BOOL', { address: '%MX2.0', comment: 'Stop pushbutton (Modbus HR1 bit 0)' }),
      newTag('Motor', 'BOOL', { address: '%QX0.0', comment: 'Motor contactor, relay 1' }),
      newTag('Level', 'INT', { address: '%IW2', comment: 'Tank level, I2 analog 0..4095' }),
      newTag('High', 'BOOL', { address: '%QX0.4', comment: 'High level, USER LED' }),
      newTag('RunTimer', 'TIMER', { comment: 'Motor run timer' }),
    ],
  }
}

export function emptyProject(name: string): Project {
  return {
    dialect: 'logix',
    name,
    ...catalogDevice('simulator', 'Simulator'),
    tasks: [{ name: 'MainTask', interval_ms: 10, programs: ['MainProgram'] }],
    declarations: [],
    pous: [{ id: newId(), name: 'MainProgram', kind: 'program', variables: [], routines: [{ id: newId(), name: 'MainRoutine', rungs: [] }] }],
    globals: [],
  }
}
