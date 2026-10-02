// SPDX-License-Identifier: MPL-2.0
//
// Editor state: the open project, undo/redo history, navigation, selection.
// Edits go through `commit`, which snapshots the previous project.

import { create } from 'zustand'
import {
  createMissingTags, demoProject, findElement, flatten, reserveProjectIds, type Element, type Id, type Instruction,
  type Program, type Project, type Routine, type Rung,
} from '@/model'
import { applyTheme, initialTheme, type ThemeId } from './theme'

export type View =
  | { kind: 'routine'; program: string; routine: string }
  | { kind: 'tags' }
  | { kind: 'io'; device: string }
  | { kind: 'tasks' }

export type Mode = 'offline' | 'simulate' | 'online'

export interface Selection {
  rungId: Id | null
  elementId: Id | null
}

/** `blocked`: edits are not saved until a disk banner is resolved. */
export type SaveStatus = 'idle' | 'pending' | 'saving' | 'saved' | 'error' | 'memory' | 'blocked'

/** Where the open project lives. */
export type ProjectSource =
  | { kind: 'folder'; key: string; folder: string }
  | { kind: 'browser'; id: string }

/** Something about the project's folder that needs the user. */
export type DiskBanner =
  | { kind: 'permission'; folder: string }
  | { kind: 'conflict'; paths: string[]; error?: string }
  | { kind: 'gone'; folder: string }

/** Dialogs opened from commands (context menus, the palette). */
export type DialogState =
  | { kind: 'manifest'; device: string }
  | { kind: 'updateManifest'; device: string }
  | { kind: 'addDevice'; pane?: 'catalog' | 'import' | 'detect' }
  | { kind: 'import' }
  | { kind: 'export'; program?: string; routine?: string }
  | { kind: 'download' }
  | null

/** Something a view should bring into view or start editing, once. */
export type FocusRequest =
  | { kind: 'tag'; tag: string; rename?: boolean }
  | { kind: 'comment'; rungId: Id }
  | { kind: 'renameRoutine'; program: string; routine: string }
  | { kind: 'point'; tag: string }
  | null

const HISTORY_LIMIT = 200

export interface EditorState {
  /** Null when no project is open: the start screen. */
  projectId: string | null
  projectSource: ProjectSource | null
  disk: DiskBanner | null
  project: Project
  past: Project[]
  future: Project[]
  view: View
  selection: Selection
  /** Element whose operand editor is open. */
  editing: { rungId: Id; elementId: Id } | null
  mode: Mode
  theme: ThemeId
  saveStatus: SaveStatus
  saveError?: string
  paletteOpen: boolean
  projectsOpen: boolean
  dialog: DialogState
  focus: FocusRequest
  /** The generated-ST panel beside a ladder routine. */
  stView: boolean
  /** Short message for the status bar (last action, errors). */
  notice: { text: string; tone: 'info' | 'alarm' | 'fault' } | null

  openProject(id: string | null, project: Project): void
  /** The open project, re-read from disk: keeps the view when it still exists. */
  reloadProject(project: Project): void
  commit(fn: (p: Project) => Project): void
  undo(): void
  redo(): void
  setView(v: View): void
  select(s: Selection): void
  setEditing(e: EditorState['editing']): void
  setMode(m: Mode): void
  setTheme(t: ThemeId): void
  setSaveStatus(s: SaveStatus, error?: string): void
  setPaletteOpen(o: boolean): void
  setProjectsOpen(o: boolean): void
  setDialog(d: DialogState): void
  setFocus(f: FocusRequest): void
  setStView(o: boolean): void
  notify(text: string, tone?: 'info' | 'alarm' | 'fault'): void
}

export function firstRoutineView(p: Project): View {
  const prog = p.pous[0]
  const r = prog?.routines[0]
  return prog && r ? { kind: 'routine', program: prog.name, routine: r.name } : { kind: 'tags' }
}

export const useEditor = create<EditorState>((set, get) => ({
  projectId: null,
  projectSource: null,
  disk: null,
  project: demoProject(),
  past: [],
  future: [],
  view: { kind: 'routine', program: 'MainProgram', routine: 'MainRoutine' },
  selection: { rungId: null, elementId: null },
  editing: null,
  mode: 'offline',
  theme: initialTheme(),
  saveStatus: 'idle',
  paletteOpen: false,
  projectsOpen: false,
  dialog: null,
  focus: null,
  stView: false,
  notice: null,

  openProject(id, project) {
    reserveProjectIds(project)
    set({
      projectId: id,
      project,
      past: [],
      future: [],
      view: firstRoutineView(project),
      selection: { rungId: null, elementId: null },
      editing: null,
      disk: null,
    })
  },
  reloadProject(project) {
    reserveProjectIds(project)
    set({ project, past: [], future: [], editing: null })
    fixView()
  },
  commit(fn) {
    const prev = get().project
    const next = fn(prev)
    if (next === prev) return
    set({ project: next, past: [...get().past, prev].slice(-HISTORY_LIMIT), future: [] })
  },
  undo() {
    const { past, project, future } = get()
    const prev = past[past.length - 1]
    if (!prev) return
    set({ project: prev, past: past.slice(0, -1), future: [project, ...future], editing: null })
    fixView()
  },
  redo() {
    const { past, project, future } = get()
    const next = future[0]
    if (!next) return
    set({ project: next, past: [...past, project], future: future.slice(1), editing: null })
    fixView()
  },
  setView(v) {
    set({ view: v, selection: { rungId: null, elementId: null }, editing: null })
  },
  select(s) {
    set({ selection: s })
  },
  setEditing(e) {
    set({ editing: e })
  },
  setMode(m) {
    set({ mode: m, editing: null })
  },
  setTheme(t) {
    applyTheme(t, true)
    set({ theme: t })
  },
  setSaveStatus(s, error) {
    set({ saveStatus: s, saveError: error })
  },
  setPaletteOpen(o) {
    set({ paletteOpen: o })
  },
  setProjectsOpen(o) {
    set({ projectsOpen: o })
  },
  setDialog(d) {
    set({ dialog: d })
  },
  setFocus(f) {
    set({ focus: f })
  },
  setStView(o) {
    set({ stView: o })
  },
  notify(text, tone = 'info') {
    set({ notice: { text, tone } })
  },
}))

/** After undo/redo the viewed routine may be gone. */
function fixView() {
  const { view, project } = useEditor.getState()
  if (view.kind === 'routine' && !findRoutine(project, view.program, view.routine)) {
    useEditor.setState({ view: firstRoutineView(project) })
  }
}

// ---------------------------------------------------------------- selectors

export function findProgram(p: Project, name: string): Program | undefined {
  return p.pous.find((x) => x.name === name)
}

export function findRoutine(p: Project, program: string, routine: string): Routine | undefined {
  return findProgram(p, program)?.routines.find((r) => r.name === routine)
}

export function currentRoutine(s: Pick<EditorState, 'project' | 'view'>): Routine | undefined {
  return s.view.kind === 'routine' ? findRoutine(s.project, s.view.program, s.view.routine) : undefined
}

export function selectedElement(s: EditorState): Instruction | undefined {
  const r = currentRoutine(s)
  const rung = r?.rungs.find((x) => x.id === s.selection.rungId)
  return rung && s.selection.elementId != null ? findElement(rung.elements, s.selection.elementId) : undefined
}

// ---------------------------------------------------------------- edit helpers

export function mapRoutine(p: Project, program: string, routine: string, fn: (r: Routine) => Routine): Project {
  return {
    ...p,
    pous: p.pous.map((pr) =>
      pr.name !== program ? pr : { ...pr, routines: pr.routines.map((r) => (r.name === routine ? fn(r) : r)) },
    ),
  }
}

/** Applies `fn` to the routine in view and creates any tags new rungs use. */
export function editCurrentRoutine(fn: (r: Routine) => Routine) {
  const { view } = useEditor.getState()
  if (view.kind !== 'routine') return
  useEditor.getState().commit((p) => {
    let next = mapRoutine(p, view.program, view.routine, fn)
    const r = findRoutine(next, view.program, view.routine)
    if (!r) return next
    let tags = next.globals
    const created: string[] = []
    const locals = findProgram(next, view.program)?.variables.map((v) => v.name) ?? []
    for (const rung of r.rungs) {
      const res = createMissingTags(tags, rung.elements, locals)
      tags = res.tags
      created.push(...res.created.map((t) => `${t.name} (${t.data_type})`))
    }
    if (created.length) {
      next = { ...next, globals: tags }
      useEditor.getState().notify(`Created tag${created.length > 1 ? 's' : ''} ${created.join(', ')}`)
    }
    return next
  })
}

export function editRung(rungId: Id, fn: (elements: Element[]) => Element[]) {
  editCurrentRoutine((r) => ({
    ...r,
    rungs: r.rungs.map((g) => (g.id === rungId ? { ...g, elements: fn(g.elements) } : g)),
  }))
}

export function updateRungs(fn: (rungs: Rung[]) => Rung[]) {
  editCurrentRoutine((r) => ({ ...r, rungs: fn(r.rungs) }))
}

/** Position of the selection for the status bar: rung n of m, element i of k. */
export function cursorText(s: EditorState): string {
  const r = currentRoutine(s)
  if (!r) return ''
  const idx = r.rungs.findIndex((g) => g.id === s.selection.rungId)
  if (idx < 0) return `${r.rungs.length} rung${r.rungs.length === 1 ? '' : 's'}`
  const rung = r.rungs[idx]
  const els = flatten(rung.elements)
  const ei = els.findIndex((e) => e.id === s.selection.elementId)
  return `Rung ${idx}${ei >= 0 ? `, element ${ei + 1}/${els.length}` : ''}`
}
