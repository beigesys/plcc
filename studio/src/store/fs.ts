// SPDX-License-Identifier: MPL-2.0
//
// A tiny file-system interface over a FileSystemDirectoryHandle: a folder on
// disk the user picked (File System Access API) or the Origin Private File
// System. Both use the same handle interface, so DirectoryFileStore serves
// both. MemoryFileStore is for tests and for browsers without OPFS.

export interface FileEntry {
  name: string
  kind: 'file' | 'dir'
}

export interface FileStat {
  lastModified: number
  size: number
}

export interface FileStore {
  /** File contents, or null when the file does not exist. */
  readText(path: string): Promise<string | null>
  /** Writes a file, creating parent directories. */
  writeText(path: string, text: string): Promise<void>
  /** Entries directly under `dir` ('' is the root). Empty when `dir` is missing. */
  list(dir: string): Promise<FileEntry[]>
  /** Removes a file or directory recursively; no error when missing. */
  remove(path: string): Promise<void>
  /** Modification time and size, or null when the file does not exist. */
  stat(path: string): Promise<FileStat | null>
}

export function splitPath(path: string): string[] {
  return path.split('/').filter((p) => p !== '' && p !== '.')
}

export class MemoryFileStore implements FileStore {
  private files = new Map<string, string>()
  private times = new Map<string, number>()
  private clock = 0

  async readText(path: string): Promise<string | null> {
    return this.files.get(splitPath(path).join('/')) ?? null
  }

  async writeText(path: string, text: string): Promise<void> {
    const key = splitPath(path).join('/')
    if (!key) throw new Error('empty path')
    this.files.set(key, text)
    this.times.set(key, ++this.clock)
  }

  async list(dir: string): Promise<FileEntry[]> {
    const prefix = splitPath(dir).join('/')
    const pre = prefix ? `${prefix}/` : ''
    const seen = new Map<string, FileEntry['kind']>()
    for (const key of this.files.keys()) {
      if (!key.startsWith(pre)) continue
      const rest = key.slice(pre.length).split('/')
      const name = rest[0]
      const kind = rest.length > 1 ? 'dir' : 'file'
      if (!seen.has(name) || kind === 'dir') seen.set(name, kind)
    }
    return [...seen].map(([name, kind]) => ({ name, kind })).sort((a, b) => a.name.localeCompare(b.name))
  }

  async remove(path: string): Promise<void> {
    const key = splitPath(path).join('/')
    for (const k of [...this.files.keys()]) {
      if (k === key || k.startsWith(`${key}/`) || key === '') {
        this.files.delete(k)
        this.times.delete(k)
      }
    }
  }

  async stat(path: string): Promise<FileStat | null> {
    const key = splitPath(path).join('/')
    const text = this.files.get(key)
    return text === undefined ? null : { lastModified: this.times.get(key) ?? 0, size: text.length }
  }
}

/** A FileStore over a directory handle: a picked folder on disk, or OPFS. */
export class DirectoryFileStore implements FileStore {
  readonly root: FileSystemDirectoryHandle

  constructor(root: FileSystemDirectoryHandle) {
    this.root = root
  }

  private async dir(parts: string[], create: boolean): Promise<FileSystemDirectoryHandle | null> {
    let d = this.root
    for (const p of parts) {
      try {
        d = await d.getDirectoryHandle(p, { create })
      } catch (e) {
        if (!create && isNotFound(e)) return null
        throw e
      }
    }
    return d
  }

  private async file(path: string): Promise<File | null> {
    const parts = splitPath(path)
    const name = parts.pop()
    if (!name) return null
    const d = await this.dir(parts, false)
    if (!d) return null
    try {
      return await (await d.getFileHandle(name)).getFile()
    } catch (e) {
      if (isNotFound(e)) return null
      throw e
    }
  }

  async readText(path: string): Promise<string | null> {
    const f = await this.file(path)
    return f ? f.text() : null
  }

  async stat(path: string): Promise<FileStat | null> {
    const f = await this.file(path)
    return f ? { lastModified: f.lastModified, size: f.size } : null
  }

  async writeText(path: string, text: string): Promise<void> {
    const parts = splitPath(path)
    const name = parts.pop()
    if (!name) throw new Error('empty path')
    const d = await this.dir(parts, true)
    if (!d) throw new Error(`cannot create directory for ${path}`)
    const fh = await d.getFileHandle(name, { create: true })
    const writable = (fh as FileSystemFileHandle & { createWritable?: () => Promise<FileSystemWritableFileStream> })
      .createWritable
    if (typeof writable !== 'function') {
      throw new Error(
        'This browser cannot write OPFS files from the page (no createWritable); projects are kept in memory only',
      )
    }
    // createWritable writes to a swap file and close() moves it into place, so
    // another program never reads half a file.
    const w = await fh.createWritable()
    try {
      await w.write(text)
    } catch (e) {
      await w.abort().catch(() => {})
      throw e
    }
    await w.close()
  }

  async list(dir: string): Promise<FileEntry[]> {
    const d = await this.dir(splitPath(dir), false)
    if (!d) return []
    const out: FileEntry[] = []
    const iterable = d as FileSystemDirectoryHandle & { values(): AsyncIterable<FileSystemHandle> }
    for await (const h of iterable.values()) {
      out.push({ name: h.name, kind: h.kind === 'directory' ? 'dir' : 'file' })
    }
    return out.sort((a, b) => a.name.localeCompare(b.name))
  }

  async remove(path: string): Promise<void> {
    const parts = splitPath(path)
    const name = parts.pop()
    if (!name) return
    const d = await this.dir(parts, false)
    if (!d) return
    try {
      await d.removeEntry(name, { recursive: true })
    } catch (e) {
      if (!isNotFound(e)) throw e
    }
  }
}

/** The browser's private storage for this site (OPFS). */
export class OpfsFileStore extends DirectoryFileStore {
  static async open(): Promise<OpfsFileStore> {
    if (typeof navigator === 'undefined' || !navigator.storage?.getDirectory) {
      throw new Error('Origin Private File System is not available in this browser')
    }
    return new OpfsFileStore(await navigator.storage.getDirectory())
  }
}

/** A FileStore seen from one of its directories. */
export class SubFileStore implements FileStore {
  readonly base: FileStore
  readonly prefix: string

  constructor(base: FileStore, prefix: string) {
    this.base = base
    this.prefix = splitPath(prefix).join('/')
  }

  private at(path: string): string {
    const rest = splitPath(path).join('/')
    return rest ? `${this.prefix}/${rest}` : this.prefix
  }

  readText(path: string) {
    return this.base.readText(this.at(path))
  }

  writeText(path: string, text: string) {
    return this.base.writeText(this.at(path), text)
  }

  list(dir: string) {
    return this.base.list(this.at(dir))
  }

  remove(path: string) {
    if (!splitPath(path).length) return Promise.reject(new Error('refusing to remove the root of a SubFileStore'))
    return this.base.remove(this.at(path))
  }

  stat(path: string) {
    return this.base.stat(this.at(path))
  }
}

function errorName(e: unknown): string {
  return typeof e === 'object' && e !== null && 'name' in e ? String((e as { name: unknown }).name) : ''
}

/** The file or folder is gone (or is the other kind). */
export function isNotFound(e: unknown): boolean {
  const n = errorName(e)
  return n === 'NotFoundError' || n === 'TypeMismatchError'
}

/** The page lost (or never had) write access to a picked folder. */
export function isPermissionError(e: unknown): boolean {
  const n = errorName(e)
  return n === 'NotAllowedError' || n === 'SecurityError'
}

/** The user closed a picker. */
export function isAbort(e: unknown): boolean {
  return errorName(e) === 'AbortError'
}

/** OPFS when it works end to end (probe write/read/remove), otherwise memory. */
export async function openDefaultStore(): Promise<{ store: FileStore; persistent: boolean; reason?: string }> {
  try {
    const store = await OpfsFileStore.open()
    // Unique name: two tabs (or a double-mounted effect) may probe at once.
    const probe = `.probe-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`
    await store.writeText(probe, 'ok')
    const back = await store.readText(probe)
    await store.remove(probe)
    if (back !== 'ok') throw new Error('OPFS probe read back the wrong contents')
    return { store, persistent: true }
  } catch (e) {
    return { store: new MemoryFileStore(), persistent: false, reason: e instanceof Error ? e.message : String(e) }
  }
}
