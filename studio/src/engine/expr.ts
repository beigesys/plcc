// SPDX-License-Identifier: MPL-2.0
//
// A small Pratt-parser evaluator for CPT expressions:
//   (A + B) * 2,  Level > 2000 AND NOT Fault,  SQRT(X ** 2 + Y ** 2),  Count MOD 10
// Comparisons give 1 or 0. AND/OR/XOR/NOT are logical on 0/1 and bitwise otherwise.

import { parseLiteral, toNumber } from './values'
import type { Scalar } from './values'

export class ExprError extends Error {}

type Tok =
  | { k: 'num'; v: number }
  | { k: 'id'; v: string }
  | { k: 'op'; v: string }
  | { k: 'end' }

const OPS = ['**', '<=', '>=', '<>', '+', '-', '*', '/', '=', '<', '>', '(', ')', ',', '&']
const WORD_OPS = new Set(['AND', 'OR', 'XOR', 'NOT', 'MOD'])

function tokenize(src: string): Tok[] {
  const out: Tok[] = []
  let i = 0
  while (i < src.length) {
    const c = src[i]
    if (/\s/.test(c)) {
      i++
      continue
    }
    const lit = /^(?:(?:2|8|16)#[0-9A-Fa-f_]+|(?:T|TIME)#[0-9a-zA-Z_.]+|\d[\d_]*(?:\.\d+)?(?:[eE][+-]?\d+)?)/.exec(src.slice(i))
    if (lit && (/\d/.test(c) || /^(T|TIME)#/i.test(lit[0]))) {
      const v = parseLiteral(lit[0])
      if (v === undefined) throw new ExprError(`bad number ${lit[0]}`)
      out.push({ k: 'num', v: toNumber(v) })
      i += lit[0].length
      continue
    }
    const id = /^[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)*/.exec(src.slice(i))
    if (id) {
      const up = id[0].toUpperCase()
      if (WORD_OPS.has(up)) out.push({ k: 'op', v: up })
      else if (up === 'TRUE' || up === 'FALSE') out.push({ k: 'num', v: up === 'TRUE' ? 1 : 0 })
      else out.push({ k: 'id', v: id[0] })
      i += id[0].length
      continue
    }
    const op = OPS.find((o) => src.startsWith(o, i))
    if (!op) throw new ExprError(`unexpected '${c}'`)
    out.push({ k: 'op', v: op === '&' ? 'AND' : op })
    i += op.length
  }
  out.push({ k: 'end' })
  return out
}

const BP: Record<string, number> = {
  OR: 1, XOR: 2, AND: 3, '=': 4, '<>': 4, '<': 5, '>': 5, '<=': 5, '>=': 5,
  '+': 6, '-': 6, '*': 7, '/': 7, MOD: 7, '**': 8,
}
const PREFIX_BP = 9

const isBit = (n: number) => n === 0 || n === 1

const FUNCS: Record<string, (x: number) => number> = {
  ABS: Math.abs, SQRT: Math.sqrt, SIN: Math.sin, COS: Math.cos, TAN: Math.tan,
  LN: Math.log, LOG: Math.log10, EXP: Math.exp, TRUNC: Math.trunc,
}

function binary(op: string, a: number, b: number): number {
  switch (op) {
    case '+': return a + b
    case '-': return a - b
    case '*': return a * b
    case '/':
      if (b === 0) throw new ExprError('divide by zero')
      return a / b
    case 'MOD':
      if (b === 0) throw new ExprError('divide by zero')
      return a % b
    case '**': return a ** b
    case '=': return a === b ? 1 : 0
    case '<>': return a !== b ? 1 : 0
    case '<': return a < b ? 1 : 0
    case '>': return a > b ? 1 : 0
    case '<=': return a <= b ? 1 : 0
    case '>=': return a >= b ? 1 : 0
    case 'AND': return isBit(a) && isBit(b) ? (a && b ? 1 : 0) : a & b
    case 'OR': return isBit(a) && isBit(b) ? (a || b ? 1 : 0) : a | b
    case 'XOR': return isBit(a) && isBit(b) ? (a !== b ? 1 : 0) : a ^ b
  }
  throw new ExprError(`unknown operator ${op}`)
}

export function evaluateExpression(src: string, read: (ref: string) => Scalar | undefined): number {
  const toks = tokenize(src)
  let p = 0
  const peek = () => toks[p]
  const next = () => toks[p++]
  const expectOp = (v: string) => {
    const t = next()
    if (t.k !== 'op' || t.v !== v) throw new ExprError(`expected '${v}'`)
  }

  const prefix = (): number => {
    const t = next()
    switch (t.k) {
      case 'num':
        return t.v
      case 'id': {
        const n = peek()
        if (n.k === 'op' && n.v === '(') {
          const f = FUNCS[t.v.toUpperCase()]
          if (!f) throw new ExprError(`unknown function ${t.v}`)
          p++
          const arg = expr(0)
          expectOp(')')
          return f(arg)
        }
        const v = read(t.v)
        if (v === undefined) throw new ExprError(`unknown tag ${t.v}`)
        return toNumber(v)
      }
      case 'op':
        if (t.v === '(') {
          const v = expr(0)
          expectOp(')')
          return v
        }
        if (t.v === '-') return -expr(PREFIX_BP)
        if (t.v === '+') return expr(PREFIX_BP)
        if (t.v === 'NOT') {
          const v = expr(PREFIX_BP)
          return isBit(v) ? 1 - v : ~v
        }
        throw new ExprError(`unexpected '${t.v}'`)
      case 'end':
        throw new ExprError('unexpected end of expression')
    }
  }

  const expr = (minBp: number): number => {
    let lhs = prefix()
    for (;;) {
      const t = peek()
      if (t.k !== 'op') break
      const bp = BP[t.v]
      if (bp === undefined || bp <= minBp) break
      p++
      // `**` is right-associative; the rest are left-associative.
      const rhs = expr(t.v === '**' ? bp - 1 : bp)
      lhs = binary(t.v, lhs, rhs)
    }
    return lhs
  }

  const v = expr(0)
  if (peek().k !== 'end') throw new ExprError('unexpected text after expression')
  return v
}
