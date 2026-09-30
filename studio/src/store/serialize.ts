// SPDX-License-Identifier: MPL-2.0
//
// The on-disk project format. All knowledge of file names and schemas lives
// here, so switching to the plcc-ladder crate's JSON is a change to this file.
//
//   project.toml                   name, [[devices]] (name, manifest), [[tasks]], [[programs]]
//   devices/<id>.toml              device manifests (docs/device-manifest.md)
//   tags.toml                      [[tag]] name, type, initial, address?, comment
//   routines/<name>.ladder.json    { format, version, name, rungs }
//   routines/<name>.st             ST routine source

import { parse as parseToml, stringify as stringifyToml } from 'smol-toml'
import type { Element, Project, Routine, RoutineKind, Rung, Series, Tag } from '@/model'
import { BOX_INSTRS } from '@/model'
import { DEVICE_PATH_RE, legacyProfileManifest } from '@/devices/project'

export const LADDER_FORMAT = 'plcc-studio-ladder'
export const LADDER_VERSION = 1

export class ProjectFormatError extends Error {
  readonly file: string
  constructor(file: string, message: string) {
    super(`${file}: ${message}`)
    this.name = 'ProjectFormatError'
    this.file = file
  }
}

export function routineFileName(r: { name: string; kind: RoutineKind }): string {
  return `routines/${r.name}${r.kind === 'st' ? '.st' : '.ladder.json'}`
}

export function projectToFiles(p: Project): Record<string, string> {
  const files: Record<string, string> = {}
  files['project.toml'] = stringifyToml({
    name: p.name,
    devices: p.devices.map((d) => ({ name: d.name, manifest: d.manifest })),
    tasks: p.tasks.map((t) => ({ name: t.name, interval_ms: t.intervalMs, programs: [...t.programs] })),
    programs: p.programs.map((pr) => ({
      name: pr.name,
      main: pr.main,
      routines: pr.routines.map((r) => ({ name: r.name, kind: r.kind })),
    })),
  })
  files['tags.toml'] = stringifyToml({
    tag: p.tags.map((t) => {
      const o: Record<string, string> = { name: t.name, type: t.type, initial: t.initial }
      if (t.address) o.address = t.address
      o.comment = t.comment
      return o
    }),
  })
  for (const d of p.devices) {
    const text = p.deviceFiles[d.manifest]
    if (text !== undefined) files[d.manifest] = text
  }
  for (const pr of p.programs) {
    for (const r of pr.routines) {
      files[routineFileName(r)] =
        r.kind === 'st'
          ? (r.st ?? '')
          : `${JSON.stringify({ format: LADDER_FORMAT, version: LADDER_VERSION, name: r.name, rungs: r.rungs }, null, 2)}\n`
    }
  }
  return files
}

// ---------------------------------------------------------------- reading

type Obj = Record<string, unknown>

function isObj(v: unknown): v is Obj {
  return typeof v === 'object' && v !== null && !Array.isArray(v)
}

function readToml(file: string, text: string | undefined): Obj {
  if (text === undefined) throw new ProjectFormatError(file, 'file is missing')
  try {
    return parseToml(text) as Obj
  } catch (e) {
    throw new ProjectFormatError(file, `invalid TOML: ${e instanceof Error ? e.message.split('\n')[0] : String(e)}`)
  }
}

function str(file: string, o: Obj, key: string, where: string, fallback?: string): string {
  const v = o[key]
  if (v === undefined && fallback !== undefined) return fallback
  if (typeof v !== 'string') throw new ProjectFormatError(file, `${where}: "${key}" must be a string`)
  return v
}

function arr(file: string, o: Obj, key: string, where: string): Obj[] {
  const v = o[key]
  if (v === undefined) return []
  if (!Array.isArray(v) || !v.every(isObj)) throw new ProjectFormatError(file, `${where}: "${key}" must be a list of tables`)
  return v
}

function strList(file: string, o: Obj, key: string, where: string): string[] {
  const v = o[key]
  if (v === undefined) return []
  if (!Array.isArray(v) || !v.every((x) => typeof x === 'string')) {
    throw new ProjectFormatError(file, `${where}: "${key}" must be a list of strings`)
  }
  return v as string[]
}

const CONTACT_KINDS = ['no', 'nc', 'rise', 'fall']
const COIL_KINDS = ['normal', 'negated', 'set', 'reset', 'rise', 'fall']

function checkElement(file: string, e: unknown, path: string): Element {
  if (!isObj(e)) throw new ProjectFormatError(file, `${path}: element must be an object`)
  const id = () => {
    if (typeof e.id !== 'string' || !e.id) throw new ProjectFormatError(file, `${path}: missing "id"`)
    return e.id
  }
  switch (e.type) {
    case 'series':
      return checkSeries(file, e, path)
    case 'parallel': {
      if (!Array.isArray(e.branches)) throw new ProjectFormatError(file, `${path}: parallel needs "branches"`)
      return {
        type: 'parallel',
        id: id(),
        branches: e.branches.map((b, i) => checkSeries(file, b, `${path}.branches[${i}]`)),
      }
    }
    case 'contact':
    case 'coil': {
      const kinds = e.type === 'contact' ? CONTACT_KINDS : COIL_KINDS
      if (typeof e.kind !== 'string' || !kinds.includes(e.kind)) {
        throw new ProjectFormatError(file, `${path}: bad ${e.type} kind ${JSON.stringify(e.kind)}`)
      }
      if (typeof e.tag !== 'string') throw new ProjectFormatError(file, `${path}: "tag" must be a string`)
      return { type: e.type, id: id(), kind: e.kind, tag: e.tag } as Element
    }
    case 'box': {
      if (typeof e.instr !== 'string' || !(BOX_INSTRS as readonly string[]).includes(e.instr)) {
        throw new ProjectFormatError(file, `${path}: unknown instruction ${JSON.stringify(e.instr)}`)
      }
      if (!isObj(e.operands) || !Object.values(e.operands).every((v) => typeof v === 'string')) {
        throw new ProjectFormatError(file, `${path}: "operands" must map names to strings`)
      }
      return { type: 'box', id: id(), instr: e.instr, operands: { ...(e.operands as Record<string, string>) } } as Element
    }
    case 'st':
      if (typeof e.code !== 'string') throw new ProjectFormatError(file, `${path}: "code" must be a string`)
      return { type: 'st', id: id(), code: e.code }
    default:
      throw new ProjectFormatError(file, `${path}: unknown element type ${JSON.stringify(e.type)}`)
  }
}

function checkSeries(file: string, s: unknown, path: string): Series {
  if (!isObj(s) || s.type !== 'series' || !Array.isArray(s.items)) {
    throw new ProjectFormatError(file, `${path}: expected a series ({ "type": "series", "items": [...] })`)
  }
  return { type: 'series', items: s.items.map((e, i) => checkElement(file, e, `${path}.items[${i}]`)) }
}

function readLadder(file: string, text: string, name: string): Rung[] {
  let doc: unknown
  try {
    doc = JSON.parse(text)
  } catch (e) {
    throw new ProjectFormatError(file, `invalid JSON: ${e instanceof Error ? e.message : String(e)}`)
  }
  if (!isObj(doc)) throw new ProjectFormatError(file, 'expected a JSON object')
  if (doc.format !== LADDER_FORMAT) throw new ProjectFormatError(file, `"format" must be "${LADDER_FORMAT}"`)
  if (typeof doc.version !== 'number' || doc.version > LADDER_VERSION) {
    throw new ProjectFormatError(file, `unsupported version ${JSON.stringify(doc.version)}`)
  }
  if (doc.name !== undefined && doc.name !== name) {
    throw new ProjectFormatError(file, `routine name "${String(doc.name)}" does not match project.toml ("${name}")`)
  }
  if (!Array.isArray(doc.rungs)) throw new ProjectFormatError(file, '"rungs" must be a list')
  return doc.rungs.map((r, i) => {
    const where = `rungs[${i}]`
    if (!isObj(r)) throw new ProjectFormatError(file, `${where}: rung must be an object`)
    if (typeof r.id !== 'string' || !r.id) throw new ProjectFormatError(file, `${where}: missing "id"`)
    const comment = r.comment === undefined ? '' : r.comment
    if (typeof comment !== 'string') throw new ProjectFormatError(file, `${where}: "comment" must be a string`)
    return { id: r.id, comment, body: checkSeries(file, r.body, `${where}.body`) }
  })
}

export function projectFromFiles(files: Record<string, string>): Project {
  const pt = readToml('project.toml', files['project.toml'])
  const name = str('project.toml', pt, 'name', 'project')

  const deviceFiles: Record<string, string> = {}
  const devices = arr('project.toml', pt, 'devices', 'project').map((d, i) => {
    const where = `devices[${i}]`
    const name = str('project.toml', d, 'name', where)
    if (d.manifest === undefined) {
      // Saved before device manifests: `profile = "<catalog id>"`. The catalog's
      // manifest becomes the project's copy (see projectNeedsMigration).
      const legacy = legacyProfileManifest(str('project.toml', d, 'profile', where))
      deviceFiles[legacy.path] ??= files[legacy.path] ?? legacy.text
      return { name, manifest: legacy.path }
    }
    const manifest = str('project.toml', d, 'manifest', where)
    if (!DEVICE_PATH_RE.test(manifest)) {
      throw new ProjectFormatError('project.toml', `${where}: "manifest" must be devices/<id>.toml, not "${manifest}"`)
    }
    const text = files[manifest]
    if (text === undefined) throw new ProjectFormatError(manifest, 'file is missing')
    deviceFiles[manifest] = text
    return { name, manifest }
  })

  const tasks = arr('project.toml', pt, 'tasks', 'project').map((t, i) => {
    const where = `tasks[${i}]`
    const interval = t.interval_ms
    if (typeof interval !== 'number' || !(interval > 0)) {
      throw new ProjectFormatError('project.toml', `${where}: "interval_ms" must be a positive number`)
    }
    return {
      name: str('project.toml', t, 'name', where),
      intervalMs: Number(interval),
      programs: strList('project.toml', t, 'programs', where),
    }
  })

  const seen = new Set<string>()
  const programs = arr('project.toml', pt, 'programs', 'project').map((pr, i) => {
    const where = `programs[${i}]`
    const routines: Routine[] = arr('project.toml', pr, 'routines', where).map((r, j) => {
      const rw = `${where}.routines[${j}]`
      const rname = str('project.toml', r, 'name', rw)
      const kind = str('project.toml', r, 'kind', rw, 'ladder')
      if (kind !== 'ladder' && kind !== 'st') {
        throw new ProjectFormatError('project.toml', `${rw}: "kind" must be "ladder" or "st"`)
      }
      if (seen.has(rname.toLowerCase())) {
        throw new ProjectFormatError('project.toml', `${rw}: duplicate routine name "${rname}"`)
      }
      seen.add(rname.toLowerCase())
      const file = routineFileName({ name: rname, kind })
      const text = files[file]
      if (text === undefined) throw new ProjectFormatError(file, 'file is missing')
      return kind === 'st'
        ? { name: rname, kind, rungs: [], st: text }
        : { name: rname, kind, rungs: readLadder(file, text, rname) }
    })
    const prName = str('project.toml', pr, 'name', where)
    const main = str('project.toml', pr, 'main', where, routines[0]?.name ?? '')
    return { name: prName, main, routines }
  })

  let tags: Tag[] = []
  if (files['tags.toml'] !== undefined) {
    const tt = readToml('tags.toml', files['tags.toml'])
    tags = arr('tags.toml', tt, 'tag', 'tags').map((t, i) => {
      const where = `tag[${i}]`
      const tag: Tag = {
        name: str('tags.toml', t, 'name', where),
        type: str('tags.toml', t, 'type', where),
        initial: str('tags.toml', t, 'initial', where, ''),
        comment: str('tags.toml', t, 'comment', where, ''),
      }
      if (t.address !== undefined) tag.address = str('tags.toml', t, 'address', where)
      return tag
    })
  }

  return { name, devices, deviceFiles, tasks, programs, tags }
}

/** Whether a project's files predate device manifests and should be saved again. */
export function projectNeedsMigration(files: Record<string, string>): boolean {
  try {
    const pt = parseToml(files['project.toml'] ?? '') as Obj
    const devs = Array.isArray(pt.devices) ? pt.devices : []
    return devs.some((d) => isObj(d) && d.manifest === undefined)
  } catch {
    return false
  }
}
