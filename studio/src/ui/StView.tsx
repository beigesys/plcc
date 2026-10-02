// SPDX-License-Identifier: MPL-2.0
// The ST view: the Structured Text plcc generates from a ladder routine's
// program, read-only, next to the rungs. "As compiled" is what plcc compiles
// (Logix semantics, calls into its Logix prelude); "IEC" is the translation
// to IEC 61131-3 ladder lowered to ST, with the translation's notes.

import { useEffect, useMemo, useState } from 'react'
import { X } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { convertProject, type ConvertOutcome } from '@/state/convert'
import { useEditor } from '@/state/editor'
import { StCode } from './StCode'

type Flavor = 'logix' | 'iec'

export function StView({ program, routine }: { program: string; routine: string }) {
  const project = useEditor((s) => s.project)
  const setStView = useEditor((s) => s.setStView)
  const selection = useEditor((s) => s.selection)
  const [flavor, setFlavor] = useState<Flavor>('logix')
  const [out, setOut] = useState<ConvertOutcome | null>(null)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    let stale = false
    const id = setTimeout(() => {
      setBusy(true)
      convertProject(project, 'st', { dialect: flavor, scope: { program, routine } })
        .then((r) => !stale && setOut(r))
        .catch((e: unknown) => !stale && setOut({ ok: false, text: null, diagnostics: [{ message: String(e), severity: 'error' } as never] }))
        .finally(() => !stale && setBusy(false))
    }, 250)
    return () => {
      stale = true
      clearTimeout(id)
    }
  }, [project, program, routine, flavor])

  const rungIndex = useMemo(() => {
    const r = project.pous.find((p) => p.name === program)?.routines.find((x) => x.name === routine)
    return r?.rungs.findIndex((g) => g.id === selection.rungId) ?? -1
  }, [project, program, routine, selection.rungId])

  // Bring the selected rung's statements into view.
  const scrollToLine = useMemo(() => {
    const text = out?.text
    if (!text || rungIndex < 0) return undefined
    const from = flavor === 'logix' ? Math.max(0, text.indexOf(`METHOD R_${routine}`)) : 0
    const at = text.indexOf(`rung ${rungIndex}`, from)
    return at < 0 ? undefined : text.slice(0, at).split('\n').length
  }, [out, rungIndex, flavor, routine])

  const notes = out?.diagnostics ?? []
  return (
    <aside aria-label="ST view" className="flex w-[44%] min-w-[360px] flex-col border-l border-line bg-surface">
      <div className="flex items-center gap-2 border-b border-line px-3 py-2">
        <h2 className="text-[11px] font-semibold tracking-wider text-text-muted uppercase">ST view</h2>
        <div role="radiogroup" aria-label="ST flavor" className="flex rounded-control border border-line p-0.5 text-dense">
          {(['logix', 'iec'] as Flavor[]).map((f) => (
            <button
              key={f}
              type="button"
              role="radio"
              aria-checked={flavor === f}
              onClick={() => setFlavor(f)}
              className={`rounded-control px-2 py-0.5 ${flavor === f ? 'bg-surface-2 text-text' : 'text-text-muted hover:text-text'}`}
            >
              {f === 'logix' ? 'As compiled' : 'IEC translation'}
            </button>
          ))}
        </div>
        {busy && <span className="text-[11px] text-text-muted">updating…</span>}
        <Button size="icon-xs" variant="ghost" className="ml-auto" aria-label="Close ST view" onClick={() => setStView(false)}>
          <X />
        </Button>
      </div>
      <StCode
        readOnly
        testId="st-view-text"
        ariaLabel={`Generated Structured Text for ${program}`}
        value={out?.text ?? (out ? '(* plcc cannot generate Structured Text while the project has errors: see below and in Problems. *)' : '(* Generating… *)')}
        scrollToLine={scrollToLine}
      />
      {notes.length > 0 && (
        <details className="border-t border-line text-dense" open={notes.some((d) => d.severity === 'error')}>
          <summary className="cursor-pointer px-3 py-1.5 text-text-muted select-none hover:text-text">
            {notes.some((d) => d.severity === 'error')
              ? `Problems (${notes.length})`
              : `Translation notes (${notes.length}): where the two dialects behave differently`}
          </summary>
          <ul aria-label="Translation notes" className="max-h-40 space-y-1.5 overflow-y-auto px-3 pb-2">
            {notes.map((d, i) => (
              <li key={i} className={d.severity === 'error' ? 'text-fault' : 'text-text-muted'}>
                {d.message}
              </li>
            ))}
          </ul>
        </details>
      )}
    </aside>
  )
}
