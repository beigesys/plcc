// SPDX-License-Identifier: MPL-2.0
import { afterEach, describe, expect, it, vi } from 'vitest'
import { demoProject, emptyProject } from '@/model'
import {
  DirectoryFileStore, ExternalChangeError, GITIGNORE_TEXT, inspectFolder, memoryRecentBackend, ProjectFolder, projectToFiles,
  RecentProjects, watchFolder, type FsObserver, type FsObserverCtor,
} from '@/store'
import { FakeDirectoryHandle } from './testing/fakeFs'

function folder(name = 'pump-skid') {
  const dir = new FakeDirectoryHandle(name)
  return { dir, store: new DirectoryFileStore(dir.handle) }
}

describe('DirectoryFileStore over a folder handle', () => {
  it('reads, writes, lists, stats and removes', async () => {
    const { dir, store } = folder()
    await store.writeText('a/b.txt', 'hi')
    expect(await store.readText('a/b.txt')).toBe('hi')
    expect(await store.readText('a/missing.txt')).toBeNull()
    expect(await store.list('')).toEqual([{ name: 'a', kind: 'dir' }])
    const st = await store.stat('a/b.txt')
    expect(st?.size).toBe(2)
    dir.touchExternal('a/b.txt')
    expect((await store.stat('a/b.txt'))!.lastModified).toBeGreaterThan(st!.lastModified)
    await store.remove('a')
    expect(await store.list('')).toEqual([])
  })

  it('reports lost permission as NotAllowedError', async () => {
    const { dir, store } = folder()
    dir.access.permission = 'prompt'
    await expect(store.writeText('x.txt', '1')).rejects.toMatchObject({ name: 'NotAllowedError' })
  })
})

describe('inspectFolder', () => {
  it('tells empty folders, projects and other folders apart', async () => {
    const { dir, store } = folder()
    dir.writeExternal('.git/HEAD', 'ref: refs/heads/main\n')
    dir.writeExternal('.DS_Store', '')
    expect(await inspectFolder(store)).toEqual({ kind: 'empty' })
    dir.writeExternal('notes.txt', 'x')
    expect(await inspectFolder(store)).toEqual({ kind: 'other', entries: ['notes.txt'] })
    dir.writeExternal('project.toml', 'format = 2\nname = "Pump Skid"\n')
    expect(await inspectFolder(store)).toEqual({ kind: 'project', name: 'Pump Skid' })
  })
})

describe('ProjectFolder', () => {
  it('creates a new project with a README and a .gitignore, all ending in a newline', async () => {
    const { dir, store } = folder()
    await new ProjectFolder(store).create(emptyProject('Pump Skid'))
    expect(dir.tree()).toEqual(['.gitignore', 'README.md', 'devices/simulator.toml', 'project.json', 'project.toml'])
    expect(dir.readExternal('README.md')).toMatch(/^# Pump Skid\n/)
    expect(dir.readExternal('.gitignore')).toBe(GITIGNORE_TEXT)
    for (const f of dir.tree()) expect(dir.readExternal(f)?.endsWith('\n'), f).toBe(true)
    // No volatile fields: the same project gives the same bytes.
    expect(projectToFiles(emptyProject('Pump Skid'))['project.toml']).toBe(dir.readExternal('project.toml'))
  })

  it('writes only the files that changed', async () => {
    const { store } = folder()
    const pf = new ProjectFolder(store)
    const p = demoProject()
    await pf.create(p)
    expect((await pf.save(p)).written).toEqual([])
    const renamed = { ...p, globals: p.globals.map((t, i) => (i === 0 ? { ...t, comment: 'changed' } : t)) }
    expect((await pf.save(renamed)).written).toEqual(['project.json'])
  })

  it('round-trips through load', async () => {
    const { store } = folder()
    const p = demoProject()
    await new ProjectFolder(store).create(p)
    expect((await new ProjectFolder(store).load()).project).toEqual(p)
  })

  it('reads only its own files from a folder that holds other things', async () => {
    const { dir, store } = folder()
    await new ProjectFolder(store).create(demoProject())
    dir.writeExternal('.git/objects/ab/cdef', 'binary')
    dir.writeExternal('docs/notes.md', 'x')
    const pf = new ProjectFolder(store)
    await pf.load()
    await pf.save(emptyProject('Other'))
    expect(dir.readExternal('docs/notes.md')).toBe('x')
    expect(dir.readExternal('.git/objects/ab/cdef')).toBe('binary')
  })

  it('notices external changes, ignores a touch, and refuses to overwrite them', async () => {
    const { dir, store } = folder()
    const pf = new ProjectFolder(store)
    const p = demoProject()
    await pf.create(p)
    expect(await pf.changedOnDisk()).toEqual([])
    dir.touchExternal('project.json')
    expect(await pf.changedOnDisk()).toEqual([])
    dir.writeExternal('project.toml', dir.readExternal('project.toml')!.replace('Demo Opta', 'From git'))
    expect(await pf.changedOnDisk()).toEqual(['project.toml'])
    const mine = { ...p, name: 'Mine' }
    await expect(pf.save(mine)).rejects.toBeInstanceOf(ExternalChangeError)
    expect(dir.readExternal('project.toml')).toContain('From git')
    await pf.save(mine, { force: true })
    expect(dir.readExternal('project.toml')).toContain('Mine')
    expect(await pf.changedOnDisk()).toEqual([])
  })

  it('reports a deleted project.toml as a change', async () => {
    const { dir, store } = folder()
    const pf = new ProjectFolder(store)
    await pf.create(demoProject())
    dir.files.delete('project.toml')
    expect(await pf.changedOnDisk()).toEqual(['project.toml'])
  })

  it('migrates a first-studio project and removes only its old files', async () => {
    const { dir, store } = folder()
    dir.writeExternal('project.toml', 'name = "Old"\n\n[[devices]]\nname = "Opta"\nprofile = "arduino-opta"\n\n[[programs]]\nname = "MainProgram"\nmain = "MainRoutine"\n\n[[programs.routines]]\nname = "MainRoutine"\nkind = "ladder"\n')
    dir.writeExternal('routines/MainRoutine.ladder.json', '{ "format": "plcc-studio-ladder", "version": 1, "name": "MainRoutine", "rungs": [] }')
    dir.writeExternal('docs/keep.md', 'keep')
    await new ProjectFolder(store).load()
    expect(dir.tree()).toEqual(['devices/arduino-opta.toml', 'docs/keep.md', 'project.json', 'project.toml'])
  })
})

describe('RecentProjects', () => {
  it('keeps the most recent first, one entry per folder, and a limit', async () => {
    const r = new RecentProjects(memoryRecentBackend(), 3)
    const a = new FakeDirectoryHandle('a')
    const b = new FakeDirectoryHandle('b')
    const ea = await r.touch(a.handle, 'A', 1)
    await r.touch(b.handle, 'B', 2)
    const ea2 = await r.touch(a.handle, 'A renamed', 3)
    expect(ea2.key).toBe(ea.key)
    expect((await r.list()).map((e) => [e.folder, e.name])).toEqual([['a', 'A renamed'], ['b', 'B']])
    await r.touch(new FakeDirectoryHandle('c').handle, 'C', 4)
    await r.touch(new FakeDirectoryHandle('d').handle, 'D', 5)
    expect((await r.list()).map((e) => e.folder)).toEqual(['d', 'c', 'a'])
    await r.remove(ea.key)
    expect((await r.list()).map((e) => e.folder)).toEqual(['d', 'c'])
  })
})

describe('watchFolder', () => {
  afterEach(() => {
    vi.useRealTimers()
  })

  function fakeDoc() {
    const listeners = new Set<() => void>()
    return {
      visibilityState: 'visible' as DocumentVisibilityState,
      addEventListener: (_: string, f: () => void) => void listeners.add(f),
      removeEventListener: (_: string, f: () => void) => void listeners.delete(f),
      fire() {
        for (const f of listeners) f()
      },
    }
  }

  it('polls while the tab is visible, and checks when it becomes visible', async () => {
    vi.useFakeTimers()
    const check = vi.fn()
    const doc = fakeDoc()
    const w = watchFolder({ check, intervalMs: 2000, doc: doc as unknown as Document, Observer: null })
    expect(w.how()).toBe('poll')
    await vi.advanceTimersByTimeAsync(4100)
    expect(check).toHaveBeenCalledTimes(2)
    doc.visibilityState = 'hidden'
    await vi.advanceTimersByTimeAsync(6000)
    expect(check).toHaveBeenCalledTimes(2)
    doc.visibilityState = 'visible'
    doc.fire()
    await vi.advanceTimersByTimeAsync(0)
    expect(check).toHaveBeenCalledTimes(3)
    w.stop()
    await vi.advanceTimersByTimeAsync(10000)
    expect(check).toHaveBeenCalledTimes(3)
  })

  it('uses FileSystemObserver when the browser has it', async () => {
    vi.useFakeTimers()
    const check = vi.fn()
    let fire: () => void = () => {}
    let disconnected = false
    class Obs implements FsObserver {
      constructor(cb: (records: unknown[], o: FsObserver) => void) {
        fire = () => cb([{ type: 'modified' }], this)
      }
      async observe() {}
      disconnect() {
        disconnected = true
      }
    }
    const w = watchFolder({ check, handle: new FakeDirectoryHandle('x').handle, doc: fakeDoc() as unknown as Document, Observer: Obs as FsObserverCtor })
    expect(w.how()).toBe('observer')
    await vi.advanceTimersByTimeAsync(10000)
    expect(check).not.toHaveBeenCalled()
    fire()
    fire()
    await vi.advanceTimersByTimeAsync(200)
    expect(check).toHaveBeenCalledTimes(1)
    w.stop()
    expect(disconnected).toBe(true)
  })

  it('falls back to polling when observe() is refused', async () => {
    vi.useFakeTimers()
    const check = vi.fn()
    class Obs implements FsObserver {
      async observe() {
        throw new DOMException('no', 'NotSupportedError')
      }
      disconnect() {}
    }
    const w = watchFolder({ check, handle: new FakeDirectoryHandle('x').handle, intervalMs: 1000, doc: fakeDoc() as unknown as Document, Observer: Obs as unknown as FsObserverCtor })
    await vi.advanceTimersByTimeAsync(1100)
    expect(w.how()).toBe('poll')
    expect(check).toHaveBeenCalledTimes(1)
    w.stop()
  })
})
