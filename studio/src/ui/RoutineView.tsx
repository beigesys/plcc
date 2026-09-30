// SPDX-License-Identifier: MPL-2.0
import { useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react'
import { Code2, GitBranch, Plus, Redo2, Undo2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { findElement, RungTextError, type Routine } from '@/model'
import {
  addRung, addRungFromQuickEntry, deleteSelection, insertInstruction, moveRung, moveSelectedElement, moveSelection,
  wrapSelectionInBranch,
} from '@/state/commands'
import { currentRoutine, useEditor } from '@/state/editor'
import { useLiveRead, useTagMap, useTrace } from '@/state/hooks'
import { simSend, useLive } from '@/state/live'
import { onlineCanWrite, onlineForceBit } from '@/state/online'
import { PALETTE_MIME, describeElement } from './ladder/Ladder'
import { RungCard } from './RungCard'
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
        <Tooltip>
          <TooltipTrigger asChild>
            <span tabIndex={0} aria-label="ST view (needs plcc serve, phase 2)">
              <Button size="sm" variant="outline" disabled className="pointer-events-none">
                <Code2 /> ST view
              </Button>
            </span>
          </TooltipTrigger>
          <TooltipContent>needs plcc serve — phase 2</TooltipContent>
        </Tooltip>
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

/** Toggles a BOOL tag in Simulate (write) or Online (%M force). */
export function toggleTag(tag: string) {
  const s = useEditor.getState()
  const t = s.project.tags.find((x) => x.name.toLowerCase() === tag.toLowerCase())
  const cur = useLive.getState().values[tag.toLowerCase()]
  if (s.mode === 'simulate') {
    simSend({ type: 'write', ref: tag, value: !cur })
  } else if (s.mode === 'online') {
    if (!t?.address || !onlineCanWrite(t.address)) {
      s.notify(`${tag} is not a %M bit the device console can write`, 'alarm')
      return
    }
    onlineForceBit(t.address, !cur).catch((e: unknown) => s.notify(String(e), 'fault'))
  }
}

function LadderList({ routine }: { routine: Routine }) {
  const listRef = useRef<HTMLDivElement>(null)
  const [width, setWidth] = useState(800)
  const selection = useEditor((s) => s.selection)
  const editing = useEditor((s) => s.editing)
  const mode = useEditor((s) => s.mode)
  const tags = useEditor((s) => s.project.tags)
  const tagMap = useTagMap()
  const trace = useTrace(routine)
  const read = useLiveRead()
  const errors = useLive((s) => s.errors)
  const live = mode !== 'offline' && trace !== null

  useLayoutEffect(() => {
    const el = listRef.current
    if (!el) return
    const ro = new ResizeObserver(([e]) => setWidth(Math.max(480, Math.floor(e.contentRect.width) - 32 - 48 - 10)))
    ro.observe(el)
    return () => ro.disconnect()
  }, [])

  // Keep the selected rung in view and announce the selection.
  useEffect(() => {
    if (!selection.rungId) return
    const card = document.querySelector(`[data-rung-id="${selection.rungId}"]`)
    const target = selection.elementId ? card?.querySelector(`[data-element-id="${selection.elementId}"]`) : card
    target?.scrollIntoView({ block: 'nearest', inline: 'nearest' })
  }, [selection])

  const announce = useMemo(() => {
    const idx = routine.rungs.findIndex((r) => r.id === selection.rungId)
    const rung = routine.rungs[idx]
    if (!rung) return ''
    const el = selection.elementId ? findElement(rung.body, selection.elementId) : undefined
    return `Rung ${idx}${el ? `, ${describeElement(el)}` : ''}`
  }, [selection, routine.rungs])

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.target !== e.currentTarget) return
    const s = useEditor.getState()
    const k = e.key
    const mod = e.ctrlKey || e.metaKey
    if (mod) return
    const handled = () => e.preventDefault()
    if (k === 'ArrowLeft' || k === 'ArrowRight' || k === 'ArrowUp' || k === 'ArrowDown') {
      handled()
      if (e.altKey) {
        if (k === 'ArrowUp' || k === 'ArrowDown') {
          if (s.selection.rungId) moveRung(s.selection.rungId, k === 'ArrowUp' ? -1 : 1)
        } else moveSelectedElement(k === 'ArrowLeft' ? -1 : 1)
        return
      }
      moveSelection(k === 'ArrowLeft' ? 'left' : k === 'ArrowRight' ? 'right' : k === 'ArrowUp' ? 'up' : 'down')
      return
    }
    if (e.altKey) return
    switch (k) {
      case 'c':
        return handled(), insertInstruction('XIC')
      case 'C':
        return handled(), insertInstruction('XIO')
      case 'o':
        return handled(), insertInstruction('OTE')
      case 'l':
      case 'L':
        return handled(), insertInstruction('OTL')
      case 'u':
      case 'U':
        return handled(), insertInstruction('OTU')
      case 'b':
      case 'B':
        handled()
        s.setPaletteOpen(true)
        return
      case 'p':
      case 'P':
        return handled(), wrapSelectionInBranch()
      case 'n':
      case 'N':
        return handled(), addRung()
      case 'Delete':
      case 'Backspace':
        return handled(), deleteSelection()
      case 'Enter': {
        handled()
        const { rungId, elementId } = s.selection
        if (rungId && elementId) {
          const rung = routine.rungs.find((r) => r.id === rungId)
          const el = rung && findElement(rung.body, elementId)
          if (el && el.type !== 'parallel') s.setEditing({ rungId, elementId })
        }
        return
      }
      case ' ': {
        const { rungId, elementId } = s.selection
        const rung = routine.rungs.find((r) => r.id === rungId)
        const el = rung && elementId ? findElement(rung.body, elementId) : undefined
        if (live && el && (el.type === 'contact' || el.type === 'coil') && el.tag) {
          handled()
          toggleTag(el.tag)
        }
        return
      }
      case 't':
      case 'T': {
        if (!s.selection.rungId) return
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

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div
        id="rung-list"
        ref={listRef}
        tabIndex={0}
        role="application"
        aria-roledescription="ladder editor"
        aria-label={`${routine.name} rungs. Arrow keys move, C contact, O coil, B box, P branch, N new rung, Enter edit, Delete remove, T rung text, / quick entry.`}
        onKeyDown={onKeyDown}
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
            errors={mode === 'simulate' ? errors : EMPTY}
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
      <QuickEntry />
      <div aria-live="polite" className="sr-only">
        {announce}
      </div>
    </div>
  )
}

const EMPTY: Record<string, string> = {}

function StRoutine({ routine }: { routine: Routine }) {
  const commit = useEditor((s) => s.commit)
  const view = useEditor((s) => s.view)
  const [text, setText] = useDraft(routine.st ?? '')
  if (view.kind !== 'routine') return null
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2 p-4">
      <p className="text-dense text-text-muted">
        Structured Text routine. Editing works and is saved; checking, compiling and simulating ST needs plcc serve (phase 2).
      </p>
      <label htmlFor="st-editor" className="sr-only">
        {routine.name} source
      </label>
      <textarea
        id="st-editor"
        spellCheck={false}
        value={text}
        onChange={(e) => setText(e.target.value)}
        onBlur={() =>
          text !== (routine.st ?? '') &&
          commit((p) => ({
            ...p,
            programs: p.programs.map((pr) =>
              pr.name !== view.program ? pr : { ...pr, routines: pr.routines.map((r) => (r.name === routine.name ? { ...r, st: text } : r)) },
            ),
          }))
        }
        className="min-h-0 flex-1 resize-none rounded-lg border border-line bg-surface p-3 text-mono text-text outline-none focus-visible:border-text-muted"
      />
    </div>
  )
}

export function RoutineView() {
  const routine = useEditor(currentRoutine)
  if (!routine) return <div className="p-6 text-text-muted">Routine not found.</div>
  if (routine.kind === 'st') return <StRoutine routine={routine} />
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <Toolbar />
      <LadderList routine={routine} />
    </div>
  )
}
