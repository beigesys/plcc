// SPDX-License-Identifier: MPL-2.0
//
// The project session against fake folders: New / Open / Recent, permission
// prompts and denials, folders that disappear, changes made on disk by other
// programs, and conflicts with unsaved edits.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { demoProject } from '@/model'
import { MemoryFileStore, memoryRecentBackend, type RecentBackend } from '@/store'
import { FakeDirectoryHandle } from '@/store/testing/fakeFs'
import { useEditor } from './editor'
import {
  checkDisk, closeProject, createBrowserProject, flushAutosave, grantAccess, initPersistence, keepMine, newFolderProject, openDemo,
  openFolderProject, openRecent, RecentError, reloadFromDisk, resetPersistenceForTests, saveToFolder, useProjects,
} from './persistence'

let picks: (FakeDirectoryHandle | null)[] = []
let store: MemoryFileStore
let recentBackend: RecentBackend

function pickNext(...dirs: (FakeDirectoryHandle | null)[]) {
  picks.push(...dirs)
}

function memoryStorage(): Storage {
  const m = new Map<string, string>()
  return {
    get length() {
      return m.size
    },
    clear: () => m.clear(),
    getItem: (k) => m.get(k) ?? null,
    key: (i) => [...m.keys()][i] ?? null,
    removeItem: (k) => void m.delete(k),
    setItem: (k, v) => void m.set(k, String(v)),
  }
}

async function start() {
  await initPersistence({ store, recent: recentBackend })
}

/** An edit to the open project, as the editor makes one. */
function edit(comment: string) {
  useEditor.getState().commit((p) => ({ ...p, globals: p.globals.map((t, i) => (i === 0 ? { ...t, comment } : t)) }))
}

const firstComment = () => useEditor.getState().project.globals[0]?.comment

beforeEach(() => {
  picks = []
  store = new MemoryFileStore()
  recentBackend = memoryRecentBackend()
  vi.stubGlobal('localStorage', memoryStorage())
  vi.stubGlobal('window', {
    addEventListener() {},
    showDirectoryPicker: async () => {
      const next = picks.shift()
      if (next === undefined) throw new Error('test did not expect a picker')
      if (next === null) throw new DOMException('cancelled', 'AbortError')
      return next.handle
    },
  })
})

afterEach(async () => {
  await resetPersistenceForTests()
  vi.unstubAllGlobals()
})

describe('start', () => {
  it('shows the start screen the first time, with folder access', async () => {
    await start()
    expect(useProjects.getState()).toMatchObject({ ready: true, folderAccess: true, recent: [], browser: [] })
    expect(useEditor.getState().projectId).toBeNull()
  })

  it('opens the demo in browser storage, and reopens it next time', async () => {
    await start()
    await openDemo()
    expect(useEditor.getState().projectSource).toEqual({ kind: 'browser', id: 'demo' })
    expect(useEditor.getState().project.name).toBe('Demo Opta')
    await resetPersistenceForTests()
    await start()
    expect(useEditor.getState().projectSource).toEqual({ kind: 'browser', id: 'demo' })
  })
})

describe('New project', () => {
  it('writes a project into an empty folder and opens it', async () => {
    await start()
    const dir = new FakeDirectoryHandle('pump-skid')
    pickNext(dir)
    expect(await newFolderProject()).toBe('opened')
    expect(dir.tree()).toEqual(['.gitignore', 'README.md', 'devices/simulator.toml', 'project.json', 'project.toml'])
    const s = useEditor.getState()
    expect(s.project.name).toBe('Pump skid')
    expect(s.projectSource).toMatchObject({ kind: 'folder', folder: 'pump-skid' })
    expect(useProjects.getState().recent.map((e) => e.folder)).toEqual(['pump-skid'])
  })

  it('does nothing when the picker is cancelled', async () => {
    await start()
    pickNext(null)
    expect(await newFolderProject()).toBe('cancelled')
    expect(useEditor.getState().projectId).toBeNull()
  })

  it('refuses a folder with other files in it', async () => {
    await start()
    const dir = new FakeDirectoryHandle('stuff')
    dir.writeExternal('photo.jpg', 'x')
    pickNext(dir)
    await expect(newFolderProject()).rejects.toThrow(/“stuff” is not empty \(photo.jpg\)/)
    expect(dir.tree()).toEqual(['photo.jpg'])
  })

  it('offers to open a folder that already is a project', async () => {
    await start()
    const dir = new FakeDirectoryHandle('existing')
    pickNext(dir)
    await newFolderProject()
    await closeProject()
    pickNext(dir)
    expect(await newFolderProject()).toBe('offered')
    expect(useProjects.getState().note).toMatchObject({ tone: 'offer', text: expect.stringMatching(/already holds a plcc project/) })
  })
})

describe('Open project', () => {
  it('opens a folder with project.toml', async () => {
    await start()
    const dir = new FakeDirectoryHandle('p')
    pickNext(dir)
    await newFolderProject(demoProject())
    await closeProject()
    pickNext(dir)
    expect(await openFolderProject()).toBe('opened')
    expect(useEditor.getState().project.name).toBe('Demo Opta')
  })

  it('says clearly when the folder is not a project', async () => {
    await start()
    const dir = new FakeDirectoryHandle('random')
    dir.writeExternal('a.txt', 'x')
    pickNext(dir)
    await expect(openFolderProject()).rejects.toThrow(/“random” is not a plcc project: it has no project.toml/)
  })
})

describe('autosave to the folder', () => {
  it('writes edits to project.json, and leaves the README alone', async () => {
    await start()
    const dir = new FakeDirectoryHandle('p')
    pickNext(dir)
    await newFolderProject(demoProject())
    dir.writeExternal('README.md', 'my own readme\n')
    edit('saved to disk')
    expect(useEditor.getState().saveStatus).toBe('pending')
    await flushAutosave()
    expect(useEditor.getState().saveStatus).toBe('saved')
    expect(dir.readExternal('project.json')).toContain('saved to disk')
    expect(dir.readExternal('README.md')).toBe('my own readme\n')
  })
})

describe('Recent', () => {
  async function recentFolder() {
    await start()
    const dir = new FakeDirectoryHandle('line-3')
    pickNext(dir)
    await newFolderProject(demoProject())
    await closeProject()
    const entry = useProjects.getState().recent[0]
    return { dir, entry }
  }

  it('asks for permission from the click and opens', async () => {
    const { dir, entry } = await recentFolder()
    dir.access.permission = 'prompt'
    dir.access.answer = 'granted'
    await openRecent(entry)
    expect(dir.access.requests).toBe(1)
    expect(useEditor.getState().projectSource).toMatchObject({ kind: 'folder', folder: 'line-3' })
  })

  it('reports a denied permission and leaves the entry', async () => {
    const { dir, entry } = await recentFolder()
    dir.access.permission = 'prompt'
    dir.access.answer = 'denied'
    const e = await openRecent(entry).catch((x: unknown) => x)
    expect(e).toBeInstanceOf(RecentError)
    expect((e as Error).message).toMatch(/Access to “line-3” was not allowed/)
    expect(useEditor.getState().projectId).toBeNull()
    expect(useProjects.getState().recent).toHaveLength(1)
  })

  it('reports a folder that was moved or deleted', async () => {
    const { dir, entry } = await recentFolder()
    dir.deleteExternal()
    await expect(openRecent(entry)).rejects.toThrow(/“line-3” was moved or deleted/)
  })

  it('reopens the last folder at startup only when access is still granted', async () => {
    const { dir } = await recentFolder()
    // Reopen it so it is the last project.
    await openRecent(useProjects.getState().recent[0])
    await resetPersistenceForTests()
    await start()
    expect(useEditor.getState().projectSource).toMatchObject({ kind: 'folder' })

    await resetPersistenceForTests()
    dir.access.permission = 'prompt'
    await start()
    expect(useEditor.getState().projectId).toBeNull()
    expect(useProjects.getState().resumeKey).toBe(useProjects.getState().recent[0].key)
    expect(dir.access.requests).toBe(0)
  })
})

describe('changes on disk', () => {
  async function openFolder() {
    await start()
    const dir = new FakeDirectoryHandle('p')
    pickNext(dir)
    await newFolderProject(demoProject())
    return dir
  }

  it('reloads when there are no unsaved edits', async () => {
    const dir = await openFolder()
    useEditor.setState({ notice: null })
    dir.writeExternal('project.toml', dir.readExternal('project.toml')!.replace('Demo Opta', 'Checked out'))
    await checkDisk()
    expect(useEditor.getState().project.name).toBe('Checked out')
    expect(useEditor.getState().notice?.text).toMatch(/Reloaded from disk: project.toml changed/)
    expect(useEditor.getState().disk).toBeNull()
    // Reloading is not an edit: nothing is written back.
    expect(useEditor.getState().saveStatus).toBe('saved')
    // Recent shows the name the project has now.
    await closeProject()
    expect(useProjects.getState().recent[0].name).toBe('Checked out')
  })

  it('ignores a touch (same contents)', async () => {
    const dir = await openFolder()
    useEditor.setState({ notice: null })
    dir.touchExternal('project.json')
    await checkDisk()
    expect(useEditor.getState().notice).toBeNull()
  })

  it('raises a conflict instead of overwriting, then keeps mine', async () => {
    const dir = await openFolder()
    edit('mine')
    dir.writeExternal('project.toml', dir.readExternal('project.toml')!.replace('Demo Opta', 'Theirs'))
    await flushAutosave()
    expect(useEditor.getState().disk).toEqual({ kind: 'conflict', paths: ['project.toml'] })
    expect(useEditor.getState().saveStatus).toBe('blocked')
    expect(dir.readExternal('project.toml')).toContain('Theirs')
    // More edits do not overwrite either.
    edit('mine 2')
    await flushAutosave()
    expect(dir.readExternal('project.toml')).toContain('Theirs')
    await keepMine()
    expect(useEditor.getState().disk).toBeNull()
    expect(dir.readExternal('project.toml')).toContain('Demo Opta')
    expect(dir.readExternal('project.json')).toContain('mine 2')
  })

  it('raises a conflict from the watcher when edits are unsaved, then reloads from disk', async () => {
    const dir = await openFolder()
    edit('mine')
    // The save failed for some other reason: the edit is still unsaved.
    useEditor.getState().setSaveStatus('error', 'disk full')
    dir.writeExternal('project.toml', dir.readExternal('project.toml')!.replace('Demo Opta', 'Theirs'))
    await checkDisk()
    expect(useEditor.getState().disk).toMatchObject({ kind: 'conflict' })
    await reloadFromDisk()
    expect(useEditor.getState().project.name).toBe('Theirs')
    expect(firstComment()).not.toBe('mine')
    expect(useEditor.getState().disk).toBeNull()
  })

  it('shows an unreadable file as a conflict and keeps the editor’s project', async () => {
    const dir = await openFolder()
    dir.writeExternal('project.json', '<<<<<<< HEAD\n')
    await checkDisk()
    expect(useEditor.getState().disk).toMatchObject({ kind: 'conflict', paths: ['project.json'], error: expect.stringMatching(/invalid JSON/) })
    expect(useEditor.getState().project.name).toBe('Demo Opta')
  })

  it('notices that the folder is gone', async () => {
    const dir = await openFolder()
    dir.deleteExternal()
    await checkDisk()
    expect(useEditor.getState().disk).toEqual({ kind: 'gone', folder: 'p' })
  })
})

describe('permission lost mid-session', () => {
  it('shows a banner, keeps the edits, and saves them once access is granted', async () => {
    await start()
    const dir = new FakeDirectoryHandle('p')
    pickNext(dir)
    await newFolderProject(demoProject())
    dir.access.permission = 'prompt'
    edit('while locked')
    await flushAutosave()
    expect(useEditor.getState().disk).toEqual({ kind: 'permission', folder: 'p' })
    expect(useEditor.getState().saveStatus).toBe('blocked')
    expect(dir.readExternal('project.json')).not.toContain('while locked')

    dir.access.answer = 'denied'
    expect(await grantAccess()).toBe(false)
    expect(useEditor.getState().disk).toMatchObject({ kind: 'permission' })

    dir.access.permission = 'prompt'
    dir.access.answer = 'granted'
    expect(await grantAccess()).toBe(true)
    expect(useEditor.getState().disk).toBeNull()
    expect(dir.readExternal('project.json')).toContain('while locked')
    expect(useEditor.getState().saveStatus).toBe('saved')
  })
})

describe('browser storage', () => {
  it('Save to folder… copies a browser project to disk and switches to it', async () => {
    await start()
    await createBrowserProject('Quick test', demoProject())
    edit('from the browser')
    const dir = new FakeDirectoryHandle('quick-test')
    pickNext(dir)
    expect(await saveToFolder()).toBe('opened')
    expect(useEditor.getState().projectSource).toMatchObject({ kind: 'folder', folder: 'quick-test' })
    expect(dir.readExternal('project.toml')).toContain('name = "Quick test"')
    expect(dir.readExternal('project.json')).toContain('from the browser')
    // The browser copy stays.
    expect(useProjects.getState().browser.map((p) => p.name)).toEqual(['Quick test'])
  })

  it('works without showDirectoryPicker (Firefox, Safari)', async () => {
    vi.stubGlobal('window', { addEventListener() {} })
    await start()
    expect(useProjects.getState().folderAccess).toBe(false)
    await createBrowserProject('Firefox project')
    expect(useEditor.getState().projectSource?.kind).toBe('browser')
    edit('x')
    await flushAutosave()
    expect(useEditor.getState().saveStatus).toBe('saved')
  })
})
