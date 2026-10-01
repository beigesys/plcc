// SPDX-License-Identifier: MPL-2.0
// The Problems panel: plcc's diagnostics for the project, each one a link to
// its rung, element, ST line, tag or device.

import { AlertTriangle, CircleX, Info, X } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { findProgram, useEditor } from '@/state/editor'
import { countBySeverity, useProblems, type Problem } from '@/state/problems'

export function goToProblem(p: Problem) {
  const s = useEditor.getState()
  const place = p.place
  if (place.kind === 'element') {
    const prog = findProgram(s.project, place.program)
    if (!prog?.routines.some((r) => r.name === place.routine)) return
    s.setView({ kind: 'routine', program: place.program, routine: place.routine })
    if (place.rung !== undefined) s.select({ rungId: place.rung, elementId: place.element ?? null })
    requestAnimationFrame(() => {
      const st = document.getElementById('st-editor') as HTMLTextAreaElement | null
      if (st && p.span) {
        st.focus()
        st.setSelectionRange(p.span.utf16, Math.max(p.span.endUtf16, p.span.utf16 + 1))
      } else document.getElementById('rung-list')?.focus({ preventScroll: true })
    })
  } else if (place.kind === 'tag') {
    s.setView({ kind: 'tags' })
    s.setFocus({ kind: 'tag', tag: place.tag })
  } else if (place.kind === 'device') {
    s.setView({ kind: 'io', device: place.device })
  }
}

function where(p: Problem): string {
  const pl = p.place
  if (pl.kind === 'element') return `${pl.program}/${pl.routine}${pl.rung !== undefined ? ' · rung' : ''}`
  if (pl.kind === 'tag') return `tag ${pl.tag}`
  if (pl.kind === 'device') return `device ${pl.device}`
  return 'project'
}

function rungIndex(p: Problem): string {
  if (p.place.kind !== 'element' || p.place.rung === undefined) return ''
  const pl = p.place
  const r = findProgram(useEditor.getState().project, pl.program)?.routines.find((x) => x.name === pl.routine)
  const i = r?.rungs.findIndex((g) => g.id === pl.rung) ?? -1
  return i >= 0 ? ` ${i}` : ''
}

export function ProblemsPanel() {
  const open = useProblems((s) => s.panelOpen)
  const problems = useProblems((s) => s.problems)
  const failure = useProblems((s) => s.failure)
  const checking = useProblems((s) => s.checking)
  const setOpen = useProblems((s) => s.setPanelOpen)
  if (!open) return null
  const { errors, warnings } = countBySeverity(problems)
  return (
    <section aria-label="Problems" className="flex max-h-56 min-h-28 flex-col border-t border-line bg-surface">
      <div className="flex items-center gap-3 border-b border-line px-3 py-1.5 text-dense">
        <h2 className="text-[11px] font-semibold tracking-wider text-text-muted uppercase">Problems</h2>
        <span className="text-text-muted">
          {errors} error{errors === 1 ? '' : 's'}, {warnings} warning{warnings === 1 ? '' : 's'}
          {checking ? ' · checking…' : ''}
        </span>
        <span className="text-[11px] text-text-muted">plcc check, in your browser</span>
        <Button size="icon-xs" variant="ghost" className="ml-auto" aria-label="Close problems" onClick={() => setOpen(false)}>
          <X />
        </Button>
      </div>
      <ul className="min-h-0 flex-1 overflow-y-auto py-1 text-dense">
        {failure && <li className="px-3 py-1 text-fault">The plcc front end failed: {failure}</li>}
        {problems.length === 0 && !failure && <li className="px-3 py-1 text-text-muted">No problems.</li>}
        {problems.map((p, i) => (
          <li key={i}>
            <button
              type="button"
              onClick={() => goToProblem(p)}
              className="flex w-full items-start gap-2 px-3 py-1 text-left hover:bg-surface-2"
            >
              {p.severity === 'error' ? (
                <CircleX className="mt-0.5 size-3.5 shrink-0 text-fault" aria-label="error" />
              ) : p.severity === 'warning' ? (
                <AlertTriangle className="mt-0.5 size-3.5 shrink-0 text-alarm" aria-label="warning" />
              ) : (
                <Info className="mt-0.5 size-3.5 shrink-0 text-text-muted" aria-label="note" />
              )}
              <span className="min-w-0 flex-1">
                {p.message}
                {p.help && <span className="text-text-muted"> · {p.help}</span>}
              </span>
              <span className="shrink-0 text-text-muted">
                {where(p)}
                {rungIndex(p)}
                {p.span ? ` · ${p.span.line}:${p.span.col}` : ''}
              </span>
            </button>
          </li>
        ))}
      </ul>
    </section>
  )
}

/** The status bar's problem counter, which opens the panel. */
export function ProblemsButton() {
  const problems = useProblems((s) => s.problems)
  const checking = useProblems((s) => s.checking)
  const open = useProblems((s) => s.panelOpen)
  const setOpen = useProblems((s) => s.setPanelOpen)
  const { errors, warnings } = countBySeverity(problems)
  return (
    <button
      type="button"
      onClick={() => setOpen(!open)}
      aria-pressed={open}
      aria-label={`Problems: ${errors} errors, ${warnings} warnings`}
      className="flex items-center gap-1.5 rounded-control px-1.5 hover:bg-surface-2"
    >
      <CircleX className={`size-3.5 ${errors ? 'text-fault' : 'text-text-muted'}`} aria-hidden />
      <span className={errors ? 'text-fault' : 'text-text-muted'}>{errors}</span>
      <AlertTriangle className={`size-3.5 ${warnings ? 'text-alarm' : 'text-text-muted'}`} aria-hidden />
      <span className={warnings ? 'text-alarm' : 'text-text-muted'}>{warnings}</span>
      {checking && <span className="text-text-muted">checking…</span>}
    </button>
  )
}
