// SPDX-License-Identifier: MPL-2.0
//
// Notices when a project's files may have changed on disk. With
// FileSystemObserver (feature-detected) the browser says so; otherwise the
// folder is polled every couple of seconds while the tab is visible, and once
// when it becomes visible again. `check` decides whether anything changed
// (ProjectFolder.changedOnDisk); this only says when to look.

import { fileSystemObserver, type FsObserverCtor } from './handles'

export interface WatchOptions {
  check: () => void | Promise<void>
  /** The folder, for FileSystemObserver. Without it, polling only. */
  handle?: FileSystemDirectoryHandle
  intervalMs?: number
  /** For tests: the document (visibility) and the observer constructor. */
  doc?: Pick<Document, 'visibilityState' | 'addEventListener' | 'removeEventListener'>
  Observer?: FsObserverCtor | null
}

export interface Watcher {
  /** 'observer' or 'poll': how changes are noticed. */
  readonly how: () => 'observer' | 'poll'
  stop(): void
}

export function watchFolder(opts: WatchOptions): Watcher {
  const interval = opts.intervalMs ?? 2000
  const doc = opts.doc ?? (typeof document === 'undefined' ? undefined : document)
  const Observer = opts.Observer === undefined ? fileSystemObserver() : opts.Observer ?? undefined
  let stopped = false
  let running = false
  let again = false
  let how: 'observer' | 'poll' = 'poll'
  let timer: ReturnType<typeof setInterval> | undefined
  let debounce: ReturnType<typeof setTimeout> | undefined
  let observer: { disconnect(): void } | undefined

  const visible = () => !doc || doc.visibilityState === 'visible'

  const run = async () => {
    if (stopped) return
    if (running) {
      again = true
      return
    }
    running = true
    try {
      await opts.check()
    } catch {
      // check reports its own errors
    } finally {
      running = false
      if (again && !stopped) {
        again = false
        void run()
      }
    }
  }

  const poll = () => {
    how = 'poll'
    timer = setInterval(() => {
      if (visible()) void run()
    }, interval)
  }

  const onVisible = () => {
    if (visible()) void run()
  }
  doc?.addEventListener('visibilitychange', onVisible)

  if (Observer && opts.handle) {
    try {
      const obs = new Observer(() => {
        clearTimeout(debounce)
        debounce = setTimeout(() => void run(), 150)
      })
      observer = obs
      how = 'observer'
      obs.observe(opts.handle, { recursive: true }).catch(() => {
        obs.disconnect()
        observer = undefined
        if (!stopped) poll()
      })
    } catch {
      poll()
    }
  } else {
    poll()
  }

  return {
    how: () => how,
    stop() {
      stopped = true
      clearInterval(timer)
      clearTimeout(debounce)
      observer?.disconnect()
      doc?.removeEventListener('visibilitychange', onVisible)
    },
  }
}
