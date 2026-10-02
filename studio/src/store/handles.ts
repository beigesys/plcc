// SPDX-License-Identifier: MPL-2.0
//
// The parts of the File System Access API that TypeScript's DOM types leave
// out (they are Chromium-only): the directory picker, permissions on a
// handle, and FileSystemObserver.

export type FolderPermission = 'granted' | 'denied' | 'prompt'

interface PermissionedHandle {
  queryPermission?(d: { mode: 'read' | 'readwrite' }): Promise<FolderPermission>
  requestPermission?(d: { mode: 'read' | 'readwrite' }): Promise<FolderPermission>
}

interface PickerWindow {
  showDirectoryPicker?(opts?: { id?: string; mode?: 'read' | 'readwrite'; startIn?: string }): Promise<FileSystemDirectoryHandle>
}

/** Whether this browser can open folders on disk (Chrome, Edge, Opera; not Firefox or Safari). */
export function folderAccessSupported(): boolean {
  return typeof window !== 'undefined' && typeof (window as PickerWindow).showDirectoryPicker === 'function'
}

/**
 * Asks the user for a folder, with read/write access. Null when they cancel.
 * The picker remembers the last folder chosen for the same `id`.
 */
export async function pickFolder(id = 'plcc-project'): Promise<FileSystemDirectoryHandle | null> {
  const w = window as PickerWindow
  if (!w.showDirectoryPicker) throw new Error('This browser cannot open folders (no File System Access API)')
  try {
    return await w.showDirectoryPicker({ id, mode: 'readwrite', startIn: 'documents' })
  } catch (e) {
    if (typeof e === 'object' && e !== null && (e as { name?: string }).name === 'AbortError') return null
    throw e
  }
}

/**
 * Read/write permission on a folder. With `request` (only from a user
 * gesture, such as a click) the browser asks the user when it would
 * otherwise say 'prompt'. Handles without permissions (OPFS) are granted.
 */
export async function folderPermission(handle: FileSystemDirectoryHandle, request: boolean): Promise<FolderPermission> {
  const h = handle as unknown as PermissionedHandle
  if (typeof h.queryPermission !== 'function') return 'granted'
  const now = await h.queryPermission({ mode: 'readwrite' })
  if (now === 'granted' || !request || typeof h.requestPermission !== 'function') return now
  return h.requestPermission({ mode: 'readwrite' })
}

/** False when the folder was moved or deleted since the handle was made. */
export async function folderExists(handle: FileSystemDirectoryHandle): Promise<boolean> {
  try {
    const it = (handle as unknown as { values(): AsyncIterator<FileSystemHandle> }).values()
    await it.next()
    return true
  } catch (e) {
    const name = typeof e === 'object' && e !== null ? (e as { name?: string }).name : ''
    if (name === 'NotFoundError') return false
    throw e
  }
}

/** Two handles to the same folder. */
export async function sameFolder(a: FileSystemDirectoryHandle, b: FileSystemDirectoryHandle): Promise<boolean> {
  try {
    return await a.isSameEntry(b)
  } catch {
    return false
  }
}

export interface FsObserver {
  observe(handle: FileSystemHandle, opts?: { recursive?: boolean }): Promise<void>
  disconnect(): void
}

export type FsObserverCtor = new (callback: (records: unknown[], observer: FsObserver) => void) => FsObserver

/** FileSystemObserver (Chrome 129+ behind a flag or origin trial, 133+ for OPFS), when present. */
export function fileSystemObserver(): FsObserverCtor | undefined {
  const ctor = (globalThis as { FileSystemObserver?: FsObserverCtor }).FileSystemObserver
  return typeof ctor === 'function' ? ctor : undefined
}
