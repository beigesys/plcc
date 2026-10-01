// SPDX-License-Identifier: MPL-2.0
//
// The ladder model as JSON, the way plcc writes it (serde: fields in struct
// order, empty lists, absent options and false flags left out), and read
// back with a check of every field, so a hand-edited or foreign file fails
// with a path instead of breaking the editor.

import type {
  Element, LadderModel, Pin, Pou, Project, Routine, Rung, Task, Variable,
} from './types'

export class ModelFormatError extends Error {
  readonly path: string
  constructor(path: string, message: string) {
    super(`${path}: ${message}`)
    this.name = 'ModelFormatError'
    this.path = path
  }
}

type Obj = Record<string, unknown>

/** An object with keys in this order, leaving out undefined, empty lists, empty strings (where said) and false flags. */
function ordered(entries: [string, unknown, ('list' | 'flag' | 'string')?][]): Obj {
  const o: Obj = {}
  for (const [k, v, skip] of entries) {
    if (v === undefined) continue
    if (skip === 'list' && Array.isArray(v) && v.length === 0) continue
    if (skip === 'flag' && v === false) continue
    if (skip === 'string' && v === '') continue
    o[k] = v
  }
  return o
}

function variableJson(v: Variable): Obj {
  return ordered([
    ['name', v.name],
    ['data_type', v.data_type],
    ['section', v.section],
    ['initial', v.initial],
    ['address', v.address],
    ['comment', v.comment],
    ['constant', v.constant, 'flag'],
    ['retain', v.retain, 'flag'],
  ])
}

function pinJson(p: Pin): Obj {
  return ordered([
    ['name', p.name],
    ['dir', p.dir ?? 'input'],
    ['value', p.value],
    ['negated', p.negated, 'flag'],
    ['rung', p.rung?.map(elementJson)],
  ])
}

function elementJson(e: Element): Obj {
  switch (e.type) {
    case 'contact':
    case 'coil':
      return ordered([['type', e.type], ['id', e.id], ['operand', e.operand], ['kind', e.kind], ['notes', e.notes, 'list']])
    case 'branch':
      return ordered([['type', e.type], ['id', e.id], ['legs', e.legs.map((l) => l.map(elementJson))]])
    case 'block':
      return ordered([
        ['type', e.type],
        ['id', e.id],
        ['name', e.name],
        ['instance', e.instance],
        ['pins', e.pins.map(pinJson)],
        ['power_in', e.power_in],
        ['power_out', e.power_out],
        ['notes', e.notes, 'list'],
      ])
    case 'jump':
      return ordered([['type', e.type], ['id', e.id], ['label', e.label]])
    case 'return':
      return ordered([['type', e.type], ['id', e.id]])
    case 'st':
      return ordered([['type', e.type], ['id', e.id], ['code', e.code], ['notes', e.notes, 'list']])
  }
}

function rungJson(r: Rung): Obj {
  return ordered([['id', r.id], ['comment', r.comment], ['label', r.label], ['elements', r.elements.map(elementJson)]])
}

function routineJson(r: Routine): Obj {
  return ordered([['id', r.id], ['name', r.name], ['rungs', r.rungs.map(rungJson)]])
}

function pouJson(p: Pou): Obj {
  return ordered([
    ['id', p.id],
    ['name', p.name],
    ['kind', p.kind],
    ['return_type', p.return_type],
    ['variables', p.variables.map(variableJson), 'list'],
    ['routines', p.routines.map(routineJson)],
    ['members', p.members, 'string'],
  ])
}

function taskJson(t: Task): Obj {
  return ordered([['name', t.name], ['interval_ms', t.interval_ms], ['priority', t.priority], ['programs', t.programs]])
}

/** The model part of a project as plcc-ladder JSON (an object). */
export function toLadderModel(p: Pick<Project, 'dialect' | 'name' | 'globals' | 'pous' | 'declarations' | 'tasks'>): Obj {
  return ordered([
    ['dialect', p.dialect],
    ['name', p.name, 'string'],
    ['globals', p.globals.map(variableJson), 'list'],
    ['pous', p.pous.map(pouJson)],
    ['declarations', p.declarations, 'list'],
    ['tasks', p.tasks.map(taskJson), 'list'],
  ])
}

/** The model as pretty JSON text (two-space indent, like `plcc convert --to ladder-json`). */
export function toLadderJson(p: Pick<Project, 'dialect' | 'name' | 'globals' | 'pous' | 'declarations' | 'tasks'>): string {
  return `${JSON.stringify(toLadderModel(p), null, 2)}\n`
}

// ---------------------------------------------------------------- reading

function isObj(v: unknown): v is Obj {
  return typeof v === 'object' && v !== null && !Array.isArray(v)
}

class Reader {
  str(o: Obj, k: string, at: string, optional = false): string | undefined {
    const v = o[k]
    if (v === undefined && optional) return undefined
    if (typeof v !== 'string') throw new ModelFormatError(at, `"${k}" must be a string`)
    return v
  }
  num(o: Obj, k: string, at: string, optional = false): number | undefined {
    const v = o[k]
    if (v === undefined && optional) return undefined
    if (typeof v !== 'number' || !Number.isFinite(v)) throw new ModelFormatError(at, `"${k}" must be a number`)
    return v
  }
  id(o: Obj, at: string): number {
    const v = this.num(o, 'id', at) as number
    if (!Number.isInteger(v) || v < 0) throw new ModelFormatError(at, '"id" must be a non-negative integer')
    return v
  }
  bool(o: Obj, k: string, at: string): boolean | undefined {
    const v = o[k]
    if (v === undefined) return undefined
    if (typeof v !== 'boolean') throw new ModelFormatError(at, `"${k}" must be true or false`)
    return v
  }
  list(o: Obj, k: string, at: string, optional = true): unknown[] {
    const v = o[k]
    if (v === undefined && optional) return []
    if (!Array.isArray(v)) throw new ModelFormatError(at, `"${k}" must be a list`)
    return v
  }
  strings(o: Obj, k: string, at: string): string[] | undefined {
    const v = o[k]
    if (v === undefined) return undefined
    if (!Array.isArray(v) || !v.every((x) => typeof x === 'string')) throw new ModelFormatError(at, `"${k}" must be a list of strings`)
    return v as string[]
  }
  oneOf<T extends string>(o: Obj, k: string, at: string, options: readonly T[], fallback?: T): T {
    const v = o[k]
    if (v === undefined && fallback !== undefined) return fallback
    if (typeof v !== 'string' || !(options as readonly string[]).includes(v)) {
      throw new ModelFormatError(at, `"${k}" must be one of ${options.join(', ')}, not ${JSON.stringify(v)}`)
    }
    return v as T
  }
  obj(v: unknown, at: string): Obj {
    if (!isObj(v)) throw new ModelFormatError(at, 'expected an object')
    return v
  }

  variable(v: unknown, at: string): Variable {
    const o = this.obj(v, at)
    const out: Variable = {
      name: this.str(o, 'name', at) as string,
      data_type: this.str(o, 'data_type', at) as string,
      section: this.oneOf(o, 'section', at, ['local', 'input', 'output', 'in_out', 'external', 'temp', 'global'] as const, 'local'),
    }
    for (const k of ['initial', 'address', 'comment'] as const) {
      const s = this.str(o, k, at, true)
      if (s !== undefined) out[k] = s
    }
    if (this.bool(o, 'constant', at)) out.constant = true
    if (this.bool(o, 'retain', at)) out.retain = true
    return out
  }

  notes(o: Obj, at: string): string[] | undefined {
    const n = this.strings(o, 'notes', at)
    return n && n.length ? n : undefined
  }

  pin(v: unknown, at: string): Pin {
    const o = this.obj(v, at)
    const p: Pin = { name: this.str(o, 'name', at) as string, dir: this.oneOf(o, 'dir', at, ['input', 'output', 'in_out'] as const, 'input') }
    const value = this.str(o, 'value', at, true)
    if (value !== undefined) p.value = value
    if (this.bool(o, 'negated', at)) p.negated = true
    if (o.rung !== undefined) p.rung = this.series(o.rung, `${at}.rung`)
    return p
  }

  series(v: unknown, at: string): Element[] {
    if (!Array.isArray(v)) throw new ModelFormatError(at, 'expected a list of elements')
    return v.map((e, i) => this.element(e, `${at}[${i}]`))
  }

  element(v: unknown, at: string): Element {
    const o = this.obj(v, at)
    const id = this.id(o, at)
    const notes = this.notes(o, at)
    const withNotes = <T extends Element>(e: T): T => (notes ? { ...e, notes } : e)
    switch (o.type) {
      case 'contact':
        return withNotes({ type: 'contact', id, operand: this.str(o, 'operand', at) as string, kind: this.oneOf(o, 'kind', at, ['no', 'nc', 'rising', 'falling'] as const, 'no') })
      case 'coil':
        return withNotes({
          type: 'coil', id, operand: this.str(o, 'operand', at) as string,
          kind: this.oneOf(o, 'kind', at, ['normal', 'negated', 'set', 'reset', 'rising', 'falling'] as const, 'normal'),
        })
      case 'branch':
        return { type: 'branch', id, legs: this.list(o, 'legs', at, false).map((l, i) => this.series(l, `${at}.legs[${i}]`)) }
      case 'block': {
        const b: Element = {
          type: 'block', id, name: this.str(o, 'name', at) as string,
          pins: this.list(o, 'pins', at).map((p, i) => this.pin(p, `${at}.pins[${i}]`)),
        }
        const instance = this.str(o, 'instance', at, true)
        if (instance !== undefined) b.instance = instance
        const pi = this.str(o, 'power_in', at, true)
        if (pi !== undefined) b.power_in = pi
        const po = this.str(o, 'power_out', at, true)
        if (po !== undefined) b.power_out = po
        return withNotes(b)
      }
      case 'jump':
        return { type: 'jump', id, label: this.str(o, 'label', at) as string }
      case 'return':
        return { type: 'return', id }
      case 'st':
        return withNotes({ type: 'st', id, code: this.str(o, 'code', at) as string })
      default:
        throw new ModelFormatError(at, `unknown element type ${JSON.stringify(o.type)}`)
    }
  }

  rung(v: unknown, at: string): Rung {
    const o = this.obj(v, at)
    const r: Rung = { id: this.id(o, at), elements: this.series(o.elements ?? [], `${at}.elements`) }
    const c = this.str(o, 'comment', at, true)
    if (c !== undefined) r.comment = c
    const l = this.str(o, 'label', at, true)
    if (l !== undefined) r.label = l
    return r
  }

  routine(v: unknown, at: string): Routine {
    const o = this.obj(v, at)
    return {
      id: this.id(o, at),
      name: this.str(o, 'name', at) as string,
      rungs: this.list(o, 'rungs', at, false).map((r, i) => this.rung(r, `${at}.rungs[${i}]`)),
    }
  }

  pou(v: unknown, at: string): Pou {
    const o = this.obj(v, at)
    const p: Pou = {
      id: this.id(o, at),
      name: this.str(o, 'name', at) as string,
      kind: this.oneOf(o, 'kind', at, ['program', 'function_block', 'function'] as const, 'program'),
      variables: this.list(o, 'variables', at).map((x, i) => this.variable(x, `${at}.variables[${i}]`)),
      routines: this.list(o, 'routines', at, false).map((x, i) => this.routine(x, `${at}.routines[${i}]`)),
    }
    const rt = this.str(o, 'return_type', at, true)
    if (rt !== undefined) p.return_type = rt
    const m = this.str(o, 'members', at, true)
    if (m) p.members = m
    return p
  }

  task(v: unknown, at: string): Task {
    const o = this.obj(v, at)
    const t: Task = { name: this.str(o, 'name', at) as string, programs: this.strings(o, 'programs', at) ?? [] }
    const ms = this.num(o, 'interval_ms', at, true)
    if (ms !== undefined) {
      if (!(ms > 0)) throw new ModelFormatError(at, '"interval_ms" must be positive')
      t.interval_ms = ms
    }
    const pr = this.num(o, 'priority', at, true)
    if (pr !== undefined) t.priority = pr
    return t
  }
}

/** Reads plcc-ladder JSON (text or parsed) into the model; throws ModelFormatError. */
export function fromLadderJson(input: string | unknown, file = 'project.json'): Required<Omit<LadderModel, 'name'>> & { name: string } {
  let doc: unknown = input
  if (typeof input === 'string') {
    try {
      doc = JSON.parse(input)
    } catch (e) {
      throw new ModelFormatError(file, `invalid JSON: ${e instanceof Error ? e.message : String(e)}`)
    }
  }
  const r = new Reader()
  const o = r.obj(doc, file)
  return {
    dialect: r.oneOf(o, 'dialect', file, ['iec', 'logix'] as const, 'iec'),
    name: r.str(o, 'name', file, true) ?? '',
    globals: r.list(o, 'globals', file).map((v, i) => r.variable(v, `${file}: globals[${i}]`)),
    pous: r.list(o, 'pous', file, false).map((v, i) => r.pou(v, `${file}: pous[${i}]`)),
    declarations: r.strings(o, 'declarations', file) ?? [],
    tasks: r.list(o, 'tasks', file).map((v, i) => r.task(v, `${file}: tasks[${i}]`)),
  }
}
