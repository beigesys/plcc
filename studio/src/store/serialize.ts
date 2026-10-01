// SPDX-License-Identifier: MPL-2.0
//
// The on-disk project format. All knowledge of file names lives here.
//
//   project.toml          format = 2, name, [[devices]] (name, manifest)
//   project.json          the plcc-ladder model (docs/ladder-translation.md):
//                         `plcc convert project.json --to l5x` works on it
//   devices/<id>.toml     device manifests (docs/device-manifest.md)
//
// Projects saved by the first studio (format 1: tags.toml and
// routines/<name>.ladder.json in its draft model, or older still, devices
// named by profile) are migrated when read (src/model/migrate.ts) and saved
// in format 2.

import { parse as parseToml, stringify as stringifyToml } from 'smol-toml'
import {
  fromLadderJson, migratePrograms, migrateTag, migrateTask, ModelFormatError, toLadderJson,
  type DraftProgram, type DraftRoutine, type Migration, type Project,
} from '@/model'
import { DEVICE_PATH_RE, legacyProfileManifest } from '@/devices/project'

export const PROJECT_FORMAT = 2
export const MODEL_FILE = 'project.json'

export class ProjectFormatError extends Error {
  readonly file: string
  constructor(file: string, message: string) {
    super(`${file}: ${message}`)
    this.name = 'ProjectFormatError'
    this.file = file
  }
}

export function projectToFiles(p: Project): Record<string, string> {
  const files: Record<string, string> = {}
  files['project.toml'] = stringifyToml({
    format: PROJECT_FORMAT,
    name: p.name,
    devices: p.devices.map((d) => ({ name: d.name, manifest: d.manifest })),
  })
  files[MODEL_FILE] = toLadderJson(p)
  for (const d of p.devices) {
    const text = p.deviceFiles[d.manifest]
    if (text !== undefined) files[d.manifest] = text
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

function readDevices(files: Record<string, string>, pt: Obj): Pick<Project, 'devices' | 'deviceFiles'> {
  const deviceFiles: Record<string, string> = {}
  const devices = arr('project.toml', pt, 'devices', 'project').map((d, i) => {
    const where = `devices[${i}]`
    const name = str('project.toml', d, 'name', where)
    if (d.manifest === undefined) {
      // Saved before device manifests: `profile = "<catalog id>"`.
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
  return { devices, deviceFiles }
}

export interface LoadedProject {
  project: Project
  /** Set when the files were in an older format: what changed. */
  migrated?: { notes: string[] }
}

/** Reads a project, migrating older formats. */
export function loadProjectFiles(files: Record<string, string>): LoadedProject {
  const pt = readToml('project.toml', files['project.toml'])
  const format = typeof pt.format === 'number' ? pt.format : 1
  if (format > PROJECT_FORMAT) {
    throw new ProjectFormatError('project.toml', `format ${format} is newer than this studio (${PROJECT_FORMAT}); update the page`)
  }
  const dev = readDevices(files, pt)
  if (format === PROJECT_FORMAT) {
    const text = files[MODEL_FILE]
    if (text === undefined) throw new ProjectFormatError(MODEL_FILE, 'file is missing')
    let model
    try {
      model = fromLadderJson(text, MODEL_FILE)
    } catch (e) {
      if (e instanceof ModelFormatError) throw new ProjectFormatError(MODEL_FILE, e.message.replace(`${MODEL_FILE}: `, ''))
      throw e
    }
    const name = str('project.toml', pt, 'name', 'project', model.name)
    return { project: { ...model, name, ...dev } }
  }
  return migrateDraft(files, pt, dev)
}

/** Reads a project (see loadProjectFiles). */
export function projectFromFiles(files: Record<string, string>): Project {
  return loadProjectFiles(files).project
}

/** Whether a project's files are in an older format and should be saved again. */
export function projectNeedsMigration(files: Record<string, string>): boolean {
  try {
    const pt = parseToml(files['project.toml'] ?? '') as Obj
    if (pt.format !== PROJECT_FORMAT) return true
    const devs = Array.isArray(pt.devices) ? pt.devices : []
    return devs.some((d) => isObj(d) && d.manifest === undefined)
  } catch {
    return false
  }
}

// ---------------------------------------------------------------- format 1

const DRAFT_LADDER = 'plcc-studio-ladder'

function migrateDraft(files: Record<string, string>, pt: Obj, dev: Pick<Project, 'devices' | 'deviceFiles'>): LoadedProject {
  const name = str('project.toml', pt, 'name', 'project')
  const tasks = arr('project.toml', pt, 'tasks', 'project').map((t, i) => {
    const where = `tasks[${i}]`
    const interval = t.interval_ms
    if (typeof interval !== 'number' || !(interval > 0)) {
      throw new ProjectFormatError('project.toml', `${where}: "interval_ms" must be a positive number`)
    }
    return migrateTask({ name: str('project.toml', t, 'name', where), intervalMs: interval, programs: strList('project.toml', t, 'programs', where) })
  })
  const programs: DraftProgram[] = arr('project.toml', pt, 'programs', 'project').map((pr, i) => {
    const where = `programs[${i}]`
    const routines: DraftRoutine[] = arr('project.toml', pr, 'routines', where).map((r, j) => {
      const rw = `${where}.routines[${j}]`
      const rname = str('project.toml', r, 'name', rw)
      const kind = str('project.toml', r, 'kind', rw, 'ladder')
      if (kind !== 'ladder' && kind !== 'st') throw new ProjectFormatError('project.toml', `${rw}: "kind" must be "ladder" or "st"`)
      const file = `routines/${rname}${kind === 'st' ? '.st' : '.ladder.json'}`
      const text = files[file]
      if (text === undefined) throw new ProjectFormatError(file, 'file is missing')
      if (kind === 'st') return { name: rname, kind, st: text }
      let doc: unknown
      try {
        doc = JSON.parse(text)
      } catch (e) {
        throw new ProjectFormatError(file, `invalid JSON: ${e instanceof Error ? e.message : String(e)}`)
      }
      if (!isObj(doc) || doc.format !== DRAFT_LADDER || !Array.isArray(doc.rungs)) {
        throw new ProjectFormatError(file, `not a ${DRAFT_LADDER} file`)
      }
      return { name: rname, kind, rungs: doc.rungs as Obj[] }
    })
    return { name: str('project.toml', pr, 'name', where), main: str('project.toml', pr, 'main', where, routines[0]?.name ?? ''), routines }
  })
  const m: Migration = { tags: [], notes: [] }
  const pous = migratePrograms(programs, m)
  let globals = m.tags
  if (files['tags.toml'] !== undefined) {
    const tt = readToml('tags.toml', files['tags.toml'])
    globals = [
      ...arr('tags.toml', tt, 'tag', 'tags').map((t, i) => {
        const where = `tag[${i}]`
        return migrateTag({
          name: str('tags.toml', t, 'name', where),
          type: str('tags.toml', t, 'type', where),
          initial: str('tags.toml', t, 'initial', where, ''),
          address: t.address === undefined ? undefined : str('tags.toml', t, 'address', where),
          comment: str('tags.toml', t, 'comment', where, ''),
        })
      }),
      ...m.tags,
    ]
  }
  return {
    project: { dialect: 'logix', name, globals, pous, declarations: [], tasks, ...dev },
    migrated: { notes: m.notes },
  }
}
