// SPDX-License-Identifier: MPL-2.0
import { afterEach, describe, expect, it, vi } from 'vitest'
import { demoProject, emptyProject } from '@/model'
import type { Project } from '@/model'
import {
  MemoryFileStore, ProjectFormatError, ProjectRepo, createAutosaver, projectFromFiles, projectNeedsMigration, projectToFiles,
} from '@/store'
import { catalogEntry } from '@/devices/catalog'
import type { AutosaveStatus } from '@/store'

function withSt(): Project {
  const p = demoProject()
  p.programs[0].routines.push({ name: 'Calc', kind: 'st', rungs: [], st: 'x := x + 1;\n' })
  return p
}

describe('project files', () => {
  it('writes the documented layout', () => {
    const files = projectToFiles(withSt())
    expect(Object.keys(files).sort()).toEqual([
      'devices/arduino-opta.toml', 'project.toml', 'routines/Calc.st', 'routines/MainRoutine.ladder.json', 'tags.toml',
    ])
    expect(files['project.toml']).toContain('manifest = "devices/arduino-opta.toml"')
    expect(files['devices/arduino-opta.toml']).toContain('id = "arduino-opta"')
    expect(files['project.toml']).toContain('name = "Demo Opta"')
    expect(files['project.toml']).toContain('interval_ms = 10')
    expect(files['tags.toml']).toContain('address = "%MX0.0"')
    const ladder = JSON.parse(files['routines/MainRoutine.ladder.json'])
    expect(ladder.format).toBe('plcc-studio-ladder')
    expect(ladder.rungs).toHaveLength(3)
  })

  it('round-trips the demo project', () => {
    const p = demoProject()
    expect(projectFromFiles(projectToFiles(p))).toEqual(p)
  })

  it('round-trips an ST routine and a tag without address', () => {
    const p = withSt()
    const back = projectFromFiles(projectToFiles(p))
    expect(back).toEqual(p)
    expect(back.tags.find((t) => t.name === 'RunTimer')?.address).toBeUndefined()
  })

  it('reports a missing project.toml', () => {
    expect(() => projectFromFiles({})).toThrow(/project.toml: file is missing/)
  })

  it('reports bad TOML with the file name', () => {
    const files = projectToFiles(demoProject())
    files['tags.toml'] = '[[tag]\nname = '
    expect(() => projectFromFiles(files)).toThrow(ProjectFormatError)
    expect(() => projectFromFiles(files)).toThrow(/tags.toml: invalid TOML/)
  })

  it('reports bad JSON and bad elements', () => {
    const files = projectToFiles(demoProject())
    expect(() => projectFromFiles({ ...files, 'routines/MainRoutine.ladder.json': '{ nope' })).toThrow(
      /MainRoutine.ladder.json: invalid JSON/,
    )
    const doc = JSON.parse(files['routines/MainRoutine.ladder.json'])
    doc.rungs[0].body.items[1].kind = 'sideways'
    expect(() =>
      projectFromFiles({ ...files, 'routines/MainRoutine.ladder.json': JSON.stringify(doc) }),
    ).toThrow(/rungs\[0\].body.items\[1\]: bad contact kind/)
  })

  it('reports a missing routine file', () => {
    const files = projectToFiles(demoProject())
    delete files['routines/MainRoutine.ladder.json']
    expect(() => projectFromFiles(files)).toThrow(/routines\/MainRoutine.ladder.json: file is missing/)
  })
})

describe('ProjectRepo', () => {
  it('creates, lists, loads, renames and removes', async () => {
    const repo = new ProjectRepo(new MemoryFileStore())
    expect(await repo.list()).toEqual([])
    const demo = demoProject()
    const id = await repo.create(demo)
    expect(id).toMatch(/^demo-opta-[a-z0-9]{4}$/)
    expect(await repo.list()).toEqual([{ id, name: 'Demo Opta' }])
    expect(await repo.load(id)).toEqual(demo)
    await repo.rename(id, 'Pump Skid')
    expect(await repo.list()).toEqual([{ id, name: 'Pump Skid' }])
    expect((await repo.load(id)).name).toBe('Pump Skid')
    const id2 = await repo.create(emptyProject('Other'))
    expect((await repo.list()).map((p) => p.id).sort()).toEqual([id, id2].sort())
    await repo.remove(id)
    expect(await repo.list()).toEqual([{ id: id2, name: 'Other' }])
    await expect(repo.load(id)).rejects.toThrow(/not found/)
  })

  it('removes routine files that are no longer in the project', async () => {
    const store = new MemoryFileStore()
    const repo = new ProjectRepo(store)
    const id = await repo.create(withSt())
    expect(await store.readText(`projects/${id}/routines/Calc.st`)).not.toBeNull()
    await repo.save(id, demoProject())
    expect(await store.readText(`projects/${id}/routines/Calc.st`)).toBeNull()
    expect(await store.readText(`projects/${id}/routines/MainRoutine.ladder.json`)).not.toBeNull()
  })

  it('exports and imports a zip', async () => {
    const repo = new ProjectRepo(new MemoryFileStore())
    const p = withSt()
    const zip = repo.exportZip(p)
    expect(zip[0]).toBe(0x50) // "PK"
    const id = await repo.importZip(zip)
    expect(await repo.load(id)).toEqual(p)
  })

  it('rejects a non-zip import', async () => {
    const repo = new ProjectRepo(new MemoryFileStore())
    await expect(repo.importZip(new Uint8Array([1, 2, 3]))).rejects.toThrow(ProjectFormatError)
  })

  it('seeds only once', async () => {
    const repo = new ProjectRepo(new MemoryFileStore())
    const seed = vi.fn(demoProject)
    const a = await repo.ensureSeeded(seed)
    const b = await repo.ensureSeeded(seed)
    expect(a).toBe(b)
    expect(seed).toHaveBeenCalledTimes(1)
    expect(await repo.list()).toHaveLength(1)
  })
})

describe('MemoryFileStore', () => {
  it('lists files and directories and removes recursively', async () => {
    const s = new MemoryFileStore()
    await s.writeText('a/b/c.txt', '1')
    await s.writeText('a/d.txt', '2')
    expect(await s.list('a')).toEqual([{ name: 'b', kind: 'dir' }, { name: 'd.txt', kind: 'file' }])
    await s.remove('a/b')
    expect(await s.list('a')).toEqual([{ name: 'd.txt', kind: 'file' }])
    await s.remove('missing')
    expect(await s.list('nope')).toEqual([])
  })
})

describe('autosaver', () => {
  afterEach(() => {
    vi.useRealTimers()
  })

  it('debounces saves and reports status', async () => {
    vi.useFakeTimers()
    const repo = new ProjectRepo(new MemoryFileStore())
    const id = await repo.create(demoProject())
    const save = vi.spyOn(repo, 'save')
    const statuses: AutosaveStatus[] = []
    const saver = createAutosaver(repo, 500, (s) => statuses.push(s))
    for (let i = 0; i < 5; i++) {
      const p = demoProject()
      p.name = `v${i}`
      saver.schedule(id, p)
      await vi.advanceTimersByTimeAsync(100)
    }
    expect(save).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(500)
    expect(save).toHaveBeenCalledTimes(1)
    expect((await repo.load(id)).name).toBe('v4')
    expect(statuses[0]).toBe('pending')
    expect(statuses.slice(-2)).toEqual(['saving', 'saved'])
  })

  it('flushes immediately and reports errors', async () => {
    const repo = new ProjectRepo(new MemoryFileStore())
    const statuses: AutosaveStatus[] = []
    const saver = createAutosaver(repo, 10_000, (s) => statuses.push(s))
    vi.spyOn(repo, 'save').mockRejectedValueOnce(new Error('disk full'))
    saver.schedule('x-0000', demoProject())
    await saver.flush()
    expect(statuses.at(-1)).toBe('error')
    saver.schedule('x-0000', demoProject())
    await saver.flush()
    expect(statuses.at(-1)).toBe('saved')
  })
})

/** project.toml as studio wrote it before device manifests. */
const LEGACY_PROJECT = `name = "Old"

[[devices]]
name = "Opta"
profile = "arduino-opta"

[[tasks]]
name = "MainTask"
interval_ms = 10
programs = ["MainProgram"]

[[programs]]
name = "MainProgram"
main = "MainRoutine"

[[programs.routines]]
name = "MainRoutine"
kind = "ladder"
`
const EMPTY_LADDER = '{ "format": "plcc-studio-ladder", "version": 1, "name": "MainRoutine", "rungs": [] }'

describe('device manifests in projects', () => {
  it('migrates a project saved with a device profile', async () => {
    const files = { 'project.toml': LEGACY_PROJECT, 'routines/MainRoutine.ladder.json': EMPTY_LADDER }
    expect(projectNeedsMigration(files)).toBe(true)
    const p = projectFromFiles(files)
    expect(p.devices).toEqual([{ name: 'Opta', manifest: 'devices/arduino-opta.toml' }])
    expect(p.deviceFiles['devices/arduino-opta.toml']).toBe(catalogEntry('arduino-opta')!.text)

    const store = new MemoryFileStore()
    for (const [k, v] of Object.entries(files)) await store.writeText(`projects/old/${k}`, v)
    const repo = new ProjectRepo(store)
    await repo.load('old')
    // The load wrote the devices/ folder and the new project.toml.
    expect(await store.readText('projects/old/devices/arduino-opta.toml')).toBe(catalogEntry('arduino-opta')!.text)
    const pt = (await store.readText('projects/old/project.toml'))!
    expect(pt).toContain('manifest = "devices/arduino-opta.toml"')
    expect(projectNeedsMigration({ 'project.toml': pt })).toBe(false)
  })

  it('migrates an unknown legacy profile to the Simulator', () => {
    const p = projectFromFiles({
      'project.toml': LEGACY_PROJECT.replace('profile = "arduino-opta"', 'profile = "gone-board"'),
      'routines/MainRoutine.ladder.json': EMPTY_LADDER,
    })
    expect(p.devices[0].manifest).toBe('devices/simulator.toml')
  })

  it('rejects manifest paths outside devices/ and missing manifest files', () => {
    const files = projectToFiles(demoProject())
    const evil = files['project.toml'].replace('devices/arduino-opta.toml', '../../escape.toml')
    expect(() => projectFromFiles({ ...files, 'project.toml': evil })).toThrow(/must be devices\/<id>\.toml/)
    const { ['devices/arduino-opta.toml']: _, ...rest } = files
    expect(_).toBeDefined()
    expect(() => projectFromFiles(rest)).toThrow(/devices\/arduino-opta.toml: file is missing/)
  })

  it('removes manifest files no device uses when saving', async () => {
    const repo = new ProjectRepo(new MemoryFileStore())
    const p = demoProject()
    const id = await repo.create(p)
    expect(await repo.store.readText(`projects/${id}/devices/arduino-opta.toml`)).not.toBeNull()
    const sim = catalogEntry('simulator')!
    await repo.save(id, { ...p, devices: [{ name: 'Sim', manifest: 'devices/simulator.toml' }], deviceFiles: { 'devices/simulator.toml': sim.text } })
    expect(await repo.store.readText(`projects/${id}/devices/arduino-opta.toml`)).toBeNull()
    expect((await repo.load(id)).devices[0].name).toBe('Sim')
  })

  it('exports and imports devices in the zip', async () => {
    const repo = new ProjectRepo(new MemoryFileStore())
    const back = ProjectRepo.projectFromZip(repo.exportZip(demoProject()))
    expect(back.deviceFiles).toEqual(demoProject().deviceFiles)
  })

  it('seeds an empty project with the Simulator', () => {
    const p = emptyProject('x')
    expect(p.devices).toEqual([{ name: 'Simulator', manifest: 'devices/simulator.toml' }])
    expect(Object.keys(p.deviceFiles)).toEqual(['devices/simulator.toml'])
  })
})
