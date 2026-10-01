// SPDX-License-Identifier: MPL-2.0
import { useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type MouseEvent } from 'react'
import { Code2, GitBranch, Plus, Redo2, Undo2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { ContextMenu, ContextMenuTrigger } from '@/components/ui/context-menu'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { findElement, isStRoutine, RungTextError, stRoutineCode, withStRoutineCode, type Id, type Routine } from '@/model'
import { addRung, addRungFromQuickEntry, insertInstruction, moveRung, moveSelectedElement, moveSelection, wrapSelectionInBranch } from '@/state/commands'
import { currentRoutine, mapRoutine, useEditor } from '@/state/editor'
import { toggleTag } from '@/state/force'
import { useLiveRead, useTagMap, useTrace } from '@/state/hooks'
import { useLive } from '@/state/live'
import { elementMarks, useProblems, type Problem } from '@/state/problems'
import { useSimBuild } from '@/state/simulate'
import { commandForKey, keyName, menuFor, type Target } from '@/state/registry'
import { CommandMenuContent } from './CommandMenu'
import { PALETTE_MIME, describeElement, type Mark } from './ladder/Ladder'
import { RungCard } from './RungCard'
import { StView } from './StView'
import { useDraft } from './useDraft'

const CHIPS = ['XIC', 'XIO', 'OTE', 'OTL', 'OTU', 'ONS', 'TON', 'TOF', 'CTU', 'RES', 'GRT', 'EQU', 'LES', 'ADD', 'MOV', 'CPT']

function Chip({ m }: { m: string }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          draggable
          onDragStart={(e) => {
            e.dataTransfer.setData(PALETTE_MIME, m)
            e.dataTransfer.setData('text/plain', m)
            e.dataTransfer.effectAllowed = 'copy'
          }}
          onClick={() => insertInstruction(m)}
          className="cursor-grab rounded-control border border-line bg-surface-2 px-1.5 py-0.5 text-mono text-text-muted hover:border-text-muted hover:text-text active:cursor-grabbing"
        >
          {m}
        </button>
      </TooltipTrigger>
      <TooltipContent>Click to insert after the selection, or drag onto a rung</TooltipContent>
    </Tooltip>
  )
}

function Toolbar() {
  const canUndo = useEditor((s) => s.past.length > 0)
  const canRedo = useEditor((s) => s.future.length > 0)
  const undo = useEditor((s) => s.undo)
  const redo = useEditor((s) => s.redo)
  const stView = useEditor((s) => s.stView)
  const setStView = useEditor((s) => s.setStView)
  return (
    <div className="flex flex-wrap items-center gap-1.5 border-b border-line bg-surface px-3 py-2" role="toolbar" aria-label="Ladder tools">
      <Button size="sm" variant="outline" onClick={() => addRung()}>
        <Plus /> Rung
      </Button>
      <Button size="sm" variant="outline" onClick={wrapSelectionInBranch} title="Add a parallel branch around the selection (P)">
        <GitBranch /> Branch
      </Button>
      <span className="mx-1 h-5 w-px bg-line" aria-hidden />
      <div className="flex flex-wrap gap-1" aria-label="Instructions">
        {CHIPS.map((m) => (
          <Chip key={m} m={m} />
        ))}
        <button
          type="button"
          onClick={() => useEditor.getState().setPaletteOpen(true)}
          className="rounded-control px-1.5 py-0.5 text-dense text-text-muted hover:text-text"
        >
          More… <kbd className="text-mono">Ctrl K</kbd>
        </button>
      </div>
      <span className="ml-auto flex items-center gap-1">
        <Button size="icon-sm" variant="ghost" aria-label="Undo" disabled={!canUndo} onClick={undo}>
          <Undo2 />
        </Button>
        <Button size="icon-sm" variant="ghost" aria-label="Redo" disabled={!canRedo} onClick={redo}>
          <Redo2 />
        </Button>
        <Button
          size="sm"
          variant={stView ? 'secondary' : 'outline'}
          aria-pressed={stView}
          onClick={() => setStView(!stView)}
          title="The Structured Text plcc generates from this routine (Ctrl+Shift+S)"
        >
          <Code2 /> ST view
        </Button>
      </span>
    </div>
  )
}

function QuickEntry() {
  const [text, setText] = useState('')
  const [error, setError] = useState<string | null>(null)
  return (
    <form
      className="sticky bottom-0 border-t border-line bg-bg/95 px-4 py-2 backdrop-blur"
      onSubmit={(e) => {
        e.preventDefault()
        if (!text.trim()) return
        try {
          addRungFromQuickEntry(text)
          setText('')
          setError(null)
        } catch (err) {
          setError(err instanceof RungTextError ? err.message : String(err))
        }
      }}
    >
      <label htmlFor="quick-entry" className="sr-only">
        Quick entry: type instructions and press Enter to add a rung
      </label>
      <input
        id="quick-entry"
        value={text}
        spellCheck={false}
        autoComplete="off"
        onChange={(e) => {
          setText(e.target.value)
          setError(null)
        }}
        onKeyDown={(e) => {
          if (e.key === 'Escape') {
            setText('')
            document.getElementById('rung-list')?.focus()
          }
        }}
        placeholder="Quick entry ( / ):  XIC Start XIO Stop OTE Motor   ↵ adds a rung"
        aria-invalid={!!error}
        className="h-8 w-full rounded-control border border-line bg-surface px-3 text-mono text-text outline-none placeholder:text-text-muted focus-visible:border-text-muted"
      />
      {error && (
        <p role="alert" className="pt-1 text-dense text-fault">
          {error}
        </p>
      )}
    </form>
  )
}

/** The target of the selection. */
function selectionTarget(): Target {
  const { selection } = useEditor.getState()
  if (selection.rungId == null) return { kind: 'none' }
  if (selection.elementId == null) return { kind: 'rung', rungId: selection.rungId }
  return { kind: 'element', rungId: selection.rungId, elementId: selection.elementId }
}

/** Inserting keys: a mnemonic each (the registry has the rest). */
const INSERT_KEYS: Record<string, string> = { c: 'XIC', C: 'XIO', o: 'OTE', l: 'OTL', L: 'OTL', u: 'OTU', U: 'OTU' }

function LadderList({ routine, program }: { routine: Routine; program: string }) {
  const listRef = useRef<HTMLDivElement>(null)
  const [width, setWidth] = useState(800)
  const [menuTarget, setMenuTarget] = useState<Target>({ kind: 'none' })
  const selection = useEditor((s) => s.selection)
  const editing = useEditor((s) => s.editing)
  const mode = useEditor((s) => s.mode)
  const tags = useEditor((s) => s.project.globals)
  const tagMap = useTagMap()
  const trace = useTrace(routine)
  const read = useLiveRead()
  const runtimeErrors = useLive((s) => s.errors)
  const problems = useProblems((s) => s.problems)
  const faultAt = useSimBuild((s) => s.faultAt)
  const fault = useLive((s) => s.fault)
  const live = mode !== 'offline' && trace !== null

  const marks = useMemo(() => {
    const out = new Map<Id, Mark>()
    for (const [id, p] of elementMarks(problems, program, routine.name)) {
      if (p.severity !== 'advice') out.set(id, { severity: p.severity, message: p.message })
    }
    if (mode === 'simulate') {
      for (const [id, message] of Object.entries(runtimeErrors)) {
        if (!out.has(Number(id))) out.set(Number(id), { severity: 'warning', message })
      }
      // The fault that stopped plcc's program, at its element.
      if (fault && faultAt && faultAt.program === program && faultAt.routine === routine.name) {
        const id = faultAt.element ?? faultAt.rung
        if (id !== undefined) out.set(id, { severity: 'error', message: `PLC fault: ${fault.message}` })
      }
    }
    return out
  }, [problems, program, routine.name, runtimeErrors, mode, fault, faultAt])

  const rungProblems = useMemo(() => {
    const by = new Map<Id, Problem[]>()
    for (const p of problems) {
      if (p.place.kind !== 'element' || p.place.program !== program || p.place.routine !== routine.name || p.place.rung === undefined) continue
      by.set(p.place.rung, [...(by.get(p.place.rung) ?? []), p])
    }
    return by
  }, [problems, program, routine.name])

  useLayoutEffect(() => {
    const el = listRef.current
    if (!el) return
    const ro = new ResizeObserver(([e]) => setWidth(Math.max(480, Math.floor(e.contentRect.width) - 32 - 48 - 10)))
    ro.observe(el)
    return () => ro.disconnect()
  }, [])

  // Keep the selected rung in view and announce the selection.
  useEffect(() => {
    if (selection.rungId == null) return
    const card = document.querySelector(`[data-rung-id="${selection.rungId}"]`)
    const target = selection.elementId != null ? card?.querySelector(`[data-element-id="${selection.elementId}"]`) : card
    target?.scrollIntoView({ block: 'nearest', inline: 'nearest' })
  }, [selection])

  const announce = useMemo(() => {
    const idx = routine.rungs.findIndex((r) => r.id === selection.rungId)
    const rung = routine.rungs[idx]
    if (!rung) return ''
    const el = selection.elementId != null ? findElement(rung.elements, selection.elementId) : undefined
    return `Rung ${idx}${el ? `, ${describeElement(el)}` : ''}`
  }, [selection, routine.rungs])

  /** Opens the context menu for the selection, at the selected element (Shift+F10, the Menu key). */
  const openMenuAtSelection = () => {
    const { rungId, elementId } = useEditor.getState().selection
    const card = rungId != null ? document.querySelector(`[data-rung-id="${rungId}"]`) : null
    const el = (elementId != null ? card?.querySelector(`[data-element-id="${elementId}"]`) : null) ?? card?.querySelector('[data-rung-gutter]') ?? listRef.current
    const r = el?.getBoundingClientRect()
    listRef.current?.dispatchEvent(
      new globalThis.MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: (r?.left ?? 0) + 12, clientY: (r?.bottom ?? 0) - 4 }),
    )
  }

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.target !== e.currentTarget) return
    const s = useEditor.getState()
    const k = e.key
    if ((e.shiftKey && k === 'F10') || k === 'ContextMenu') {
      e.preventDefault()
      openMenuAtSelection()
      return
    }
    const handled = () => e.preventDefault()
    if (k === 'ArrowLeft' || k === 'ArrowRight' || k === 'ArrowUp' || k === 'ArrowDown') {
      if (e.ctrlKey || e.metaKey) return
      handled()
      if (e.altKey) {
        if (k === 'ArrowUp' || k === 'ArrowDown') {
          if (s.selection.rungId != null) moveRung(s.selection.rungId, k === 'ArrowUp' ? -1 : 1)
        } else moveSelectedElement(k === 'ArrowLeft' ? -1 : 1)
        return
      }
      moveSelection(k === 'ArrowLeft' ? 'left' : k === 'ArrowRight' ? 'right' : k === 'ArrowUp' ? 'up' : 'down')
      return
    }
    if (e.altKey) return
    const mod = e.ctrlKey || e.metaKey
    if (!mod && INSERT_KEYS[k]) return handled(), insertInstruction(INSERT_KEYS[k])
    if (!mod && (k === 'b' || k === 'B')) {
      handled()
      s.setPaletteOpen(true)
      return
    }
    const cmd = commandForKey(keyName(e), selectionTarget())
    if (cmd) {
      handled()
      cmd.run(selectionTarget())
      return
    }
    if (mod) return
    switch (k) {
      case ' ': {
        // Space on a contact in Simulate/Online with no command (non-BOOL): nothing.
        return
      }
      case 't':
      case 'T': {
        if (s.selection.rungId == null) return
        handled()
        document.querySelector<HTMLInputElement>(`[data-rung-text="${s.selection.rungId}"]`)?.focus()
        return
      }
      case '/':
        handled()
        document.getElementById('quick-entry')?.focus()
        return
      case 'Escape':
        handled()
        s.select({ rungId: null, elementId: null })
        return
    }
  }

  /** Right click: select what is under the pointer, then show its menu. */
  const onContextMenu = (e: MouseEvent<HTMLDivElement>) => {
    const t = e.target as Element
    const elNode = t.closest?.('[data-element-id]')
    const rungNode = t.closest?.('[data-rung-id]')
    const s = useEditor.getState()
    if (rungNode) {
      const rungId = Number(rungNode.getAttribute('data-rung-id'))
      const elementId = elNode ? Number(elNode.getAttribute('data-element-id')) : null
      // A keyboard-opened menu (dispatched on the list) keeps the selection.
      if (e.currentTarget !== e.target || elNode) s.select({ rungId, elementId })
    }
    setMenuTarget(selectionTarget())
  }

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <div
            id="rung-list"
            ref={listRef}
            tabIndex={0}
            role="application"
            aria-roledescription="ladder editor"
            aria-label={`${routine.name} rungs. Arrow keys move, C contact, O coil, B box, P branch, N new rung, Enter edit, Delete remove, T rung text, / quick entry, Shift+F10 menu.`}
            onKeyDown={onKeyDown}
            onContextMenu={onContextMenu}
            onClick={(e) => {
              const t = e.target as HTMLElement
              if (!t.closest('input, textarea, button, [role=dialog]')) listRef.current?.focus({ preventScroll: true })
            }}
            className="min-h-0 flex-1 space-y-3 overflow-y-auto px-4 py-4 outline-none focus-visible:shadow-[inset_0_0_0_2px_var(--focus-ring)]"
          >
            {routine.rungs.map((r, i) => (
              <RungCard
                key={r.id}
                rung={r}
                index={i}
                count={routine.rungs.length}
                width={width}
                trace={trace}
                live={live}
                read={read}
                tags={tags}
                tagMap={tagMap}
                marks={marks}
                problems={rungProblems.get(r.id) ?? EMPTY}
                selected={selection.rungId === r.id}
                selectedElement={selection.rungId === r.id ? selection.elementId : null}
                editingElement={editing?.rungId === r.id ? editing.elementId : null}
                onToggleTag={live ? toggleTag : undefined}
              />
            ))}
            {routine.rungs.length === 0 && (
              <div className="rounded-lg border border-dashed border-line p-8 text-center text-text-muted">
                No rungs yet. Press <kbd className="text-mono text-text">N</kbd> for an empty rung, or type instructions below.
              </div>
            )}
            <button
              type="button"
              onClick={() => addRung()}
              className="flex w-full items-center justify-center gap-1.5 rounded-lg border border-dashed border-line py-2 text-dense text-text-muted hover:border-text-muted hover:text-text"
            >
              <Plus className="size-4" /> Add rung
            </button>
          </div>
        </ContextMenuTrigger>
        <CommandMenuContent entries={menuFor(menuTarget)} target={menuTarget} label={menuLabel(menuTarget)} />
      </ContextMenu>
      <QuickEntry />
      <div aria-live="polite" className="sr-only">
        {announce}
      </div>
    </div>
  )
}

function menuLabel(t: Target): string {
  if (t.kind === 'element') return 'Instruction'
  if (t.kind === 'rung') return 'Rung'
  return 'Ladder'
}

const EMPTY: Problem[] = []

function StRoutine({ routine, program }: { routine: Routine; program: string }) {
  const commit = useEditor((s) => s.commit)
  const code = stRoutineCode(routine)
  const [text, setText] = useDraft(code)
  const problems = useProblems((s) => s.problems)
  const here = problems.filter((p) => p.place.kind === 'element' && p.place.program === program && p.place.routine === routine.name)
  const area = useRef<HTMLTextAreaElement>(null)
  const save = (t: string) => t !== code && commit((p) => mapRoutine(p, program, routine.name, (r) => withStRoutineCode(r, t)))
  // Check while typing, not only on blur.
  useEffect(() => {
    if (text === code) return
    const id = setTimeout(() => save(text), 700)
    return () => clearTimeout(id)
  })
  const goTo = (p: Problem) => {
    const ta = area.current
    if (!ta || !p.span) return
    ta.focus()
    ta.setSelectionRange(p.span.utf16, Math.max(p.span.endUtf16, p.span.utf16 + 1))
  }
  const lines = text.split('\n').length
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2 p-4">
      <p className="text-dense text-text-muted">
        Structured Text routine (Logix ST). plcc checks it as you type; it runs in the simulator and on the device with the ladder.
      </p>
      <div className="flex min-h-0 flex-1 overflow-hidden rounded-lg border border-line bg-surface focus-within:border-text-muted">
        <div aria-hidden className="select-none overflow-hidden border-r border-line px-2 py-3 text-right text-mono text-text-muted">
          {Array.from({ length: lines }, (_, i) => {
            const bad = here.find((p) => p.span?.line === i + 1)
            return (
              <div key={i} className={bad ? (bad.severity === 'error' ? 'text-fault' : 'text-alarm') : ''} title={bad?.message}>
                {i + 1}
              </div>
            )
          })}
        </div>
        <label htmlFor="st-editor" className="sr-only">
          {routine.name} source
        </label>
        <textarea
          id="st-editor"
          ref={area}
          spellCheck={false}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onBlur={() => save(text)}
          aria-invalid={here.some((p) => p.severity === 'error')}
          className="min-h-0 flex-1 resize-none bg-transparent p-3 text-mono leading-[inherit] text-text outline-none"
        />
      </div>
      {here.length > 0 && (
        <ul aria-label="Problems in this routine" className="max-h-40 space-y-1 overflow-y-auto">
          {here.map((p, i) => (
            <li key={i}>
              <button
                type="button"
                onClick={() => goTo(p)}
                className={`w-full rounded-control border px-2 py-1 text-left text-dense ${p.severity === 'error' ? 'border-fault/50 text-fault' : 'border-alarm-border text-alarm'}`}
              >
                {p.span ? `${p.span.line}:${p.span.col} ` : ''}
                {p.message}
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}

export function RoutineView() {
  const routine = useEditor(currentRoutine)
  const view = useEditor((s) => s.view)
  const stView = useEditor((s) => s.stView)
  if (!routine || view.kind !== 'routine') return <div className="p-6 text-text-muted">Routine not found.</div>
  if (isStRoutine(routine)) return <StRoutine routine={routine} program={view.program} />
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <Toolbar />
      <div className="flex min-h-0 flex-1">
        <LadderList routine={routine} program={view.program} />
        {stView && <StView program={view.program} routine={routine.name} />}
      </div>
    </div>
  )
}
