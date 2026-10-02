// SPDX-License-Identifier: MPL-2.0
//
// Where projects live, and the open project's session.
//
// A project is a folder: one the user picked on disk (File System Access
// API: Chrome, Edge), or `projects/<id>/` in the browser's own storage (OPFS:
// the demo, quick experiments, and every project in Firefox and Safari).
// Recent folders are FileSystemDirectoryHandles kept in IndexedDB.
//
// The open project autosaves (debounced) to its folder, writing only files
// whose contents changed, each atomically. A watcher notices when another
// program changes the files: with no unsaved edits the project reloads; with
// unsaved edits a conflict banner asks which to keep. Losing write access or
// the folder itself shows a banner too, instead of failing silently.

import { create } from 'zustand'
import { demoProject, emptyProject, type Project } from '@/model'
import {
  createAutosaver, defaultRecentBackend, DirectoryFileStore, ExternalChangeError, folderAccessSupported, folderExists,
  folderPermission, inspectFolder, isNotFound, isPermissionError, openDefaultStore, pickFolder, ProjectFolder, ProjectRepo,
  RecentProjects, slugify, watchFolder, type Autosaver, type FileStore, type ProjectSummary, type RecentBackend,
  type RecentEntry, type Watcher,
} from '@/store'
import { useEditor, type DiskBanner, type ProjectSource, type View } from './editor'

// ---------------------------------------------------------------- lists for the UI

/** A message for the projects screen: an error, or an offer to open a folder that already is a project. */
export type ProjectsNote =
  | { tone: 'error' | 'info'; text: string; removeKey?: string }
  | { tone: 'offer'; text: string; handle: FileSystemDirectoryHandle }

export interface ProjectsState {
  /** Startup finished (the last project is open, or the start screen shows). */
  ready: boolean
  folderAccess: boolean
  /** Browser storage is OPFS (false: memory only). */
  persistent: boolean
  recent: RecentEntry[]
  browser: ProjectSummary[]
  /** A recent folder that was open last time but needs a click to get access again. */
  resumeKey: string | null
  note: ProjectsNote | null
}

export const useProjects = create<ProjectsState>(() => ({
  ready: false,
  folderAccess: false,
  persistent: false,
  recent: [],
  browser: [],
  resumeKey: null,
  note: null,
}))

export function setProjectsNote(note: ProjectsNote | null) {
  useProjects.setState({ note })
}

// ---------------------------------------------------------------- module state

interface Session {
  key: string
  source: ProjectSource
  folder: ProjectFolder
  handle?: FileSystemDirectoryHandle
  watcher?: Watcher
}

let repo: ProjectRepo | null = null
let recent: RecentProjects | null = null
let autosaver: Autosaver | null = null
let session: Session | null = null
/** Saves and disk checks run one at a time, so a check never sees half a save. */
let diskQueue: Promise<unknown> = Promise.resolve()
/** Set while the editor's project is replaced from disk: that is not an edit. */
let replacing = false

const LAST_KEY = 'plcc-studio.last'
const LEGACY_LAST_KEY = 'plcc-studio.lastProject'
const VIEW_KEY = 'plcc-studio.view:'
export const DEMO_ID = 'demo'

type Last = { kind: 'browser'; id: string } | { kind: 'folder'; key: string }

function storage(): Storage | null {
  try {
    return typeof localStorage === 'undefined' ? null : localStorage
  } catch {
    return null
  }
}

function readLast(): Last | null {
  const s = storage()
  try {
    const raw = s?.getItem(LAST_KEY)
    if (raw) return JSON.parse(raw) as Last
    const legacy = s?.getItem(LEGACY_LAST_KEY)
    if (legacy) return { kind: 'browser', id: legacy }
  } catch {
    // ignore
  }
  return null
}

function writeLast(last: Last | null) {
  const s = storage()
  try {
    s?.removeItem(LEGACY_LAST_KEY)
    if (last) s?.setItem(LAST_KEY, JSON.stringify(last))
    else s?.removeItem(LAST_KEY)
  } catch {
    // not important
  }
}

/** UI state stays out of the project's files: the last view, per project, in localStorage. */
function rememberView(key: string, view: View) {
  try {
    storage()?.setItem(VIEW_KEY + key, JSON.stringify(view))
  } catch {
    // not important
  }
}

function restoreView(key: string) {
  try {
    const raw = storage()?.getItem(VIEW_KEY + key)
    if (!raw) return
    const v = JSON.parse(raw) as View
    const p = useEditor.getState().project
    const ok =
      v.kind === 'tags' ||
      v.kind === 'tasks' ||
      (v.kind === 'io' && p.devices.some((d) => d.name === v.device)) ||
      (v.kind === 'routine' && p.pous.some((x) => x.name === v.program && x.routines.some((r) => r.name === v.routine)))
    if (ok) useEditor.getState().setView(v)
  } catch {
    // not important
  }
}

function needRepo(): ProjectRepo {
  if (!repo) throw new Error('project storage is not ready')
  return repo
}

function needRecent(): RecentProjects {
  if (!recent) throw new Error('project storage is not ready')
  return recent
}

function queue<T>(fn: () => Promise<T>): Promise<T> {
  const run = diskQueue.then(fn, fn)
  diskQueue = run.catch(() => {})
  return run
}

function setBanner(disk: DiskBanner | null) {
  useEditor.setState({ disk })
}

/** The open project has edits that are not on disk yet. */
export function hasUnsavedEdits(): boolean {
  const s = useEditor.getState().saveStatus
  return s === 'pending' || s === 'saving' || s === 'error' || s === 'blocked'
}

export async function refreshLists() {
  const [browser, rec] = await Promise.all([
    repo ? repo.list().catch(() => []) : [],
    recent ? recent.list().catch(() => []) : [],
  ])
  useProjects.setState({ browser, recent: rec })
}

// ---------------------------------------------------------------- startup

let initOnce: Promise<void> | null = null

/** Opens storage and the last project (when it can without asking). Safe to call more than once. */
export function initPersistence(opts: { store?: FileStore; recent?: RecentBackend } = {}): Promise<void> {
  initOnce ??= init(opts)
  return initOnce
}

/** For tests: forget everything initPersistence set up. */
export async function resetPersistenceForTests() {
  await closeSession()
  initOnce = null
  repo = null
  recent = null
  autosaver = null
  useProjects.setState({ ready: false, recent: [], browser: [], resumeKey: null, note: null })
  useEditor.setState({ projectId: null, projectSource: null, disk: null, saveStatus: 'idle' })
}

let subscribed = false

async function init(opts: { store?: FileStore; recent?: RecentBackend }) {
  let persistent = true
  let reason: string | undefined
  let store = opts.store
  if (!store) {
    const d = await openDefaultStore()
    store = d.store
    persistent = d.persistent
    reason = d.reason
  }
  repo = new ProjectRepo(store)
  recent = new RecentProjects(opts.recent ?? defaultRecentBackend())
  autosaver = createAutosaver({ save: (key, project) => queue(() => saveSession(key, project)) }, 600, (status, error) => {
    if (status === 'error' && isBlocking(error)) {
      useEditor.getState().setSaveStatus('blocked', describeError(error))
      return
    }
    const s = persistent || session?.source.kind === 'folder' ? status : status === 'saved' ? 'memory' : status
    useEditor.getState().setSaveStatus(s, error ? describeError(error) : undefined)
  })
  useProjects.setState({ folderAccess: folderAccessSupported(), persistent })

  if (!subscribed) {
    subscribed = true
    let prev = useEditor.getState().project
    let prevId = useEditor.getState().projectId
    let prevView = useEditor.getState().view
    useEditor.subscribe((s) => {
      // Opening another project is not an edit; only changes to the open one are saved.
      const changed = s.project !== prev && s.projectId && s.projectId === prevId && !replacing
      // Update first: scheduling reports status through the same store and re-enters here.
      prev = s.project
      prevId = s.projectId
      if (changed && s.projectId) autosaver?.schedule(s.projectId, s.project)
      if (s.view !== prevView) {
        prevView = s.view
        if (s.projectId && !replacing) rememberView(s.projectId, s.view)
      }
    })
    if (typeof window !== 'undefined') window.addEventListener('beforeunload', () => void autosaver?.flush())
  }

  await refreshLists()
  const last = readLast()
  try {
    if (last?.kind === 'browser' && (await repo.exists(last.id))) await openBrowserProject(last.id)
    else if (last?.kind === 'folder') {
      const e = await recent.get(last.key)
      // Reopening without a click works only while the browser still grants
      // access (Chrome's "allow on every visit"); otherwise the start screen
      // offers the folder, and the click asks.
      if (e && (await folderPermission(e.handle, false).catch(() => 'prompt')) === 'granted') await openFolderHandle(e.handle)
      else if (e) useProjects.setState({ resumeKey: e.key })
    }
  } catch (e) {
    setProjectsNote({ tone: 'error', text: `Could not reopen the last project: ${describeError(e)}` })
  }
  if (!persistent) {
    useEditor.getState().notify(`Browser storage is in memory only: ${reason ?? 'OPFS unavailable'}`, 'alarm')
  }
  useProjects.setState({ ready: true })
}

// ---------------------------------------------------------------- sessions

function isBlocking(e: unknown): boolean {
  return e instanceof ExternalChangeError || isPermissionError(e) || isNotFound(e)
}

export function describeError(e: unknown): string {
  if (e instanceof ExternalChangeError) return `${e.paths.join(', ')} changed on disk`
  if (isPermissionError(e)) return 'no permission to write to the folder'
  if (isNotFound(e)) return 'the folder was moved or deleted'
  return e instanceof Error ? e.message : String(e)
}

async function saveSession(key: string, project: Project) {
  const s = session
  if (!s || s.key !== key) return
  if (useEditor.getState().disk?.kind === 'conflict') throw new ExternalChangeError((useEditor.getState().disk as { paths: string[] }).paths)
  try {
    await s.folder.save(project)
    if (useEditor.getState().disk && session === s) setBanner(null)
  } catch (e) {
    if (session === s) await bannerFor(s, e)
    throw e
  }
}

async function bannerFor(s: Session, e: unknown) {
  const folder = s.source.kind === 'folder' ? s.source.folder : 'browser storage'
  if (e instanceof ExternalChangeError) setBanner({ kind: 'conflict', paths: e.paths })
  else if (isPermissionError(e)) setBanner({ kind: 'permission', folder })
  else if (isNotFound(e) || (s.handle && !(await folderExists(s.handle).catch(() => true)))) setBanner({ kind: 'gone', folder })
}

/** Looks for changes made on disk by other programs (the watcher calls this). */
export function checkDisk(): Promise<void> {
  return queue(async () => {
    const s = session
    if (!s) return
    const st = useEditor.getState()
    // An edit is about to be saved: the save checks first, and raises a conflict.
    if (st.saveStatus === 'pending' || st.saveStatus === 'saving') return
    let changed: string[]
    try {
      changed = await s.folder.changedOnDisk()
    } catch (e) {
      if (session === s) await bannerFor(s, e)
      return
    }
    if (session !== s) return
    if (st.disk?.kind === 'permission' || st.disk?.kind === 'gone') {
      // Reading works again (access granted elsewhere, the folder is back).
      if (!s.handle || (await folderExists(s.handle))) setBanner(null)
    }
    if (!changed.length) return
    if (s.handle && !(await folderExists(s.handle))) {
      setBanner({ kind: 'gone', folder: s.handle.name })
      return
    }
    if (hasUnsavedEdits()) {
      setBanner({ kind: 'conflict', paths: changed })
      useEditor.getState().setSaveStatus('blocked', `${changed.join(', ')} changed on disk`)
      return
    }
    await reloadSession(s, changed)
  })
}

async function reloadSession(s: Session, changed: string[]) {
  let project: Project
  try {
    project = (await s.folder.load()).project
  } catch (e) {
    setBanner({ kind: 'conflict', paths: changed, error: describeError(e) })
    return
  }
  if (session !== s) return
  replacing = true
  try {
    useEditor.getState().reloadProject(project)
  } finally {
    replacing = false
  }
  setBanner(null)
  useEditor.getState().setSaveStatus('saved')
  useEditor.getState().notify(`Reloaded from disk: ${changed.join(', ')} changed`, 'info')
}

/** Conflict: drop the edits in the editor and load what is on disk. */
export function reloadFromDisk(): Promise<void> {
  autosaver?.cancel()
  return queue(async () => {
    const s = session
    if (!s) return
    const changed = useEditor.getState().disk?.kind === 'conflict' ? (useEditor.getState().disk as { paths: string[] }).paths : []
    setBanner(null)
    await reloadSession(s, changed.length ? changed : ['project files'])
  })
}

/** Conflict: write the editor's project over what is on disk. */
export function keepMine(): Promise<void> {
  autosaver?.cancel()
  return queue(async () => {
    const s = session
    if (!s) return
    try {
      await s.folder.save(useEditor.getState().project, { force: true })
      setBanner(null)
      useEditor.getState().setSaveStatus('saved')
    } catch (e) {
      await bannerFor(s, e)
      useEditor.getState().setSaveStatus('error', describeError(e))
    }
  })
}

/** Permission banner: ask again (from the click), then save what is pending. */
export async function grantAccess(): Promise<boolean> {
  const s = session
  if (!s?.handle) return false
  const p = await folderPermission(s.handle, true)
  if (p !== 'granted') {
    useEditor.getState().notify(`Access to “${s.handle.name}” was not granted; edits are not saved`, 'alarm')
    return false
  }
  setBanner(null)
  if (hasUnsavedEdits()) {
    autosaver?.schedule(s.key, useEditor.getState().project)
    await autosaver?.flush()
  }
  void checkDisk()
  return true
}

/** Saves pending edits now. */
export async function flushAutosave() {
  await autosaver?.flush()
}

async function closeSession() {
  await autosaver?.flush()
  const s = session
  s?.watcher?.stop()
  session = null
  // The project may have been renamed (here, or on disk): Recent shows the name it has now.
  if (s?.handle && recent) await recent.touch(s.handle, useEditor.getState().project.name).catch(() => {})
}

function startSession(s: Session, project: Project, migrated?: { notes: string[] }) {
  session = s
  useEditor.getState().openProject(s.key, project)
  useEditor.setState({ projectSource: s.source, disk: null, saveStatus: 'saved', saveError: undefined })
  restoreView(s.key)
  useProjects.setState({ resumeKey: null, note: null })
  s.watcher = watchFolder({ check: () => checkDisk(), handle: s.handle })
  if (migrated) {
    const n = migrated.notes.length
    useEditor
      .getState()
      .notify(
        `${project.name} was saved by an older studio and is now in plcc's ladder model${n ? `: ${migrated.notes.join('; ')}` : ''}`,
        n ? 'alarm' : 'info',
      )
  }
}

/** Closes the open project: the start screen. */
export async function closeProject() {
  await closeSession()
  useEditor.setState({ projectId: null, projectSource: null, disk: null, mode: 'offline', saveStatus: 'idle', projectsOpen: false })
  useProjects.setState({ note: null })
  writeLast(null)
  await refreshLists()
}

// ---------------------------------------------------------------- folders on disk

export type FolderResult = 'opened' | 'cancelled' | 'offered'

function titleFromFolder(name: string): string {
  const t = name.replace(/[-_]+/g, ' ').trim()
  return t ? t[0].toUpperCase() + t.slice(1) : 'Project'
}

/** Opens a folder that holds a project (permission already granted). */
export async function openFolderHandle(handle: FileSystemDirectoryHandle): Promise<void> {
  if (!(await folderExists(handle))) throw new FolderGoneError(handle.name)
  const folder = new ProjectFolder(new DirectoryFileStore(handle))
  const loaded = await folder.load()
  await closeSession()
  const entry = await needRecent().touch(handle, loaded.project.name)
  startSession({ key: `folder:${entry.key}`, source: { kind: 'folder', key: entry.key, folder: handle.name }, folder, handle }, loaded.project, loaded.migrated)
  writeLast({ kind: 'folder', key: entry.key })
  await refreshLists()
}

export class FolderGoneError extends Error {
  constructor(folder: string) {
    super(`“${folder}” was moved or deleted`)
    this.name = 'FolderGoneError'
  }
}

/**
 * New project…: pick (or create, in the picker) an empty folder and write a
 * new project into it. A folder that already is a project is offered for
 * opening instead; any other non-empty folder is refused.
 */
export async function newFolderProject(template?: Project): Promise<FolderResult> {
  const handle = await pickFolder()
  if (!handle) return 'cancelled'
  const store = new DirectoryFileStore(handle)
  const c = await inspectFolder(store, handle.name)
  if (c.kind === 'project') {
    setProjectsNote({
      tone: 'offer',
      text: `“${handle.name}” already holds a plcc project${c.name ? ` (${c.name})` : ''}. Open it instead?`,
      handle,
    })
    return 'offered'
  }
  if (c.kind === 'other') {
    const list = c.entries.slice(0, 4).join(', ') + (c.entries.length > 4 ? `, and ${c.entries.length - 4} more` : '')
    throw new Error(`“${handle.name}” is not empty (${list}). Pick an empty folder; the picker can create one.`)
  }
  const project = template ? { ...template } : emptyProject(titleFromFolder(handle.name))
  await new ProjectFolder(store).create(project)
  await openFolderHandle(handle)
  return 'opened'
}

/** Open project…: pick a folder that holds project.toml and project.json. */
export async function openFolderProject(): Promise<FolderResult> {
  const handle = await pickFolder()
  if (!handle) return 'cancelled'
  const c = await inspectFolder(new DirectoryFileStore(handle), handle.name)
  if (c.kind !== 'project') {
    throw new Error(
      `“${handle.name}” is not a plcc project: it has no project.toml. Pick the folder that holds project.toml and project.json, or use New project… to start one.`,
    )
  }
  await openFolderHandle(handle)
  return 'opened'
}

/** Opens a recent folder. Call it from the click: it may ask the browser for access. */
export async function openRecent(entry: RecentEntry): Promise<void> {
  let p
  try {
    p = await folderPermission(entry.handle, true)
  } catch (e) {
    if (isNotFound(e)) throw goneNote(entry)
    throw e
  }
  if (p !== 'granted') {
    throw new RecentError(`Access to “${entry.folder}” was not allowed. Click it again to allow access, or remove it from the list.`, entry.key)
  }
  try {
    await openFolderHandle(entry.handle)
  } catch (e) {
    if (e instanceof FolderGoneError || isNotFound(e)) throw goneNote(entry)
    throw e
  }
}

function goneNote(entry: RecentEntry) {
  return new RecentError(`“${entry.folder}” was moved or deleted. Remove it from the list, or use Open project… to pick it where it is now.`, entry.key)
}

/** An error about one recent entry: the UI offers to remove it. */
export class RecentError extends Error {
  readonly key: string
  constructor(message: string, key: string) {
    super(message)
    this.name = 'RecentError'
    this.key = key
  }
}

export async function removeRecent(key: string) {
  await needRecent().remove(key)
  if (useProjects.getState().resumeKey === key) useProjects.setState({ resumeKey: null })
  await refreshLists()
}

/**
 * Save to folder…: copies a browser-storage project (the open one, or `id`)
 * into a picked empty folder and switches to it. The browser copy stays.
 */
export async function saveToFolder(id?: string): Promise<FolderResult> {
  let project: Project
  const cur = useEditor.getState()
  if (id === undefined || (cur.projectSource?.kind === 'browser' && cur.projectSource.id === id)) {
    await autosaver?.flush()
    project = cur.project
  } else {
    project = await needRepo().load(id)
  }
  return newFolderProject(project)
}

// ---------------------------------------------------------------- browser storage

export async function openBrowserProject(id: string) {
  const r = needRepo()
  if (!(await r.exists(id))) throw new Error(`project "${id}" not found`)
  await closeSession()
  const folder = r.folder(id)
  const loaded = await folder.load()
  startSession({ key: `browser:${id}`, source: { kind: 'browser', id }, folder }, loaded.project, loaded.migrated)
  writeLast({ kind: 'browser', id })
}

/** A new project in browser storage. */
export async function createBrowserProject(name: string, from?: Project): Promise<string> {
  const p = from ? { ...from, name } : emptyProject(name)
  const id = await needRepo().create(p)
  await openBrowserProject(id)
  await refreshLists()
  return id
}

/** Try the demo: the demo project in browser storage, made the first time. */
export async function openDemo() {
  const r = needRepo()
  if (!(await r.exists(DEMO_ID))) await r.save(DEMO_ID, demoProject())
  await openBrowserProject(DEMO_ID)
  await refreshLists()
}

/** A project made elsewhere (an import): in a folder when the browser can, else in browser storage. */
export async function createProject(name: string, from?: Project): Promise<FolderResult> {
  if (folderAccessSupported()) return newFolderProject(from ? { ...from, name } : emptyProject(name))
  await createBrowserProject(name, from)
  return 'opened'
}

export async function renameBrowserProject(id: string, name: string) {
  await autosaver?.flush()
  await needRepo().rename(id, name)
  const s = useEditor.getState()
  if (s.projectSource?.kind === 'browser' && s.projectSource.id === id) {
    replacing = true
    try {
      useEditor.setState({ project: { ...s.project, name } })
    } finally {
      replacing = false
    }
    // Re-read so the session's snapshot has the new project.toml.
    if (session) await session.folder.load()
  }
  await refreshLists()
}

export async function deleteBrowserProject(id: string) {
  const s = useEditor.getState()
  if (s.projectSource?.kind === 'browser' && s.projectSource.id === id) {
    autosaver?.cancel()
    session?.watcher?.stop()
    session = null
    await needRepo().remove(id)
    await closeProject()
    return
  }
  await needRepo().remove(id)
  await refreshLists()
}

export function isPersistent() {
  return useProjects.getState().persistent
}

export function downloadZip(project: Project) {
  const bytes = needRepo().exportZip(project)
  const blob = new Blob([bytes as BlobPart], { type: 'application/zip' })
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = `${slugify(project.name) || 'project'}.zip`
  a.click()
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}

/** Import .zip: into browser storage (Save to folder… moves it to disk). */
export async function importZipFile(file: File): Promise<string> {
  const bytes = new Uint8Array(await file.arrayBuffer())
  const id = await needRepo().importZip(bytes)
  await openBrowserProject(id)
  await refreshLists()
  return id
}
