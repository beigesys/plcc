// SPDX-License-Identifier: MPL-2.0
//
// Projects on a FileStore: `<root>/<id>/project.toml`, `project.json` (the
// plcc-ladder model), `devices/*`. The id is a directory name and never
// changes; the display name lives in project.toml.

import { strFromU8, strToU8, unzipSync, zipSync } from 'fflate'
import { parse as parseToml, stringify as stringifyToml } from 'smol-toml'
import type { Project } from '@/model'
import { SubFileStore, type FileStore } from './fs'
import { ProjectFolder } from './folder'
import { ProjectFormatError, projectFromFiles, projectToFiles, type LoadedProject } from './serialize'
import { reserveProjectIds } from '@/model'

export interface ProjectSummary {
  id: string
  name: string
}

export function slugify(name: string): string {
  const s = name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 40)
  return s || 'project'
}

function shortSuffix(): string {
  return Math.floor(Math.random() * 36 ** 4)
    .toString(36)
    .padStart(4, '0')
}

export class ProjectRepo {
  readonly store: FileStore
  readonly root: string

  constructor(store: FileStore, root = 'projects') {
    this.store = store
    this.root = root
  }

  private dir(id: string): string {
    if (!id || id.includes('/') || id === '.' || id === '..') throw new Error(`bad project id "${id}"`)
    return `${this.root}/${id}`
  }

  async list(): Promise<ProjectSummary[]> {
    const out: ProjectSummary[] = []
    for (const e of await this.store.list(this.root)) {
      if (e.kind !== 'dir') continue
      const text = await this.store.readText(`${this.root}/${e.name}/project.toml`)
      if (text === null) continue
      let name = e.name
      try {
        const t = parseToml(text)
        if (typeof t.name === 'string') name = t.name
      } catch {
        // A broken project still shows up, under its id, so it can be deleted.
      }
      out.push({ id: e.name, name })
    }
    return out.sort((a, b) => a.name.localeCompare(b.name))
  }

  /** The project's folder, to read, save and watch it. */
  folder(id: string): ProjectFolder {
    return new ProjectFolder(new SubFileStore(this.store, this.dir(id)))
  }

  async load(id: string): Promise<Project> {
    return (await this.loadWithNotes(id)).project
  }

  /** Loads a project; one in an older format is migrated and saved, with what changed. */
  async loadWithNotes(id: string): Promise<LoadedProject> {
    if (!(await this.exists(id))) throw new Error(`project "${id}" not found`)
    return this.folder(id).load()
  }

  async save(id: string, project: Project): Promise<void> {
    await this.folder(id).save(project, { force: true })
  }

  async exists(id: string): Promise<boolean> {
    return (await this.store.readText(`${this.dir(id)}/project.toml`)) !== null
  }

  async create(project: Project): Promise<string> {
    const base = slugify(project.name)
    let id = `${base}-${shortSuffix()}`
    while (await this.exists(id)) id = `${base}-${shortSuffix()}`
    await this.save(id, project)
    return id
  }

  async rename(id: string, name: string): Promise<void> {
    const path = `${this.dir(id)}/project.toml`
    const text = await this.store.readText(path)
    if (text === null) throw new Error(`project "${id}" not found`)
    let t: Record<string, unknown>
    try {
      t = parseToml(text)
    } catch (e) {
      throw new ProjectFormatError('project.toml', e instanceof Error ? e.message : String(e))
    }
    t.name = name
    await this.store.writeText(path, stringifyToml(t))
  }

  async remove(id: string): Promise<void> {
    await this.store.remove(this.dir(id))
  }

  exportZip(project: Project): Uint8Array {
    const top = slugify(project.name)
    const entries: Record<string, Uint8Array> = {}
    for (const [path, text] of Object.entries(projectToFiles(project))) entries[`${top}/${path}`] = strToU8(text)
    return zipSync(entries, { level: 6 })
  }

  /** Parses a project zip (with or without one top folder) without saving it. */
  static projectFromZip(bytes: Uint8Array): Project {
    let raw: Record<string, Uint8Array>
    try {
      raw = unzipSync(bytes)
    } catch (e) {
      throw new ProjectFormatError('zip', `not a valid zip archive (${e instanceof Error ? e.message : String(e)})`)
    }
    const paths = Object.keys(raw).filter((p) => !p.endsWith('/'))
    const tops = new Set(paths.map((p) => (p.includes('/') ? p.split('/')[0] : '')))
    const strip = tops.size === 1 && !tops.has('') && !paths.includes('project.toml') ? `${[...tops][0]}/` : ''
    const files: Record<string, string> = {}
    for (const p of paths) files[p.startsWith(strip) ? p.slice(strip.length) : p] = strFromU8(raw[p])
    const p = projectFromFiles(files)
    reserveProjectIds(p)
    return p
  }

  async importZip(bytes: Uint8Array): Promise<string> {
    return this.create(ProjectRepo.projectFromZip(bytes))
  }

  async ensureSeeded(seed: () => Project): Promise<string> {
    const list = await this.list()
    if (list.length > 0) return list[0].id
    return this.create(seed())
  }
}

export type AutosaveStatus = 'idle' | 'pending' | 'saving' | 'saved' | 'error'

export interface Autosaver {
  schedule(id: string, project: Project): void
  flush(): Promise<void>
  cancel(): void
}

/** Debounced saves: the last project scheduled within `delayMs` wins. */
export function createAutosaver(
  repo: { save(id: string, project: Project): Promise<unknown> },
  delayMs: number,
  onStatus: (status: AutosaveStatus, error?: unknown) => void = () => {},
): Autosaver {
  let timer: ReturnType<typeof setTimeout> | undefined
  let pending: { id: string; project: Project } | undefined
  let running: Promise<void> = Promise.resolve()

  const run = (): Promise<void> => {
    if (timer !== undefined) clearTimeout(timer)
    timer = undefined
    const job = pending
    pending = undefined
    if (!job) return running
    running = running.then(async () => {
      onStatus('saving')
      try {
        await repo.save(job.id, job.project)
        onStatus(pending ? 'pending' : 'saved')
      } catch (e) {
        onStatus('error', e)
      }
    })
    return running
  }

  return {
    schedule(id, project) {
      if (pending && pending.id !== id) void run()
      pending = { id, project }
      if (timer !== undefined) clearTimeout(timer)
      timer = setTimeout(() => void run(), delayMs)
      onStatus('pending')
    },
    flush: run,
    cancel() {
      if (timer !== undefined) clearTimeout(timer)
      timer = undefined
      pending = undefined
      onStatus('idle')
    },
  }
}
