// SPDX-License-Identifier: MPL-2.0
//
// The Logix instruction catalog: plcc's (logix-catalog.json, generated from
// plcc_ladder::catalog by scripts/gen-catalog.mjs) with what the editor adds
// for the instructions it offers: titles, defaults, the type a new tag gets,
// and the status bits drawn on a box.

import catalogJson from './logix-catalog.json'
import type { Block, Element, PinDir } from './types'

export type Role = 'input' | 'output'

/** One instruction of plcc's catalog (`plcc_ladder::catalog::Spec`). */
export interface CatalogSpec {
  name: string
  category: string
  role: 'input' | 'output' | 'box'
  pins: { name: string; dir: PinDir }[]
  variadic: boolean
  instance: boolean
}

export const LOGIX_CATALOG = catalogJson as CatalogSpec[]
const BY_NAME = new Map(LOGIX_CATALOG.map((s) => [s.name, s]))

export function catalogSpec(name: string): CatalogSpec | undefined {
  return BY_NAME.get(name.toUpperCase())
}

/** Name of operand `k` of `mnemonic`, as plcc names it (`catalog::logix_operand_name`). */
export function logixOperandName(mnemonic: string, k: number): string {
  const s = catalogSpec(mnemonic)
  if (s && s.variadic && s.pins.length > 0 && k + 1 >= s.pins.length) {
    return `${s.pins[s.pins.length - 1].name} ${k + 2 - s.pins.length}`
  }
  if (s && k < s.pins.length) return s.pins[k].name
  return `Operand ${k + 1}`
}

/** Direction of operand `k` of `mnemonic` (`catalog::logix_operand_dir`). */
export function logixOperandDir(mnemonic: string, k: number): PinDir {
  const s = catalogSpec(mnemonic)
  if (s && k < s.pins.length) return s.pins[k].dir
  if (s && s.variadic && s.pins.length > 0) return s.pins[s.pins.length - 1].dir
  return 'input'
}

export interface PinSpec {
  /** The pin's name in the model (plcc's operand name). */
  name: string
  /** Shorter label for the box. */
  label: string
  /** Type a new tag gets when this operand names an unknown tag; undefined = never create. */
  newTagType?: string
  /** Text when the instruction is inserted. */
  default: string
}

export interface BoxSpec {
  instr: string
  title: string
  role: Role
  pins: PinSpec[]
  /** Status bits drawn on the right edge (members of the first operand's tag). */
  outputs?: string[]
  group: 'Timers' | 'Counters' | 'Bit' | 'Compare' | 'Math' | 'Move' | 'Program' | 'Other'
}

const tag = (name: string, label: string, newTagType?: string): PinSpec => ({ name, label, newTagType, default: '?' })
const lit = (name: string, label: string, def: string): PinSpec => ({ name, label, default: def })

const timer = (instr: string, title: string): BoxSpec => ({
  instr, title, role: 'output', group: 'Timers', outputs: ['EN', 'DN'],
  pins: [tag('Timer', 'Timer', 'TIMER'), lit('Preset', 'Preset', '1000'), lit('Accum', 'Accum', '0')],
})
const counter = (instr: string, title: string): BoxSpec => ({
  instr, title, role: 'output', group: 'Counters', outputs: [instr === 'CTU' ? 'CU' : 'CD', 'DN'],
  pins: [tag('Counter', 'Counter', 'COUNTER'), lit('Preset', 'Preset', '10'), lit('Accum', 'Accum', '0')],
})
const compare = (instr: string, title: string): BoxSpec => ({
  instr, title, role: 'input', group: 'Compare',
  pins: [tag('Source A', 'Source A', 'DINT'), lit('Source B', 'Source B', '0')],
})
const math = (instr: string, title: string): BoxSpec => ({
  instr, title, role: 'output', group: 'Math',
  pins: [tag('Source A', 'Source A', 'DINT'), lit('Source B', 'Source B', '0'), tag('Dest', 'Dest', 'DINT')],
})

/** The box instructions the editor offers, with its extras. */
export const BOX_SPECS: Record<string, BoxSpec> = {
  TON: timer('TON', 'Timer On Delay'),
  TOF: timer('TOF', 'Timer Off Delay'),
  RTO: timer('RTO', 'Retentive Timer On'),
  CTU: counter('CTU', 'Count Up'),
  CTD: counter('CTD', 'Count Down'),
  RES: { instr: 'RES', title: 'Reset', role: 'output', group: 'Timers', pins: [tag('Structure', 'Timer/Counter')] },
  ONS: { instr: 'ONS', title: 'One Shot', role: 'input', group: 'Bit', pins: [tag('Storage Bit', 'Storage Bit', 'BOOL')] },
  OSR: {
    instr: 'OSR', title: 'One Shot Rising', role: 'output', group: 'Bit',
    pins: [tag('Storage Bit', 'Storage Bit', 'BOOL'), tag('Output Bit', 'Output Bit', 'BOOL')],
  },
  OSF: {
    instr: 'OSF', title: 'One Shot Falling', role: 'output', group: 'Bit',
    pins: [tag('Storage Bit', 'Storage Bit', 'BOOL'), tag('Output Bit', 'Output Bit', 'BOOL')],
  },
  EQU: compare('EQU', 'Equal'),
  NEQ: compare('NEQ', 'Not Equal'),
  GRT: compare('GRT', 'Greater Than'),
  GEQ: compare('GEQ', 'Greater or Equal'),
  LES: compare('LES', 'Less Than'),
  LEQ: compare('LEQ', 'Less or Equal'),
  LIM: {
    instr: 'LIM', title: 'Limit', role: 'input', group: 'Compare',
    pins: [lit('Low Limit', 'Low', '0'), tag('Test', 'Test', 'DINT'), lit('High Limit', 'High', '100')],
  },
  ADD: math('ADD', 'Add'),
  SUB: math('SUB', 'Subtract'),
  MUL: math('MUL', 'Multiply'),
  DIV: math('DIV', 'Divide'),
  MOV: {
    instr: 'MOV', title: 'Move', role: 'output', group: 'Move',
    pins: [lit('Source', 'Source', '0'), tag('Dest', 'Dest', 'DINT')],
  },
  CPT: {
    instr: 'CPT', title: 'Compute', role: 'output', group: 'Math',
    pins: [tag('Dest', 'Dest', 'DINT'), lit('Expression', 'Expression', '0')],
  },
  JSR: {
    instr: 'JSR', title: 'Jump to Subroutine', role: 'output', group: 'Program',
    pins: [lit('Routine Name', 'Routine', '?'), lit('Input Count', 'Inputs', '0')],
  },
}

/** The editor's spec of a block: its own, else one from plcc's catalog, else from its pins. */
export function boxSpec(b: Pick<Block, 'name' | 'pins'>): BoxSpec {
  const own = BOX_SPECS[b.name.toUpperCase()]
  if (own) return own
  const c = catalogSpec(b.name)
  const pins = b.pins.map((p) => ({ name: p.name, label: p.name, default: '?' }))
  return {
    instr: b.name,
    title: c ? c.category.replace('_', ' ') : 'Instruction',
    role: c?.role === 'input' ? 'input' : 'output',
    group: 'Other',
    pins,
  }
}

/** Any instruction the editor can insert by mnemonic (contacts, coils, boxes, ST). */
export function specOf(mnemonic: string): { pins: PinSpec[] } | undefined {
  const m = mnemonic.toUpperCase()
  if (m === 'XIC' || m === 'XIO' || m === 'OTE' || m === 'OTL' || m === 'OTU') return { pins: [tag('Data Bit', 'Tag', 'BOOL')] }
  if (m === 'JMP') return { pins: [lit('Label Name', 'Label', '?')] }
  if (m === 'RET') return { pins: [] }
  const own = BOX_SPECS[m]
  if (own) return own
  const c = catalogSpec(m)
  return c ? { pins: c.pins.map((p) => ({ name: p.name, label: p.name, default: '?' })) } : undefined
}

export function isBoxInstr(m: string): boolean {
  return Object.prototype.hasOwnProperty.call(BOX_SPECS, m.toUpperCase())
}

/** Whether an element acts on the rung (coils, output instructions) rather than conditioning it. */
export function isOutput(e: Element): boolean {
  if (e.type === 'coil' || e.type === 'jump' || e.type === 'return') return true
  if (e.type === 'block') return boxSpec(e).role === 'output'
  return false
}

/** Contact and coil mnemonics (Logix). */
export const CONTACT_MNEMONIC = { no: 'XIC', nc: 'XIO' } as const
export const COIL_MNEMONIC = { normal: 'OTE', set: 'OTL', reset: 'OTU' } as const

export interface PaletteItem {
  mnemonic: string
  label: string
  group: string
}

export const PALETTE: PaletteItem[] = [
  { mnemonic: 'XIC', label: 'Examine On (NO contact)', group: 'Bit' },
  { mnemonic: 'XIO', label: 'Examine Off (NC contact)', group: 'Bit' },
  { mnemonic: 'OTE', label: 'Output Energize (coil)', group: 'Bit' },
  { mnemonic: 'OTL', label: 'Output Latch (set)', group: 'Bit' },
  { mnemonic: 'OTU', label: 'Output Unlatch (reset)', group: 'Bit' },
  ...Object.values(BOX_SPECS).map((s) => ({ mnemonic: s.instr, label: s.title, group: s.group })),
  { mnemonic: 'JMP', label: 'Jump to label', group: 'Program' },
  { mnemonic: 'RET', label: 'Return', group: 'Program' },
  { mnemonic: 'ST', label: 'Structured Text box', group: 'Program' },
]
