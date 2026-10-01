// SPDX-License-Identifier: MPL-2.0
//
// Import and export through plcc's converter (`plcc convert`, in the front-end
// worker): L5X, PLCopen XML, TwinCAT and ST in; L5X, PLCopen XML and ST out,
// with the translation warnings plcc reports.

import {
  fromLadderJson, newId, toLadderJson, withNewIds, type Element, type LadderModel, type Pou, type Project, type Task, type Variable,
} from '@/model'
import { plcc, type Diagnostic } from '@/plcc/frontend'
import { useEditor } from './editor'

export type ExportFormat = 'st' | 'plcopen' | 'l5x'

export interface ConvertOutcome {
  ok: boolean
  text: string | null
  /** Translation and write warnings, reader errors. */
  diagnostics: Diagnostic[]
}

/** The project's model, limited to one program (and one routine of it, first). */
export function scopedModel(p: Project, scope?: { program: string; routine?: string }): Project {
  if (!scope) return p
  const pous = p.pous
    .filter((x) => x.name === scope.program)
    .map((x) => {
      if (!scope.routine) return x
      const r = x.routines.find((y) => y.name === scope.routine)
      return r ? { ...x, routines: [r, ...x.routines.filter((y) => y !== r)] } : x
    })
  const tasks = p.tasks.map((t) => ({ ...t, programs: t.programs.filter((n) => n === scope.program) })).filter((t) => t.programs.length)
  return { ...p, pous, tasks }
}

/** Converts the project (or one program of it) to a notation. */
export async function convertProject(p: Project, to: ExportFormat, opts: { dialect?: 'iec' | 'logix'; scope?: { program: string; routine?: string } } = {}): Promise<ConvertOutcome> {
  const model = toLadderJson(scopedModel(p, opts.scope))
  const r = await plcc.convert({
    files: { 'project.json': model },
    entry: ['project.json'],
    to,
    ...(to === 'st' && opts.dialect ? { dialect: opts.dialect } : {}),
    ...(to === 'st' && opts.dialect !== 'iec' ? { prelude: false } : {}),
  })
  return { ok: r.ok, text: r.output, diagnostics: r.diagnostics }
}

const EXT: Record<ExportFormat, string> = { st: 'st', plcopen: 'xml', l5x: 'L5X' }
const MIME: Record<ExportFormat, string> = { st: 'text/plain', plcopen: 'application/xml', l5x: 'application/xml' }

export function saveText(name: string, text: string, type = 'text/plain') {
  const blob = new Blob([text], { type })
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = name
  a.click()
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}

/** The last export's warnings, for the export dialog. */
export interface ExportReport {
  format: ExportFormat
  file: string
  diagnostics: Diagnostic[]
  ok: boolean
}

let lastReport: ExportReport | null = null
export function lastExportReport() {
  return lastReport
}

/** Exports and downloads; the dialog shows the warnings. ST export is the IEC translation (readable, with warnings). */
export async function exportRoutine(format: ExportFormat, scope?: { program: string; routine?: string }): Promise<ExportReport> {
  const s = useEditor.getState()
  const base = scope?.routine ?? s.project.name.replace(/[^A-Za-z0-9_-]+/g, '_')
  const r = await convertProject(s.project, format, { scope, dialect: format === 'st' ? 'iec' : undefined })
  const file = `${base}.${EXT[format]}`
  if (r.ok && r.text !== null) saveText(file, r.text, MIME[format])
  lastReport = { format, file, diagnostics: r.diagnostics, ok: r.ok }
  const warnings = r.diagnostics.filter((d) => d.severity !== 'error').length
  if (!r.ok) s.notify(`Export failed: ${r.diagnostics.find((d) => d.severity === 'error')?.message ?? 'unknown error'}`, 'fault')
  else s.notify(`Exported ${file}${warnings ? ` with ${warnings} translation note${warnings > 1 ? 's' : ''}` : ''}`, warnings ? 'alarm' : 'info')
  s.setDialog({ kind: 'export', program: scope?.program, routine: scope?.routine })
  return lastReport
}

// ---------------------------------------------------------------- import

export type ImportKind = 'l5x' | 'plcopen' | 'st' | 'twincat'

export function importKind(path: string, text?: string): ImportKind | undefined {
  const p = path.toLowerCase()
  if (p.endsWith('.l5x')) return 'l5x'
  if (p.endsWith('.st') || p.endsWith('.iecst')) return 'st'
  if (p.endsWith('.plcproj') || p.endsWith('.tcpou') || p.endsWith('.tcgvl') || p.endsWith('.tcdut') || p.endsWith('.zip')) return 'twincat'
  if (p.endsWith('.xml')) {
    if (text && /<RSLogix5000Content/.test(text)) return 'l5x'
    return 'plcopen'
  }
  return undefined
}

export interface Imported {
  model: Omit<Required<LadderModel>, 'name'> & { name: string }
  diagnostics: Diagnostic[]
}

/**
 * Reads files (path → text) into a Logix ladder model: L5X as it is, PLCopen
 * XML and ST translated from IEC (warnings name every difference), TwinCAT
 * projects through their ST.
 */
export async function readImport(files: Record<string, string>, kind: ImportKind): Promise<Imported> {
  const diagnostics: Diagnostic[] = []
  let input = files
  let entry = Object.keys(files)
  if (kind === 'twincat') {
    const st = await plcc.convert({ files, to: 'st' })
    diagnostics.push(...st.diagnostics)
    if (!st.ok || st.output === null) throw new ImportError('the TwinCAT project could not be read', diagnostics)
    input = { 'twincat.st': st.output }
    entry = ['twincat.st']
  } else if (entry.length !== 1) {
    throw new ImportError('import one file at a time (or a TwinCAT project)', diagnostics)
  }
  const r = await plcc.convert({ files: input, entry, to: 'ladder-json', dialect: 'logix' })
  diagnostics.push(...r.diagnostics)
  if (!r.ok || r.output === null) throw new ImportError('plcc could not read it', diagnostics)
  return { model: fromLadderJson(r.output, 'import'), diagnostics }
}

export class ImportError extends Error {
  readonly diagnostics: Diagnostic[]
  constructor(message: string, diagnostics: Diagnostic[]) {
    super(message)
    this.diagnostics = diagnostics
  }
}

function freshIds(p: Pou): Pou {
  return {
    ...p,
    id: newId(),
    routines: p.routines.map((r) => ({ ...r, id: newId(), rungs: r.rungs.map((g) => ({ ...g, id: newId(), elements: withNewIds(g.elements as Element[]) })) })),
  }
}

export interface MergeReport {
  programs: string[]
  tags: number
  notes: string[]
}

/** Adds an imported model's programs, tags and tasks to a project (renaming what clashes). */
export function mergeImport(p: Project, m: Imported['model']): { project: Project; report: MergeReport } {
  const notes: string[] = []
  const taken = new Set(p.pous.map((x) => x.name.toLowerCase()))
  const renamed = new Map<string, string>()
  const pous: Pou[] = m.pous.map((x) => {
    let name = x.name
    for (let n = 2; taken.has(name.toLowerCase()); n++) name = `${x.name}_${n}`
    taken.add(name.toLowerCase())
    if (name !== x.name) {
      renamed.set(x.name, name)
      notes.push(`program ${x.name} imported as ${name} (the project has one of that name)`)
    }
    return freshIds({ ...x, name })
  })
  const have = new Map(p.globals.map((v) => [v.name.toLowerCase(), v]))
  const globals: Variable[] = []
  for (const v of m.globals) {
    const cur = have.get(v.name.toLowerCase())
    if (cur) {
      if (cur.data_type.toUpperCase() !== v.data_type.toUpperCase()) {
        notes.push(`tag ${v.name}: the project's ${cur.data_type} is kept, the import has ${v.data_type}`)
      }
      continue
    }
    globals.push({ ...v, section: 'global' })
  }
  let tasks: Task[] = p.tasks
  const imported = pous.map((x) => x.name)
  const fromTasks = m.tasks
    .map((t) => ({ ...t, programs: t.programs.map((n) => renamed.get(n) ?? n).filter((n) => imported.includes(n)) }))
    .filter((t) => t.programs.length)
  const scheduled = new Set(fromTasks.flatMap((t) => t.programs))
  const unscheduled = imported.filter((n) => !scheduled.has(n))
  if (fromTasks.length) {
    const names = new Set(p.tasks.map((t) => t.name.toLowerCase()))
    tasks = [...tasks, ...fromTasks.map((t) => (names.has(t.name.toLowerCase()) ? { ...t, name: `${t.name}_import` } : t))]
  }
  if (unscheduled.length) {
    if (tasks.length) tasks = tasks.map((t, i) => (i === 0 ? { ...t, programs: [...t.programs, ...unscheduled] } : t))
    else tasks = [{ name: 'MainTask', interval_ms: 10, programs: unscheduled }]
  }
  const declarations = [...p.declarations, ...m.declarations]
  if (m.declarations.length) notes.push(`${m.declarations.length} declaration(s) (types, classes) are carried along as ST; the ladder editor does not show them`)
  return {
    project: { ...p, pous: [...p.pous, ...pous], globals: [...p.globals, ...globals], tasks, declarations },
    report: { programs: pous.map((x) => x.name), tags: globals.length, notes },
  }
}
