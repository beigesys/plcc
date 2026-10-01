// SPDX-License-Identifier: MPL-2.0
//
// Problems: plcc's diagnostics for the open project (`check` on the ladder
// model, in the front-end worker, debounced after edits) and the device
// manifests' (`validateDevice`), placed on programs, routines, rungs,
// elements, tags and devices.

import { create } from 'zustand'
import { toLadderJson, type Id, type Project } from '@/model'
import { plcc, type Diagnostic } from '@/plcc/frontend'

export type ProblemSeverity = 'error' | 'warning' | 'advice'

export type ProblemPlace =
  | { kind: 'element'; program: string; routine: string; rung?: Id; element?: Id; operand?: number }
  | { kind: 'tag'; tag: string }
  | { kind: 'device'; device: string; path?: string }
  | { kind: 'project' }

export interface Problem {
  severity: ProblemSeverity
  message: string
  help?: string | null
  source: 'check' | 'device' | 'compile' | 'import'
  place: ProblemPlace
  /** In the rung's text (line 1) or the ST code. */
  span?: { line: number; col: number; endLine: number; endCol: number; utf16: number; endUtf16: number }
}

export interface ProblemsState {
  problems: Problem[]
  checking: boolean
  /** When the last check finished (ms since epoch). */
  checkedAt: number
  /** Set when the front end itself failed (not a diagnostic). */
  failure: string | null
  panelOpen: boolean
  setPanelOpen(o: boolean): void
}

export const useProblems = create<ProblemsState>((set) => ({
  problems: [],
  checking: false,
  checkedAt: 0,
  failure: null,
  panelOpen: false,
  setPanelOpen: (o) => set({ panelOpen: o }),
}))

/** A plcc diagnostic as a problem. */
export function toProblem(d: Diagnostic, source: Problem['source'] = 'check'): Problem {
  const l = d.ladder
  let place: ProblemPlace = { kind: 'project' }
  if (l?.routine && l.pou) {
    place = { kind: 'element', program: l.pou, routine: l.routine, rung: l.rung, element: l.element, operand: l.operand }
  } else if (l?.tag) place = { kind: 'tag', tag: l.tag }
  const severity: ProblemSeverity = d.severity
  const p: Problem = { severity, message: d.message, help: d.help, source, place }
  if (d.span && l) {
    p.span = {
      line: d.span.start.line,
      col: d.span.start.col,
      endLine: d.span.end.line,
      endCol: d.span.end.col,
      utf16: d.span.start.utf16,
      endUtf16: d.span.end.utf16,
    }
  }
  return p
}

/** The check request for a project: its model as the one input. */
export function checkRequest(project: Project) {
  return { files: { 'project.json': toLadderJson(project) }, entry: ['project.json'], tags: false }
}

let generation = 0

/** Checks the project and its device manifests; the latest call wins. */
export async function runCheck(project: Project): Promise<Problem[]> {
  const gen = ++generation
  useProblems.setState({ checking: true })
  try {
    const result = await plcc.check(checkRequest(project))
    const problems = result.diagnostics.map((d) => toProblem(d))
    for (const dev of project.devices) {
      const text = project.deviceFiles[dev.manifest]
      if (text === undefined) {
        problems.push({ severity: 'error', message: `${dev.manifest} is missing`, source: 'device', place: { kind: 'device', device: dev.name } })
        continue
      }
      const r = await plcc.validateDevice(text, dev.manifest)
      for (const d of r.diagnostics) {
        problems.push({
          severity: d.severity,
          message: `${d.path ? `${d.path}: ` : ''}${d.message}${d.line ? ` (line ${d.line})` : ''}`,
          source: 'device',
          place: { kind: 'device', device: dev.name, path: d.path },
        })
      }
    }
    if (gen === generation) useProblems.setState({ problems, checking: false, checkedAt: Date.now(), failure: null })
    return problems
  } catch (e) {
    if (gen === generation) useProblems.setState({ checking: false, failure: e instanceof Error ? e.message : String(e) })
    return []
  }
}

let timer: ReturnType<typeof setTimeout> | undefined

/** Re-check after edits settle. */
export function scheduleCheck(project: Project, delayMs = 400) {
  clearTimeout(timer)
  timer = setTimeout(() => void runCheck(project), delayMs)
}

/** Problems in one routine, by element id (the worst per element). */
export function elementMarks(problems: Problem[], program: string, routine: string): Map<Id, Problem> {
  const out = new Map<Id, Problem>()
  for (const p of problems) {
    if (p.place.kind !== 'element' || p.place.program !== program || p.place.routine !== routine) continue
    const id = p.place.element ?? p.place.rung
    if (id === undefined) continue
    const cur = out.get(id)
    if (!cur || (cur.severity !== 'error' && p.severity === 'error')) out.set(id, p)
  }
  return out
}

/** Problems of one rung (its elements included). */
export function rungProblems(problems: Problem[], program: string, routine: string, rung: Id): Problem[] {
  return problems.filter((p) => p.place.kind === 'element' && p.place.program === program && p.place.routine === routine && p.place.rung === rung)
}

export function countBySeverity(problems: Problem[]): { errors: number; warnings: number } {
  let errors = 0
  let warnings = 0
  for (const p of problems) {
    if (p.severity === 'error') errors++
    else if (p.severity === 'warning') warnings++
  }
  return { errors, warnings }
}
