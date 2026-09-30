// SPDX-License-Identifier: MPL-2.0
//
// Device profiles are plain data: terminals, the process-image address each
// one maps to, and how to talk to the device. Adding a board means adding an
// object here.

export type PointDir = 'in' | 'out' | 'mem'
export type PointKind = 'digital' | 'analog' | 'register'

export interface IoPoint {
  /** Stable id within the profile. */
  id: string
  /** Terminal name printed on the device. */
  terminal: string
  label: string
  address: string
  type: string
  dir: PointDir
  kind: PointKind
  /** Raw range for analog points. */
  range?: [number, number]
  /** Group heading in the I/O lists. */
  group: string
}

export interface SerialConsole {
  kind: 'webserial-console'
  baudRate: number
  /** `img` prints `I: <hex bytes>  Q: <hex bytes>  M: <first N hex bytes>`. */
  imgCommand: string
  /** Bytes of %M the `img` line includes. */
  imgMBytes: number
  /** `mw <n> <value>` writes %MWn. */
  writeWordCommand: string
}

export interface DeviceProfile {
  id: string
  name: string
  vendor: string
  description: string
  imageSizes: { I: number; Q: number; M: number }
  points: IoPoint[]
  transport?: SerialConsole
}

const range = (n: number) => Array.from({ length: n }, (_, i) => i)

const OPTA_REGISTERS = 32

export const ARDUINO_OPTA: DeviceProfile = {
  id: 'arduino-opta',
  name: 'Arduino Opta',
  vendor: 'Arduino',
  description:
    'Opta running the plcc generic runtime (runtime.ino): 8 inputs, 4 relays, USER button and LED, Modbus RTU holding registers mapped to %MW.',
  imageSizes: { I: 18, Q: 1, M: OPTA_REGISTERS * 2 },
  transport: {
    kind: 'webserial-console',
    baudRate: 115200,
    imgCommand: 'img',
    imgMBytes: 8,
    writeWordCommand: 'mw',
  },
  points: [
    ...range(8).map<IoPoint>((i) => ({
      id: `I${i + 1}`,
      terminal: `I${i + 1}`,
      label: `Input ${i + 1}`,
      address: `%IX0.${i}`,
      type: 'BOOL',
      dir: 'in',
      kind: 'digital',
      group: 'Digital inputs',
    })),
    ...range(8).map<IoPoint>((i) => ({
      id: `A${i + 1}`,
      terminal: `I${i + 1}`,
      label: `Input ${i + 1} analog`,
      address: `%IW${i + 1}`,
      type: 'INT',
      dir: 'in',
      kind: 'analog',
      range: [0, 4095],
      group: 'Analog inputs (0..4095)',
    })),
    {
      id: 'BTN', terminal: 'USER', label: 'USER button', address: '%IX1.0', type: 'BOOL', dir: 'in', kind: 'digital',
      group: 'Digital inputs',
    },
    ...range(4).map<IoPoint>((i) => ({
      id: `R${i + 1}`,
      terminal: `R${i + 1}`,
      label: `Relay ${i + 1}`,
      address: `%QX0.${i}`,
      type: 'BOOL',
      dir: 'out',
      kind: 'digital',
      group: 'Relay outputs',
    })),
    {
      id: 'LED', terminal: 'LED', label: 'USER LED (blue)', address: '%QX0.4', type: 'BOOL', dir: 'out', kind: 'digital',
      group: 'Relay outputs',
    },
    ...range(8).map<IoPoint>((i) => ({
      id: `HR${i}`,
      terminal: `HR${i}`,
      label: `Holding register ${i}`,
      address: `%MW${i}`,
      type: 'INT',
      dir: 'mem',
      kind: 'register',
      group: `Modbus holding registers (HR0..HR${OPTA_REGISTERS - 1})`,
    })),
  ],
}

export const SIMULATOR: DeviceProfile = {
  id: 'simulator',
  name: 'Simulator',
  vendor: 'plcc studio',
  description: 'Virtual I/O for the in-browser preview simulator: 8 inputs, 4 analog inputs, 8 outputs.',
  imageSizes: { I: 16, Q: 4, M: 64 },
  points: [
    ...range(8).map<IoPoint>((i) => ({
      id: `DI${i}`, terminal: `DI${i}`, label: `Digital in ${i}`, address: `%IX0.${i}`, type: 'BOOL', dir: 'in',
      kind: 'digital', group: 'Digital inputs',
    })),
    ...range(4).map<IoPoint>((i) => ({
      id: `AI${i + 1}`, terminal: `AI${i + 1}`, label: `Analog in ${i + 1}`, address: `%IW${i + 1}`, type: 'INT',
      dir: 'in', kind: 'analog', range: [0, 4095], group: 'Analog inputs (0..4095)',
    })),
    ...range(8).map<IoPoint>((i) => ({
      id: `DO${i}`, terminal: `DO${i}`, label: `Digital out ${i}`, address: `%QX0.${i}`, type: 'BOOL', dir: 'out',
      kind: 'digital', group: 'Digital outputs',
    })),
  ],
}

export const PROFILES: DeviceProfile[] = [ARDUINO_OPTA, SIMULATOR]

export function getProfile(id: string): DeviceProfile {
  return PROFILES.find((p) => p.id === id) ?? SIMULATOR
}
