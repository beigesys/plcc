// SPDX-License-Identifier: MPL-2.0
//
// One project in one folder: a folder on disk the user picked, or
// `projects/<id>/` in the browser's storage. ProjectFolder reads and writes
// only the project's own files (project.toml, project.json, devices/*.toml),
// writes only the files whose contents changed, and notices when another
// program (git, an editor) changed them since it last read or wrote them.

import { parse as parseToml } from 'smol-toml'
import { reserveProjectIds, type Project } from '@/model'
import type { FileStore } from './fs'
import { loadProjectFiles, MODEL_FILE, ProjectFormatError, projectNeedsMigration, projectToFiles, type LoadedProject } from './serialize'

export const MANIFEST_FILE = 'project.toml'
export const README_FILE = 'README.md'
export const GITIGNORE_FILE = '.gitignore'
/** Untracked, per-checkout state; a new project's .gitignore lists it. */
export const LOCAL_DIR = '.plcc'

/** What a picked folder holds. Hidden entries (.git, .DS_Store) do not count. */
export type FolderContents =
  | { kind: 'empty' }
  | { kind: 'project'; name: string }
  | { kind: 'other'; entries: string[] }

export async function inspectFolder(store: FileStore, fallbackName = ''): Promise<FolderContents> {
  const entries = await store.list('')
  if (entries.some((e) => e.kind === 'file' && e.name === MANIFEST_FILE)) {
    let name = fallbackName
    try {
      const t = parseToml((await store.readText(MANIFEST_FILE)) ?? '')
      if (typeof t.name === 'string') name = t.name
    } catch {
      // A broken project.toml is still a project; loading it reports why.
    }
    return { kind: 'project', name }
  }
  const visible = entries.filter((e) => !e.name.startsWith('.')).map((e) => e.name)
  return visible.length ? { kind: 'other', entries: visible } : { kind: 'empty' }
}

/** The project's files changed on disk since they were last read or written. */
export class ExternalChangeError extends Error {
  readonly paths: string[]
  constructor(paths: string[]) {
    super(`changed on disk: ${paths.join(', ')}`)
    this.name = 'ExternalChangeError'
    this.paths = paths
  }
}

export function readmeText(name: string): string {
  return `# ${name}

A PLC project for [plcc studio](https://github.com/beigesys/plcc/tree/main/studio).

Open it in plcc studio (Chrome or Edge): **File > Open project…** and pick
this folder. The studio saves to these files as you edit.

| File | Contents |
|---|---|
| \`project.toml\` | the project's name and its devices |
| \`project.json\` | the program, in plcc's ladder model: \`plcc convert project.json --to l5x\` works on it |
| \`devices/*.toml\` | the device manifests |
`
}

export const GITIGNORE_TEXT = `# plcc studio: local state that does not belong in version control
${LOCAL_DIR}/
`

function stampOf(st: { lastModified: number; size: number } | null): string {
  return st ? `${st.lastModified}:${st.size}` : 'missing'
}

/** Files of the first studio's format, replaced by project.json when migrated. */
const LEGACY = ['tags.toml', 'routines']

export class ProjectFolder {
  readonly store: FileStore
  /** Tracked path → its contents as last read or written. */
  private snapshot = new Map<string, string>()
  /** Tracked path → `lastModified:size` as last seen. */
  private stamps = new Map<string, string>()
  private legacy = false

  constructor(store: FileStore) {
    this.store = store
  }

  private async readDir(dir: string, files: Record<string, string>, filter: (name: string) => boolean = () => true) {
    for (const e of await this.store.list(dir)) {
      if (e.kind !== 'file' || !filter(e.name)) continue
      const t = await this.store.readText(`${dir}/${e.name}`)
      if (t !== null) files[`${dir}/${e.name}`] = t
    }
  }

  /** The project's files (and a first-studio project's), nothing else in the folder. */
  private async readProjectFiles(): Promise<Record<string, string>> {
    const files: Record<string, string> = {}
    for (const f of [MANIFEST_FILE, MODEL_FILE, 'tags.toml']) {
      const t = await this.store.readText(f)
      if (t !== null) files[f] = t
    }
    await this.readDir('devices', files, (n) => n.endsWith('.toml'))
    if (files['tags.toml'] !== undefined || !(MODEL_FILE in files)) await this.readDir('routines', files)
    return files
  }

  private async remember(files: Record<string, string>) {
    this.snapshot = new Map(Object.entries(files))
    this.stamps = new Map()
    for (const path of this.snapshot.keys()) this.stamps.set(path, stampOf(await this.store.stat(path)))
  }

  /** Reads the project; one in an older format is migrated and saved, with what changed. */
  async load(): Promise<LoadedProject> {
    const files = await this.readProjectFiles()
    if (files[MANIFEST_FILE] === undefined) throw new ProjectFormatError(MANIFEST_FILE, 'file is missing: this folder is not a plcc project')
    const loaded = loadProjectFiles(files)
    reserveProjectIds(loaded.project)
    this.legacy = LEGACY.some((l) => Object.keys(files).some((p) => p === l || p.startsWith(`${l}/`)))
    await this.remember(files)
    if (projectNeedsMigration(files)) await this.save(loaded.project, { force: true })
    return loaded
  }

  /**
   * Project files whose contents on disk differ from what was last read or
   * written. Cheap when nothing changed: one stat per file.
   */
  async changedOnDisk(): Promise<string[]> {
    const changed: string[] = []
    for (const [path, text] of this.snapshot) {
      const st = await this.store.stat(path)
      const stamp = stampOf(st)
      if (stamp === this.stamps.get(path)) continue
      const now = st ? await this.store.readText(path) : null
      if (now === text) {
        // Touched (a checkout of the same contents): nothing to do.
        this.stamps.set(path, stamp)
        continue
      }
      changed.push(path)
    }
    if (!this.snapshot.has(MANIFEST_FILE) && (await this.store.stat(MANIFEST_FILE))) changed.push(MANIFEST_FILE)
    return changed
  }

  /**
   * Writes the project. Only files whose contents changed are written, each
   * atomically. Unless `force`, refuses (ExternalChangeError) when another
   * program changed the files since they were read, so it never overwrites
   * someone else's edit unasked.
   */
  async save(project: Project, opts: { force?: boolean } = {}): Promise<{ written: string[] }> {
    if (!opts.force) {
      const ext = await this.changedOnDisk()
      if (ext.length) throw new ExternalChangeError(ext)
    }
    const files = projectToFiles(project)
    const written: string[] = []
    // Devices first and project.toml last: a reader that sees the new
    // project.toml also finds the manifests it names.
    const order = Object.keys(files).sort((a, b) => rank(a) - rank(b) || a.localeCompare(b))
    for (const path of order) {
      if (!opts.force && this.snapshot.get(path) === files[path]) continue
      await this.store.writeText(path, files[path])
      written.push(path)
    }
    for (const e of await this.store.list('devices')) {
      const rel = `devices/${e.name}`
      if (e.kind === 'file' && e.name.endsWith('.toml') && !(rel in files)) await this.store.remove(rel)
    }
    if (this.legacy) {
      for (const l of LEGACY) await this.store.remove(l)
      this.legacy = false
    }
    await this.remember(files)
    return { written }
  }

  /** Writes a new project into an empty folder, with a README and a .gitignore. */
  async create(project: Project): Promise<void> {
    if ((await this.store.readText(README_FILE)) === null) await this.store.writeText(README_FILE, readmeText(project.name))
    if ((await this.store.readText(GITIGNORE_FILE)) === null) await this.store.writeText(GITIGNORE_FILE, GITIGNORE_TEXT)
    await this.save(project, { force: true })
  }
}

function rank(path: string): number {
  return path.startsWith('devices/') ? 0 : path === MANIFEST_FILE ? 2 : 1
}
