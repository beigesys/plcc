// SPDX-License-Identifier: MPL-2.0
//
// Projects in the Origin Private File System (or memory when OPFS is not
// available): bootstrap, autosave, and the project list operations.

import { demoProject, emptyProject, type Project } from '@/model'
import { ProjectRepo, createAutosaver, openDefaultStore, slugify, type Autosaver, type ProjectSummary } from '@/store'
import { useEditor } from './editor'

let repo: ProjectRepo | null = null
let autosaver: Autosaver | null = null
let persistent = false
const LAST_KEY = 'plcc-studio.lastProject'

function remember(id: string) {
  try {
    localStorage.setItem(LAST_KEY, id)
  } catch {
    // not important
  }
}

function lastOpened(): string | null {
  try {
    return localStorage.getItem(LAST_KEY)
  } catch {
    return null
  }
}

let initOnce: Promise<void> | null = null

/** Opens storage and the last project. Safe to call more than once. */
export function initPersistence(): Promise<void> {
  initOnce ??= init()
  return initOnce
}

async function init() {
  const { store, persistent: p, reason } = await openDefaultStore()
  persistent = p
  repo = new ProjectRepo(store)
  autosaver = createAutosaver(repo, 600, (status, error) => {
    const s = persistent ? status : status === 'saved' ? 'memory' : status
    useEditor.getState().setSaveStatus(s, error ? String(error) : undefined)
  })
  const seeded = await repo.ensureSeeded(demoProject)
  const last = lastOpened()
  const id = last && (await repo.exists(last)) ? last : seeded
  await openById(id)
  if (!persistent) {
    useEditor.getState().setSaveStatus('memory')
    useEditor.getState().notify(`Projects are kept in memory only: ${reason ?? 'OPFS unavailable'}`, 'alarm')
  }

  let prev = useEditor.getState().project
  let prevId = useEditor.getState().projectId
  useEditor.subscribe((s) => {
    // Opening another project is not an edit; only changes to the open one are saved.
    const changed = s.project !== prev && s.projectId && s.projectId === prevId
    // Update first: scheduling reports status through the same store and re-enters here.
    prev = s.project
    prevId = s.projectId
    if (changed && s.projectId) autosaver?.schedule(s.projectId, s.project)
  })
  window.addEventListener('beforeunload', () => void autosaver?.flush())
}

function need(): ProjectRepo {
  if (!repo) throw new Error('project storage is not ready')
  return repo
}

export function isPersistent() {
  return persistent
}

export async function listProjects(): Promise<ProjectSummary[]> {
  return need().list()
}

export async function openById(id: string) {
  await autosaver?.flush()
  const project = await need().load(id)
  useEditor.getState().openProject(id, project)
  remember(id)
}

export async function createProject(name: string, from?: Project): Promise<string> {
  const p = from ? { ...from, name } : emptyProject(name)
  const id = await need().create(p)
  await openById(id)
  return id
}

export async function renameProject(id: string, name: string) {
  await autosaver?.flush()
  await need().rename(id, name)
  const s = useEditor.getState()
  if (s.projectId === id) useEditor.setState({ project: { ...s.project, name } })
}

export async function deleteProject(id: string) {
  const r = need()
  autosaver?.cancel()
  await r.remove(id)
  if (useEditor.getState().projectId === id) {
    const rest = await r.list()
    const next = rest[0]?.id ?? (await r.ensureSeeded(demoProject))
    await openById(next)
  }
}

export function downloadZip(project: Project) {
  const bytes = need().exportZip(project)
  const blob = new Blob([bytes as BlobPart], { type: 'application/zip' })
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = `${slugify(project.name) || 'project'}.zip`
  a.click()
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}

export async function importZipFile(file: File): Promise<string> {
  const bytes = new Uint8Array(await file.arrayBuffer())
  const id = await need().importZip(bytes)
  await openById(id)
  return id
}
