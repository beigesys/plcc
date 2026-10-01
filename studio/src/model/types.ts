// SPDX-License-Identifier: MPL-2.0
//
// The project model: plcc's ladder model (crate `plcc-ladder`,
// crates/plcc-ladder/src/model.rs, docs/ladder-translation.md) as TypeScript,
// field for field, plus what only the studio keeps (the devices).
//
// A project's ladder part is saved as the model's JSON (`project.json`) and
// is exactly what plcc reads: `plcc convert project.json --to l5x`, and the
// browser front end and compiler take it as an input. Studio projects use
// the Logix dialect: Rockwell instructions (XIC, OTE, TON(Timer, Preset,
// Accum), ...), TIMER / COUNTER tags.

/** Element identifier, unique in a project. */
export type Id = number

/** Whose ladder semantics the model follows. */
export type Dialect = 'iec' | 'logix'

export type ContactKind = 'no' | 'nc' | 'rising' | 'falling'
export type CoilKind = 'normal' | 'negated' | 'set' | 'reset' | 'rising' | 'falling'
export type PinDir = 'input' | 'output' | 'in_out'
export type PouKind = 'program' | 'function_block' | 'function'
export type VarSection = 'local' | 'input' | 'output' | 'in_out' | 'external' | 'temp' | 'global'

export interface Contact {
  type: 'contact'
  id: Id
  operand: string
  kind: ContactKind
  /** Translation warnings, dialect differences. */
  notes?: string[]
}

export interface Coil {
  type: 'coil'
  id: Id
  operand: string
  kind: CoilKind
  notes?: string[]
}

/** Parallel legs, top to bottom; each a series. Their results are ORed where they join. */
export interface Branch {
  type: 'branch'
  id: Id
  legs: Element[][]
}

export interface Pin {
  /** Logix operand name (`Timer`, `Source A`); Logix operands are positional, in pin order. */
  name: string
  dir?: PinDir
  /** The operand text: a tag, member, immediate or expression. Absent: `?`. */
  value?: string
  negated?: boolean
  /** IEC only: a second power-flow path of the pin. */
  rung?: Element[]
}

/** An instruction box (Logix) or function block / function (IEC). */
export interface Block {
  type: 'block'
  id: Id
  /** Instruction name: `TON`, `GRT`, `CPT`. */
  name: string
  /** IEC function block instance; unused in Logix. */
  instance?: string
  pins: Pin[]
  power_in?: string
  power_out?: string
  notes?: string[]
}

export interface Jump {
  type: 'jump'
  id: Id
  label: string
}

export interface Return {
  type: 'return'
  id: Id
}

/** Structured Text statements run when the rung is powered; the power passes through. */
export interface StBox {
  type: 'st'
  id: Id
  code: string
  notes?: string[]
}

export type Element = Contact | Coil | Branch | Block | Jump | Return | StBox

export interface Rung {
  id: Id
  comment?: string
  /** A jump target (Logix `LBL` as the rung's first instruction). */
  label?: string
  /** The rung, left to right. */
  elements: Element[]
}

export interface Routine {
  id: Id
  name: string
  rungs: Rung[]
}

export interface Variable {
  name: string
  /** Logix data type: `BOOL`, `DINT`, `TIMER`, `DINT[10]`. */
  data_type: string
  section: VarSection
  initial?: string
  /** Direct address in the process image: `%IX0.0`, `%QX0.4`, `%MW3`. */
  address?: string
  comment?: string
  constant?: boolean
  retain?: boolean
}

export interface Pou {
  id: Id
  name: string
  kind: PouKind
  return_type?: string
  /** Program tags. */
  variables: Variable[]
  /** The first routine is the main routine; the others run through JSR. */
  routines: Routine[]
  /** METHODs / PROPERTYs / ACTIONs as ST (carried along, not edited here). */
  members?: string
}

/** A task: which programs run, and how often. */
export interface Task {
  name: string
  /** Period in ms; absent: continuous. */
  interval_ms?: number
  /** Logix priority (1 = most urgent). */
  priority?: number
  programs: string[]
}

/** `plcc_ladder::model::Project`, as JSON. */
export interface LadderModel {
  dialect: Dialect
  name?: string
  globals?: Variable[]
  pous: Pou[]
  declarations?: string[]
  tasks?: Task[]
}

export interface DeviceRef {
  name: string
  /** The device's manifest in the project: `devices/<id>.toml` (docs/device-manifest.md). */
  manifest: string
}

/** A studio project: the ladder model (arrays always present) and its devices. */
export interface Project {
  dialect: Dialect
  name: string
  /** Controller tags. */
  globals: Variable[]
  /** Programs. */
  pous: Pou[]
  /** TYPEs, CLASSes and the like as ST (carried along from imports). */
  declarations: string[]
  tasks: Task[]
  devices: DeviceRef[]
  /** Manifest files (TOML text) by project path, `devices/<id>.toml`. */
  deviceFiles: Record<string, string>
}

// Studio vocabulary for the same things.
export type Tag = Variable
export type Program = Pou
/** Anything with an id in a rung. */
export type Instruction = Element
export type RoutineKind = 'ladder' | 'st'
