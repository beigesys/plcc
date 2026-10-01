// SPDX-License-Identifier: MPL-2.0
//
// Rockwell neutral rung text, both directions, as plcc reads and writes it
// (crates/plcc-ladder/src/rll.rs):
//
//   [XIC(StartPB) ,XIC(Motor) ]XIO(StopPB)OTE(Motor);
//   XIC(Motor)TON(RunTimer,5000,0);
//
// Reading follows plcc: XIC/XIO are contacts, OTE/OTL/OTU coils, JMP a
// jump, RET() a return, a leading LBL the rung's label, anything else a block
// whose pins are named from plcc's Logix catalog. Writing is canonical (what
// Studio 5000 exports and plcc writes into L5X), so the text the studio shows
// is the text plcc compiles and its diagnostic columns line up.
//
// One studio extension: `ST("...")` holds an ST box (plcc writes an ST box
// to L5X as a JSR to an ST routine). Quick entry (`XIC Start XIO Stop OTE
// Motor`, with `BST`/`NXB`/`BND` or `[ , ]` for branches) is also here.

import { newId } from './ids'
import { logixOperandName, logixOperandDir, specOf } from './instructions'
import type { Block, Element, Rung } from './types'

export class RungTextError extends Error {
  readonly offset: number
  constructor(message: string, offset: number) {
    super(`${message} (at column ${offset + 1})`)
    this.offset = offset
  }
}

// ---------------------------------------------------------------- writing

const CONTACT_TEXT = { no: 'XIC', nc: 'XIO', rising: 'XICR', falling: 'XICF' } as const
const COIL_TEXT = { normal: 'OTE', negated: 'OTEN', set: 'OTL', reset: 'OTU', rising: 'OTER', falling: 'OTEF' } as const

function operand(v: string | undefined): string {
  return v === undefined || v.trim() === '' ? '?' : v
}

export function printElement(e: Element): string {
  switch (e.type) {
    case 'contact':
      return `${CONTACT_TEXT[e.kind]}(${operand(e.operand)})`
    case 'coil':
      return `${COIL_TEXT[e.kind]}(${operand(e.operand)})`
    case 'branch':
      return `[${e.legs.map((l) => `${printSeries(l)} `).join(',')}]`
    case 'block':
      return `${e.name}(${e.pins.map((p) => operand(p.value)).join(',')})`
    case 'jump':
      return `JMP(${e.label})`
    case 'return':
      return 'RET()'
    case 'st':
      return `ST(${JSON.stringify(e.code)})`
  }
}

export function printSeries(items: Element[]): string {
  return items.map(printElement).join('')
}

/** A rung's text, `;` included, with its label as a leading `LBL`. */
export function printRung(rung: Pick<Rung, 'elements' | 'label'>): string {
  return `${rung.label ? `LBL(${rung.label})` : ''}${printSeries(rung.elements)};`
}

// ---------------------------------------------------------------- reading

interface RawInstr {
  kind: 'instr'
  name: string
  operands: string[]
  at: number
}
interface RawBranch {
  kind: 'branch'
  legs: Raw[][]
}
type Raw = RawInstr | RawBranch

class Parser {
  pos = 0
  readonly src: string
  constructor(src: string) {
    this.src = src
  }

  ws() {
    while (this.pos < this.src.length && /\s/.test(this.src[this.pos])) this.pos++
  }

  seq(depth: number): Raw[] {
    const out: Raw[] = []
    for (;;) {
      this.ws()
      if (this.pos >= this.src.length) {
        if (depth > 0) throw new RungTextError('unterminated branch `[`', this.pos)
        return out
      }
      const c = this.src[this.pos]
      if (c === ';' || c === ',' || c === ']') return out
      if (c === '[') {
        this.pos++
        const legs: Raw[][] = []
        for (;;) {
          legs.push(this.seq(depth + 1))
          this.ws()
          const d = this.src[this.pos]
          if (d === ',') this.pos++
          else if (d === ']') {
            this.pos++
            break
          } else throw new RungTextError('expected `,` or `]` in branch', this.pos)
        }
        out.push({ kind: 'branch', legs })
      } else if (/[A-Za-z_]/.test(c)) out.push(this.instr())
      else throw new RungTextError(`unexpected \`${c}\` in rung`, this.pos)
    }
  }

  instr(): RawInstr {
    const start = this.pos
    while (this.pos < this.src.length && /[A-Za-z0-9_]/.test(this.src[this.pos])) this.pos++
    const name = this.src.slice(start, this.pos)
    this.ws()
    if (this.src[this.pos] !== '(') throw new RungTextError(`expected \`(\` after instruction \`${name}\``, start)
    this.pos++
    // The studio's ST box: a JSON string, kept whole.
    if (name.toUpperCase() === 'ST') {
      this.ws()
      if (this.src[this.pos] === '"') {
        const s = this.pos
        this.pos++
        while (this.pos < this.src.length && this.src[this.pos] !== '"') this.pos += this.src[this.pos] === '\\' ? 2 : 1
        if (this.pos >= this.src.length) throw new RungTextError('unterminated string', s)
        this.pos++
        const lit = this.src.slice(s, this.pos)
        this.ws()
        if (this.src[this.pos] !== ')') throw new RungTextError('expected `)` after the ST text', this.pos)
        this.pos++
        return { kind: 'instr', name: 'ST', operands: [lit], at: start }
      }
    }
    const operands: string[] = []
    let depth = 0
    let opStart = this.pos
    for (;;) {
      if (this.pos >= this.src.length) throw new RungTextError(`unterminated operand list of \`${name}\``, start)
      const c = this.src[this.pos]
      if (c === "'" || c === '"') {
        this.pos++
        while (this.pos < this.src.length && this.src[this.pos] !== c) {
          if (this.src[this.pos] === '$') this.pos++
          this.pos++
        }
      } else if (c === '(' || c === '[') depth++
      else if ((c === ')' || c === ']') && depth > 0) depth--
      else if (c === ')') {
        operands.push(this.src.slice(opStart, this.pos).trim())
        this.pos++
        break
      } else if (c === ',' && depth === 0) {
        operands.push(this.src.slice(opStart, this.pos).trim())
        opStart = this.pos + 1
      }
      this.pos++
    }
    if (operands.length === 1 && operands[0] === '') operands.length = 0
    return { kind: 'instr', name, operands, at: start }
  }
}

function element(r: Raw): Element {
  if (r.kind === 'branch') return { type: 'branch', id: newId(), legs: r.legs.map((l) => l.map(element)) }
  const up = r.name.toUpperCase()
  const one = r.operands.length === 1
  const exact = r.name === up
  if (one && exact && (up === 'XIC' || up === 'XIO')) {
    return { type: 'contact', id: newId(), operand: r.operands[0], kind: up === 'XIC' ? 'no' : 'nc' }
  }
  if (one && exact && (up === 'OTE' || up === 'OTL' || up === 'OTU')) {
    return { type: 'coil', id: newId(), operand: r.operands[0], kind: up === 'OTE' ? 'normal' : up === 'OTL' ? 'set' : 'reset' }
  }
  if (one && exact && up === 'JMP') return { type: 'jump', id: newId(), label: r.operands[0] }
  if (exact && up === 'RET' && r.operands.length === 0) return { type: 'return', id: newId() }
  if (one && up === 'ST') {
    let code = r.operands[0]
    try {
      if (code.startsWith('"')) code = JSON.parse(code) as string
    } catch {
      throw new RungTextError('bad ST string', r.at)
    }
    return { type: 'st', id: newId(), code }
  }
  const block: Block = {
    type: 'block',
    id: newId(),
    name: r.name,
    pins: r.operands.map((v, k) => ({ name: logixOperandName(r.name, k), dir: logixOperandDir(r.name, k), value: v })),
  }
  return block
}

function toRung(seq: Raw[]): Pick<Rung, 'elements' | 'label'> {
  const first = seq[0]
  if (first?.kind === 'instr' && first.name === 'LBL' && first.operands.length === 1) {
    return { label: first.operands[0], elements: seq.slice(1).map(element) }
  }
  return { elements: seq.map(element) }
}

function parseAll(text: string): Pick<Rung, 'elements' | 'label'>[] {
  const p = new Parser(text)
  const out: Pick<Rung, 'elements' | 'label'>[] = []
  for (;;) {
    p.ws()
    if (p.pos >= text.length) break
    const seq = p.seq(0)
    p.ws()
    if (p.pos < text.length && text[p.pos] === ';') p.pos++
    else if (p.pos < text.length) throw new RungTextError(`unexpected \`${text[p.pos]}\` in rung`, p.pos)
    out.push(toRung(seq))
  }
  return out
}

/** Parses one rung (a trailing `;` is optional). */
export function parseRung(text: string): Pick<Rung, 'elements' | 'label'> {
  const all = parseAll(text)
  if (all.length > 1) throw new RungTextError('text after end of rung', text.indexOf(';') + 1)
  return all[0] ?? { elements: [] }
}

/** Parses several rungs separated by `;`. */
export function parseRungs(text: string): Pick<Rung, 'elements' | 'label'>[] {
  return parseAll(text)
}

// ---------------------------------------------------------------- quick entry

/** Operand count of a mnemonic for quick entry, or undefined if unknown. */
function operandCount(m: string): number | undefined {
  if (m === 'ST') return 1
  const s = specOf(m)
  return s ? s.pins.length : undefined
}

/**
 * `XIC Start XIO Stop OTE Motor`, `XIC Motor TON RunTimer 5000 0`,
 * `BST XIC Start NXB XIC Motor BND XIO Stop OTE Motor`. Text containing a
 * `(` is read as ordinary rung text instead.
 */
export function parseQuickEntry(text: string): Pick<Rung, 'elements' | 'label'> {
  if (text.includes('(')) return parseRung(text)
  const tokens = text.trim().split(/\s+/).filter(Boolean)
  const out: string[] = []
  let i = 0
  while (i < tokens.length) {
    const t = tokens[i].toUpperCase()
    if (t === 'BST' || t === '[') {
      out.push('[')
      i++
      continue
    }
    if (t === 'NXB' || t === ',') {
      out.push(' ,')
      i++
      continue
    }
    if (t === 'BND' || t === ']') {
      out.push(' ]')
      i++
      continue
    }
    const n = operandCount(t)
    if (n === undefined) throw new RungTextError(`unknown instruction ${tokens[i]}`, 0)
    const ops = tokens.slice(i + 1, i + 1 + n)
    while (ops.length < n) ops.push('?')
    out.push(`${t}(${ops.join(',')})`)
    i += 1 + n
  }
  return parseRung(out.join(''))
}
