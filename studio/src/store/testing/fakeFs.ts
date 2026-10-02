// SPDX-License-Identifier: MPL-2.0
//
// An in-memory FileSystemDirectoryHandle for tests: the parts DirectoryFileStore,
// the recent list and the session use, plus the browser's permission model
// (granted / prompt / denied, and what requestPermission answers), folders
// that are deleted under the page, and edits "by another program".

type Perm = 'granted' | 'denied' | 'prompt'

interface FileRec {
  text: string
  lastModified: number
}

let clock = 1_700_000_000_000

function err(name: string, message: string): DOMException {
  return new DOMException(message, name)
}

/** Shared by every handle under one root: permission and the request answer. */
class Access {
  permission: Perm = 'granted'
  /** What requestPermission answers (the user's click in the browser prompt). */
  answer: Perm = 'granted'
  requests = 0
}

export class FakeFileHandle {
  readonly kind = 'file'
  readonly name: string
  private readonly parent: FakeDirectoryHandle

  constructor(parent: FakeDirectoryHandle, name: string) {
    this.parent = parent
    this.name = name
  }

  async getFile() {
    this.parent.check('read')
    const rec = this.parent.files.get(this.name)
    if (!rec) throw err('NotFoundError', `${this.name} not found`)
    const { text, lastModified } = rec
    return { name: this.name, lastModified, size: new TextEncoder().encode(text).length, text: async () => text }
  }

  async createWritable() {
    this.parent.check('write')
    let buf = ''
    return {
      write: async (t: string) => {
        buf += t
      },
      close: async () => {
        this.parent.check('write')
        this.parent.files.set(this.name, { text: buf, lastModified: ++clock })
      },
      abort: async () => {},
    }
  }

  async isSameEntry(o: unknown) {
    return o instanceof FakeFileHandle && o.parent === this.parent && o.name === this.name
  }
}

export class FakeDirectoryHandle {
  readonly kind = 'directory'
  readonly name: string
  readonly files = new Map<string, FileRec>()
  readonly dirs = new Map<string, FakeDirectoryHandle>()
  readonly access: Access
  removed = false
  private parent?: FakeDirectoryHandle

  constructor(name: string, parent?: FakeDirectoryHandle) {
    this.name = name
    this.parent = parent
    this.access = parent?.access ?? new Access()
  }

  /** Throws as the browser does: gone → NotFoundError, no permission → NotAllowedError. */
  check(op: 'read' | 'write') {
    if (this.isRemoved()) throw err('NotFoundError', `${this.name} was removed`)
    if (this.access.permission !== 'granted') throw err('NotAllowedError', `no ${op} permission`)
  }

  private isRemoved(): boolean {
    return this.removed || (this.parent?.isRemoved() ?? false)
  }

  async queryPermission() {
    return this.access.permission
  }

  async requestPermission() {
    this.access.requests++
    if (this.access.permission === 'prompt') this.access.permission = this.access.answer
    return this.access.permission
  }

  async getDirectoryHandle(name: string, opts: { create?: boolean } = {}) {
    this.check(opts.create ? 'write' : 'read')
    let d = this.dirs.get(name)
    if (!d) {
      if (this.files.has(name)) throw err('TypeMismatchError', `${name} is a file`)
      if (!opts.create) throw err('NotFoundError', `${name} not found`)
      d = new FakeDirectoryHandle(name, this)
      this.dirs.set(name, d)
    }
    return d
  }

  async getFileHandle(name: string, opts: { create?: boolean } = {}) {
    this.check(opts.create ? 'write' : 'read')
    if (this.dirs.has(name)) throw err('TypeMismatchError', `${name} is a directory`)
    if (!this.files.has(name)) {
      if (!opts.create) throw err('NotFoundError', `${name} not found`)
      this.files.set(name, { text: '', lastModified: ++clock })
    }
    return new FakeFileHandle(this, name)
  }

  async removeEntry(name: string) {
    this.check('write')
    if (this.files.delete(name)) return
    const d = this.dirs.get(name)
    if (!d) throw err('NotFoundError', `${name} not found`)
    d.removed = true
    this.dirs.delete(name)
  }

  async *values(): AsyncGenerator<FakeDirectoryHandle | FakeFileHandle> {
    this.check('read')
    for (const name of [...this.dirs.keys()].sort()) yield this.dirs.get(name)!
    for (const name of [...this.files.keys()].sort()) yield new FakeFileHandle(this, name)
  }

  async isSameEntry(o: unknown) {
    return o === this
  }

  // ------------------------------------------------ "another program"

  private walk(path: string, create: boolean): { dir: FakeDirectoryHandle; name: string } {
    const parts = path.split('/').filter(Boolean)
    if (parts.length === 1) return { dir: this, name: parts[0] }
    let next = this.dirs.get(parts[0])
    if (!next) {
      if (!create) throw new Error(`no ${path}`)
      next = new FakeDirectoryHandle(parts[0], this)
      this.dirs.set(parts[0], next)
    }
    return next.walk(parts.slice(1).join('/'), create)
  }

  /** Writes a file as another program would (git, an editor): no permission needed. */
  writeExternal(path: string, text: string) {
    const { dir, name } = this.walk(path, true)
    dir.files.set(name, { text, lastModified: ++clock })
  }

  /** Changes a file's time without changing it (git checkout of the same contents). */
  touchExternal(path: string) {
    const { dir, name } = this.walk(path, false)
    const rec = dir.files.get(name)
    if (rec) rec.lastModified = ++clock
  }

  readExternal(path: string): string | undefined {
    try {
      const { dir, name } = this.walk(path, false)
      return dir.files.get(name)?.text
    } catch {
      return undefined
    }
  }

  /** Every file under this folder, by path. */
  tree(prefix = ''): string[] {
    const out: string[] = []
    for (const [n, d] of this.dirs) out.push(...d.tree(`${prefix}${n}/`))
    for (const n of this.files.keys()) out.push(`${prefix}${n}`)
    return out.sort()
  }

  /** The folder was deleted or moved away. */
  deleteExternal() {
    this.removed = true
  }

  /** Narrow to the DOM type for code under test. */
  get handle(): FileSystemDirectoryHandle {
    return this as unknown as FileSystemDirectoryHandle
  }
}
