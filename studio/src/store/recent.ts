// SPDX-License-Identifier: MPL-2.0
//
// Recently opened project folders. A FileSystemDirectoryHandle can be stored
// in IndexedDB (it is structured-cloneable), and that is the only way to
// reopen a folder later without picking it again; the browser still asks for
// permission again in a later session (folderPermission).

import { sameFolder } from './handles'

export interface RecentEntry {
  key: string
  /** The folder's name, as the picker showed it. */
  folder: string
  /** The project's name when it was last opened. */
  name: string
  handle: FileSystemDirectoryHandle
  openedAt: number
}

export interface RecentBackend {
  all(): Promise<RecentEntry[]>
  put(e: RecentEntry): Promise<void>
  delete(key: string): Promise<void>
}

export function memoryRecentBackend(): RecentBackend {
  const m = new Map<string, RecentEntry>()
  return {
    all: async () => [...m.values()],
    put: async (e) => void m.set(e.key, e),
    delete: async (k) => void m.delete(k),
  }
}

function req<T>(r: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    r.onsuccess = () => resolve(r.result)
    r.onerror = () => reject(r.error ?? new Error('IndexedDB request failed'))
  })
}

/** IndexedDB `plcc-studio`, object store `recent`. */
export function idbRecentBackend(dbName = 'plcc-studio', storeName = 'recent'): RecentBackend {
  let db: Promise<IDBDatabase> | null = null
  const open = () => {
    db ??= new Promise((resolve, reject) => {
      const r = indexedDB.open(dbName, 1)
      r.onupgradeneeded = () => {
        if (!r.result.objectStoreNames.contains(storeName)) r.result.createObjectStore(storeName, { keyPath: 'key' })
      }
      r.onsuccess = () => resolve(r.result)
      r.onerror = () => reject(r.error ?? new Error('cannot open IndexedDB'))
      r.onblocked = () => reject(new Error('IndexedDB is blocked by another tab'))
    })
    return db
  }
  const tx = async (mode: IDBTransactionMode) => (await open()).transaction(storeName, mode).objectStore(storeName)
  return {
    all: async () => req((await tx('readonly')).getAll()) as Promise<RecentEntry[]>,
    put: async (e) => void (await req((await tx('readwrite')).put(e))),
    delete: async (k) => void (await req((await tx('readwrite')).delete(k))),
  }
}

/** IndexedDB when the browser has it, otherwise memory. */
export function defaultRecentBackend(): RecentBackend {
  return typeof indexedDB === 'undefined' ? memoryRecentBackend() : idbRecentBackend()
}

function newKey(): string {
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`
}

export class RecentProjects {
  readonly backend: RecentBackend
  readonly limit: number

  constructor(backend: RecentBackend, limit = 12) {
    this.backend = backend
    this.limit = limit
  }

  /** Most recent first. */
  async list(): Promise<RecentEntry[]> {
    return (await this.backend.all()).sort((a, b) => b.openedAt - a.openedAt)
  }

  async get(key: string): Promise<RecentEntry | undefined> {
    return (await this.backend.all()).find((e) => e.key === key)
  }

  /** Records that a folder was opened: moves it to the top, keeps its key. */
  async touch(handle: FileSystemDirectoryHandle, name: string, now = Date.now()): Promise<RecentEntry> {
    const all = await this.list()
    let existing: RecentEntry | undefined
    for (const e of all) {
      if (await sameFolder(e.handle, handle)) {
        existing = e
        break
      }
    }
    const entry: RecentEntry = { key: existing?.key ?? newKey(), folder: handle.name, name, handle, openedAt: now }
    await this.backend.put(entry)
    const rest = all.filter((e) => e.key !== entry.key)
    for (const old of rest.slice(this.limit - 1)) await this.backend.delete(old.key)
    return entry
  }

  async remove(key: string): Promise<void> {
    await this.backend.delete(key)
  }
}
