// SPDX-License-Identifier: MPL-2.0
//
// Instruction catalog: mnemonics, operand lists, and how each one is drawn and
// executed. Rung text, the palette, quick entry and the simulator all read it.

import type { BoxInstr, CoilKind, ContactKind } from './types'

export type Role = 'input' | 'output'

export interface OperandSpec {
  /** Key in `Box.operands`. */
  key: string
  label: string
  /** Type a new tag gets when this operand names an unknown tag; undefined = never create. */
  newTagType?: string
  /** Default text when the instruction is inserted. */
  default: string
}

export interface BoxSpec {
  instr: BoxInstr
  title: string
  role: Role
  operands: OperandSpec[]
  /** Status bits drawn on the right edge (read from the first operand's tag). */
  outputs?: string[]
  group: 'Timers' | 'Counters' | 'Bit' | 'Compare' | 'Math' | 'Move' | 'Program'
}

const tag = (key: string, label: string, newTagType?: string): OperandSpec => ({
  key, label, newTagType, default: '?',
})
const lit = (key: string, label: string, def: string): OperandSpec => ({ key, label, default: def })

const timer = (instr: BoxInstr, title: string): BoxSpec => ({
  instr, title, role: 'output', group: 'Timers', outputs: ['EN', 'DN'],
  operands: [tag('timer', 'Timer', 'TIMER'), lit('preset', 'Preset', '1000'), lit('accum', 'Accum', '0')],
})
const counter = (instr: BoxInstr, title: string): BoxSpec => ({
  instr, title, role: 'output', group: 'Counters', outputs: [instr === 'CTU' ? 'CU' : 'CD', 'DN'],
  operands: [tag('counter', 'Counter', 'COUNTER'), lit('preset', 'Preset', '10'), lit('accum', 'Accum', '0')],
})
const compare = (instr: BoxInstr, title: string): BoxSpec => ({
  instr, title, role: 'input', group: 'Compare',
  operands: [tag('a', 'Source A', 'DINT'), lit('b', 'Source B', '0')],
})
const math = (instr: BoxInstr, title: string): BoxSpec => ({
  instr, title, role: 'output', group: 'Math',
  operands: [tag('a', 'Source A', 'DINT'), lit('b', 'Source B', '0'), tag('dest', 'Dest', 'DINT')],
})

export const BOX_SPECS: Record<BoxInstr, BoxSpec> = {
  TON: timer('TON', 'Timer On Delay'),
  TOF: timer('TOF', 'Timer Off Delay'),
  RTO: timer('RTO', 'Retentive Timer On'),
  CTU: counter('CTU', 'Count Up'),
  CTD: counter('CTD', 'Count Down'),
  RES: { instr: 'RES', title: 'Reset', role: 'output', group: 'Timers', operands: [tag('target', 'Timer/Counter')] },
  ONS: { instr: 'ONS', title: 'One Shot', role: 'input', group: 'Bit', operands: [tag('storage', 'Storage Bit', 'BOOL')] },
  OSR: {
    instr: 'OSR', title: 'One Shot Rising', role: 'output', group: 'Bit',
    operands: [tag('storage', 'Storage Bit', 'BOOL'), tag('output', 'Output Bit', 'BOOL')],
  },
  EQU: compare('EQU', 'Equal'),
  NEQ: compare('NEQ', 'Not Equal'),
  GRT: compare('GRT', 'Greater Than'),
  GEQ: compare('GEQ', 'Greater or Equal'),
  LES: compare('LES', 'Less Than'),
  LEQ: compare('LEQ', 'Less or Equal'),
  ADD: math('ADD', 'Add'),
  SUB: math('SUB', 'Subtract'),
  MUL: math('MUL', 'Multiply'),
  DIV: math('DIV', 'Divide'),
  MOV: {
    instr: 'MOV', title: 'Move', role: 'output', group: 'Move',
    operands: [lit('source', 'Source', '0'), tag('dest', 'Dest', 'DINT')],
  },
  CPT: {
    instr: 'CPT', title: 'Compute', role: 'output', group: 'Math',
    operands: [tag('dest', 'Dest', 'DINT'), lit('expr', 'Expression', '0')],
  },
  JSR: { instr: 'JSR', title: 'Jump to Subroutine', role: 'output', group: 'Program', operands: [lit('routine', 'Routine', '?')] },
}

/** Rung-text mnemonics for contacts and coils. XIC/XIO/OTE/OTL/OTU are Logix; the rest are plcc extensions. */
export const CONTACT_MNEMONIC: Record<ContactKind, string> = { no: 'XIC', nc: 'XIO', rise: 'XICR', fall: 'XICF' }
export const COIL_MNEMONIC: Record<CoilKind, string> = {
  normal: 'OTE', negated: 'OTEN', set: 'OTL', reset: 'OTU', rise: 'OTER', fall: 'OTEF',
}

export const CONTACT_BY_MNEMONIC: Record<string, ContactKind> = Object.fromEntries(
  Object.entries(CONTACT_MNEMONIC).map(([k, m]) => [m, k as ContactKind]),
)
export const COIL_BY_MNEMONIC: Record<string, CoilKind> = Object.fromEntries(
  Object.entries(COIL_MNEMONIC).map(([k, m]) => [m, k as CoilKind]),
)

export function isBoxInstr(m: string): m is BoxInstr {
  return Object.prototype.hasOwnProperty.call(BOX_SPECS, m)
}

/** Number of operands a mnemonic takes (for quick entry), or undefined if unknown. */
export function operandCount(mnemonic: string): number | undefined {
  if (mnemonic in CONTACT_BY_MNEMONIC || mnemonic in COIL_BY_MNEMONIC) return 1
  if (mnemonic === 'ST') return 1
  if (isBoxInstr(mnemonic)) return BOX_SPECS[mnemonic].operands.length
  return undefined
}

export interface PaletteItem {
  mnemonic: string
  label: string
  group: string
}

export const PALETTE: PaletteItem[] = [
  { mnemonic: 'XIC', label: 'Examine On (NO contact)', group: 'Bit' },
  { mnemonic: 'XIO', label: 'Examine Off (NC contact)', group: 'Bit' },
  { mnemonic: 'XICR', label: 'Rising-edge contact', group: 'Bit' },
  { mnemonic: 'XICF', label: 'Falling-edge contact', group: 'Bit' },
  { mnemonic: 'OTE', label: 'Output Energize (coil)', group: 'Bit' },
  { mnemonic: 'OTEN', label: 'Negated coil', group: 'Bit' },
  { mnemonic: 'OTL', label: 'Output Latch (set)', group: 'Bit' },
  { mnemonic: 'OTU', label: 'Output Unlatch (reset)', group: 'Bit' },
  { mnemonic: 'OTER', label: 'Rising-edge coil', group: 'Bit' },
  { mnemonic: 'OTEF', label: 'Falling-edge coil', group: 'Bit' },
  ...Object.values(BOX_SPECS).map((s) => ({ mnemonic: s.instr, label: s.title, group: s.group })),
]
