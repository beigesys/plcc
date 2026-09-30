// SPDX-License-Identifier: MPL-2.0
//
// Draft ladder/project model for plcc studio.
//
// This is the ONLY module that defines the persisted shape of a project. The
// Rust `plcc-ladder` crate will later emit its own JSON schema; swapping to it
// should mean changing this file (plus `serialize.ts`) and nothing else. Every
// other module imports these types through `@/model`.

export type ContactKind = 'no' | 'nc' | 'rise' | 'fall'
export type CoilKind = 'normal' | 'negated' | 'set' | 'reset' | 'rise' | 'fall'

export const BOX_INSTRS = [
  'TON', 'TOF', 'RTO', 'CTU', 'CTD', 'RES', 'ONS', 'OSR',
  'EQU', 'NEQ', 'GRT', 'GEQ', 'LES', 'LEQ',
  'ADD', 'SUB', 'MUL', 'DIV', 'MOV', 'CPT', 'JSR',
] as const
export type BoxInstr = (typeof BOX_INSTRS)[number]

/** Instructions in a row: power flows left to right (AND). */
export interface Series {
  type: 'series'
  items: Element[]
}

/** Branches side by side: power out is the OR of every branch. */
export interface Parallel {
  type: 'parallel'
  id: string
  branches: Series[]
}

export interface Contact {
  type: 'contact'
  id: string
  kind: ContactKind
  tag: string
}

export interface Coil {
  type: 'coil'
  id: string
  kind: CoilKind
  tag: string
}

export interface Box {
  type: 'box'
  id: string
  instr: BoxInstr
  /** Operand name (see `instructions.ts`) to operand text: a tag, member, literal or expression. */
  operands: Record<string, string>
}

/** Inline Structured Text. Not simulated in this phase. */
export interface StBox {
  type: 'st'
  id: string
  code: string
}

export type Element = Series | Parallel | Contact | Coil | Box | StBox
/** Any element that has an id (everything but a bare series). */
export type Instruction = Parallel | Contact | Coil | Box | StBox

export interface Rung {
  id: string
  comment: string
  body: Series
}

export type RoutineKind = 'ladder' | 'st'

export interface Routine {
  name: string
  kind: RoutineKind
  /** Ladder routines. */
  rungs: Rung[]
  /** ST routines (source text). */
  st?: string
}

export interface Program {
  name: string
  routines: Routine[]
  /** Routine run every scan; others run through JSR. */
  main: string
}

export interface Tag {
  name: string
  /** IEC elementary type, or TIMER / COUNTER (Logix-style structures). */
  type: string
  initial: string
  /** Direct representation, e.g. `%IX0.0`, `%QX0.4`, `%MW3`. */
  address?: string
  comment: string
}

export interface DeviceRef {
  name: string
  /** Id of a profile in `src/devices/`. */
  profile: string
}

export interface Task {
  name: string
  intervalMs: number
  programs: string[]
}

export interface Project {
  name: string
  devices: DeviceRef[]
  tasks: Task[]
  programs: Program[]
  tags: Tag[]
}
