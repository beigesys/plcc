// SPDX-License-Identifier: MPL-2.0
//
// Rockwell-style neutral rung text, both directions:
//
//   [XIC(StartPB) ,XIC(Motor) ]XIO(StopPB)OTE(Motor);
//   XIC(Motor)TON(RunTimer,5000,0);
//
// Branches are `[a ,b ]`. Operands are separated by commas at paren depth 0,
// so CPT expressions may contain parentheses. `ST("...")` holds inline ST.
// Also exported: quick entry (`XIC Start XIO Stop OTE Motor`, with optional
// `BST`/`NXB`/`BND` or `[ , ]` for branches).

import { newId } from './ids'
import {
  BOX_SPECS, COIL_BY_MNEMONIC, COIL_MNEMONIC, CONTACT_BY_MNEMONIC, CONTACT_MNEMONIC, isBoxInstr, operandCount,
} from './instructions'
import type { Element, Instruction, Parallel, Series } from './types'

export class RungTextError extends Error {
  readonly offset: number
  constructor(message: string, offset: number) {
    super(`${message} (at column ${offset + 1})`)
    this.offset = offset
  }
}

// ---------------------------------------------------------------- printing

export function printElement(e: Element): string {
  switch (e.type) {
    case 'series':
      return e.items.map(printElement).join('')
    case 'parallel':
      return `[${e.branches.map(printElement).join(' ,')} ]`
    case 'contact':
      return `${CONTACT_MNEMONIC[e.kind]}(${e.tag || '?'})`
    case 'coil':
      return `${COIL_MNEMONIC[e.kind]}(${e.tag || '?'})`
    case 'box': {
      const spec = BOX_SPECS[e.instr]
      const ops = spec.operands.map((o) => (e.operands[o.key] ?? '').trim() || '?')
      return `${e.instr}(${ops.join(',')})`
    }
    case 'st':
      return `ST(${JSON.stringify(e.code)})`
  }
}

export function printRung(body: Series): string {
  return `${printElement(body)};`
}

// ---------------------------------------------------------------- parsing

class Parser {
  pos = 0
  readonly src: string
  constructor(src: string) {
    this.src = src
  }

  ws() {
    while (this.pos < this.src.length && /\s/.test(this.src[this.pos])) this.pos++
  }
  peek(): string {
    this.ws()
    return this.src[this.pos] ?? ''
  }
  expect(ch: string) {
    if (this.peek() !== ch) throw new RungTextError(`expected '${ch}'`, this.pos)
    this.pos++
  }

  series(stop: string[]): Series {
    const items: Element[] = []
    for (;;) {
      const c = this.peek()
      if (c === '' || stop.includes(c)) return { type: 'series', items }
      if (c === '[') items.push(this.parallel())
      else items.push(this.instruction())
    }
  }

  parallel(): Parallel {
    this.expect('[')
    const branches: Series[] = [this.series([',', ']'])]
    while (this.peek() === ',') {
      this.pos++
      branches.push(this.series([',', ']']))
    }
    this.expect(']')
    return { type: 'parallel', id: newId('p'), branches }
  }

  instruction(): Instruction {
    this.ws()
    const start = this.pos
    const m = /^[A-Za-z_][A-Za-z0-9_]*/.exec(this.src.slice(this.pos))
    if (!m) throw new RungTextError(`unexpected '${this.src[this.pos]}'`, this.pos)
    const mnemonic = m[0].toUpperCase()
    this.pos += m[0].length
    this.expect('(')
    const ops = this.operands()
    return buildInstruction(mnemonic, ops, start)
  }

  /** Reads operands up to the matching ')', splitting on depth-0 commas. */
  operands(): string[] {
    const out: string[] = []
    let depth = 0
    let cur = ''
    while (this.pos < this.src.length) {
      const c = this.src[this.pos]
      if (c === '"') {
        const end = scanString(this.src, this.pos)
        cur += this.src.slice(this.pos, end)
        this.pos = end
        continue
      }
      this.pos++
      if (c === '(') depth++
      else if (c === ')') {
        if (depth === 0) {
          out.push(cur.trim())
          return out
        }
        depth--
      } else if (c === ',' && depth === 0) {
        out.push(cur.trim())
        cur = ''
        continue
      }
      cur += c
    }
    throw new RungTextError("missing ')'", this.pos)
  }
}

function scanString(src: string, start: number): number {
  let i = start + 1
  while (i < src.length) {
    if (src[i] === '\\') i += 2
    else if (src[i] === '"') return i + 1
    else i++
  }
  throw new RungTextError('unterminated string', start)
}

function buildInstruction(mnemonic: string, ops: string[], at: number): Instruction {
  const clean = (s: string | undefined) => (s === undefined || s === '?' ? '' : s)
  const want = (n: number) => {
    const got = ops.length === 1 && ops[0] === '' ? 0 : ops.length
    if (got !== n) throw new RungTextError(`${mnemonic} takes ${n} operand${n === 1 ? '' : 's'}, got ${got}`, at)
  }
  if (mnemonic in CONTACT_BY_MNEMONIC) {
    want(1)
    return { type: 'contact', id: newId('c'), kind: CONTACT_BY_MNEMONIC[mnemonic], tag: clean(ops[0]) }
  }
  if (mnemonic in COIL_BY_MNEMONIC) {
    want(1)
    return { type: 'coil', id: newId('o'), kind: COIL_BY_MNEMONIC[mnemonic], tag: clean(ops[0]) }
  }
  if (mnemonic === 'ST') {
    want(1)
    let code = ops[0]
    try {
      if (code.startsWith('"')) code = JSON.parse(code) as string
    } catch {
      throw new RungTextError('bad ST string', at)
    }
    return { type: 'st', id: newId('s'), code }
  }
  if (isBoxInstr(mnemonic)) {
    const spec = BOX_SPECS[mnemonic]
    want(spec.operands.length)
    const operands: Record<string, string> = {}
    spec.operands.forEach((o, i) => (operands[o.key] = clean(ops[i])))
    return { type: 'box', id: newId('b'), instr: mnemonic, operands }
  }
  throw new RungTextError(`unknown instruction ${mnemonic}`, at)
}

/** Parses one rung. A trailing `;` is optional. */
export function parseRung(text: string): Series {
  const p = new Parser(text)
  const body = p.series([';', ']', ','])
  const c = p.peek()
  if (c === ']' || c === ',') throw new RungTextError(`unexpected '${c}'`, p.pos)
  if (c === ';') p.pos++
  if (p.peek() !== '') throw new RungTextError('text after end of rung', p.pos)
  return body
}

/** Parses several rungs separated by `;`. */
export function parseRungs(text: string): Series[] {
  const out: Series[] = []
  const p = new Parser(text)
  while (p.peek() !== '') {
    const body = p.series([';', ']', ','])
    const c = p.peek()
    if (c === ']' || c === ',') throw new RungTextError(`unexpected '${c}'`, p.pos)
    if (c === ';') p.pos++
    out.push(body)
  }
  return out
}

// ---------------------------------------------------------------- quick entry

/**
 * `XIC Start XIO Stop OTE Motor`, `XIC Motor TON RunTimer 5000 0`,
 * `BST XIC Start NXB XIC Motor BND XIO Stop OTE Motor`. If the text contains a
 * `(` it is read as ordinary rung text instead.
 */
export function parseQuickEntry(text: string): Series {
  if (text.includes('(')) return parseRung(text)
  const tokens = text.trim().split(/\s+/).filter(Boolean)
  const out: string[] = []
  let i = 0
  while (i < tokens.length) {
    const t = tokens[i].toUpperCase()
    if (t === 'BST' || t === '[') { out.push('['); i++; continue }
    if (t === 'NXB' || t === ',') { out.push(' ,'); i++; continue }
    if (t === 'BND' || t === ']') { out.push(' ]'); i++; continue }
    const n = operandCount(t)
    if (n === undefined) throw new RungTextError(`unknown instruction ${tokens[i]}`, 0)
    const ops = tokens.slice(i + 1, i + 1 + n)
    while (ops.length < n) ops.push('?')
    out.push(`${t}(${ops.join(',')})`)
    i += 1 + n
  }
  return parseRung(out.join(''))
}
