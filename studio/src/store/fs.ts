// SPDX-License-Identifier: MPL-2.0
//
// A tiny file-system interface over the Origin Private File System, with an
// in-memory implementation for tests and for browsers without OPFS.

export interface FileEntry {
  name: string
  kind: 'file' | 'dir'
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
}

export function splitPath(path: string): string[] {
  return path.split('/').filter((p) => p !== '' && p !== '.')
}

export class MemoryFileStore implements FileStore {
  private files = new Map<string, string>()

  async readText(path: string): Promise<string | null> {
    return this.files.get(splitPath(path).join('/')) ?? null
  }

  async writeText(path: string, text: string): Promise<void> {
    const key = splitPath(path).join('/')
    if (!key) throw new Error('empty path')
    this.files.set(key, text)
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
      if (k === key || k.startsWith(`${key}/`) || key === '') this.files.delete(k)
    }
  }
}

export class OpfsFileStore implements FileStore {
  private readonly root: FileSystemDirectoryHandle

  constructor(root: FileSystemDirectoryHandle) {
    this.root = root
  }

  static async open(): Promise<OpfsFileStore> {
    if (typeof navigator === 'undefined' || !navigator.storage?.getDirectory) {
      throw new Error('Origin Private File System is not available in this browser')
    }
    return new OpfsFileStore(await navigator.storage.getDirectory())
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

  async readText(path: string): Promise<string | null> {
    const parts = splitPath(path)
    const name = parts.pop()
    if (!name) return null
    const d = await this.dir(parts, false)
    if (!d) return null
    try {
      const fh = await d.getFileHandle(name)
      return await (await fh.getFile()).text()
    } catch (e) {
      if (isNotFound(e)) return null
      throw e
    }
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
    const w = await fh.createWritable()
    await w.write(text)
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

function isNotFound(e: unknown): boolean {
  return e instanceof DOMException ? e.name === 'NotFoundError' || e.name === 'TypeMismatchError' : false
}

/** OPFS when it works end to end (probe write/read/remove), otherwise memory. */
export async function openDefaultStore(): Promise<{ store: FileStore; persistent: boolean; reason?: string }> {
  try {
    const store = await OpfsFileStore.open()
    const probe = '.probe'
    await store.writeText(probe, 'ok')
    const back = await store.readText(probe)
    await store.remove(probe)
    if (back !== 'ok') throw new Error('OPFS probe read back the wrong contents')
    return { store, persistent: true }
  } catch (e) {
    return { store: new MemoryFileStore(), persistent: false, reason: e instanceof Error ? e.message : String(e) }
  }
}
