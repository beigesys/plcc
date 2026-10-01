// SPDX-License-Identifier: MPL-2.0
import { memo, useEffect, useMemo, useRef, useState } from 'react'
import { ArrowDown, ArrowUp, Copy, Trash2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import type { Trace } from '@/engine'
import { findElement, flatten, isUnassigned, printRung, RungTextError, type Element, type Id, type Rung, type Tag } from '@/model'
import type { Scalar } from '@/runtime/messages'
import { deleteRung, duplicateRung, insertInstruction, moveRung, setRungComment, setRungText, updateInstruction } from '@/state/commands'
import { useEditor } from '@/state/editor'
import type { Problem } from '@/state/problems'
import { Ladder, type Mark } from './ladder/Ladder'
import { allNodes, layoutRung } from './ladder/layout'
import { OperandEditor } from './OperandEditor'
import { useDraft } from './useDraft'

export interface RungCardProps {
  rung: Rung
  index: number
  count: number
  width: number
  trace: Trace | null
  live: boolean
  read?: (ref: string) => Scalar | undefined
  tags: Tag[]
  tagMap: Map<string, Tag>
  /** Problems by element id (the whole routine's). */
  marks: Map<Id, Mark>
  /** plcc's diagnostics for this rung. */
  problems: Problem[]
  selected: boolean
  selectedElement: Id | null
  editingElement: Id | null
  onToggleTag?(tag: string): void
}

function RungTextBar({ rung, index, problems }: { rung: Rung; index: number; problems: Problem[] }) {
  const printed = printRung(rung)
  const [text, setText] = useState(printed)
  const [focused, setFocused] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const shown = focused ? text : printed

  const apply = () => {
    try {
      setRungText(rung.id, text)
      setError(null)
      return true
    } catch (e) {
      setError(e instanceof RungTextError ? e.message : String(e))
      return false
    }
  }

  return (
    <div className="border-t border-line bg-rungtext">
      <label htmlFor={`rt-${rung.id}`} className="sr-only">
        Rung {index} text
      </label>
      <input
        id={`rt-${rung.id}`}
        data-rung-text={rung.id}
        spellCheck={false}
        autoComplete="off"
        value={shown}
        aria-invalid={!!error}
        aria-describedby={error ? `rt-err-${rung.id}` : undefined}
        onFocus={() => {
          setText(printed)
          setFocused(true)
        }}
        onBlur={() => {
          if (text !== printed) apply()
          setFocused(false)
        }}
        onChange={(e) => {
          setText(e.target.value)
          setError(null)
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            e.preventDefault()
            if (apply()) (e.target as HTMLInputElement).blur()
          } else if (e.key === 'Escape') {
            e.preventDefault()
            setText(printed)
            setError(null)
            ;(e.target as HTMLInputElement).blur()
          }
        }}
        className="block h-8 w-full bg-transparent px-3 text-mono text-text-muted outline-none focus:text-text"
      />
      {error && (
        <p id={`rt-err-${rung.id}`} role="alert" className="px-3 pb-1.5 text-dense text-fault">
          {error}
        </p>
      )}
      {problems.length > 0 && (
        <ul className="px-3 pb-1.5 text-dense" aria-label={`Rung ${index} problems`}>
          {problems.map((p, i) => (
            <li key={i} className={p.severity === 'error' ? 'text-fault' : p.severity === 'warning' ? 'text-alarm' : 'text-text-muted'}>
              {p.message}
              {p.help ? <span className="text-text-muted"> · {p.help}</span> : null}
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}

function RungCardImpl(p: RungCardProps) {
  const { rung, index } = p
  const select = useEditor((s) => s.select)
  const setEditing = useEditor((s) => s.setEditing)
  const focus = useEditor((s) => s.focus)
  const layout = useMemo(() => layoutRung(rung.elements, p.width), [rung.elements, p.width])
  const [drop, setDrop] = useState<Id | null>(null)
  const [comment, setComment] = useDraft(rung.comment ?? '')
  const commentRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    if (focus?.kind === 'comment' && focus.rungId === rung.id) {
      commentRef.current?.focus()
      useEditor.getState().setFocus(null)
    }
  }, [focus, rung.id])

  const els = useMemo(() => flatten(rung.elements), [rung.elements])
  const rungMarks = els.map((e) => p.marks.get(e.id)).filter((m): m is Mark => !!m)
  const worst = rungMarks.find((m) => m.severity === 'error') ?? rungMarks[0]
  const hasError = p.problems.some((x) => x.severity === 'error') || rungMarks.some((m) => m.severity === 'error')
  const unassigned = els.some((e) => (e.type === 'contact' || e.type === 'coil') && isUnassigned(e.operand))

  const editingEl = p.editingElement != null ? findElement(rung.elements, p.editingElement) : undefined
  const editingNode = editingEl ? allNodes(layout).find((n) => n.el.id === editingEl.id) : undefined

  const onActivate = (id: Id) => {
    const el = findElement(rung.elements, id)
    if (p.live && el?.type === 'contact' && !isUnassigned(el.operand) && p.onToggleTag) {
      p.onToggleTag(el.operand)
      return
    }
    if (el && el.type !== 'branch' && el.type !== 'return') setEditing({ rungId: rung.id, elementId: id })
  }

  return (
    <article
      aria-label={`Rung ${index}${rung.comment ? `: ${rung.comment}` : ''}`}
      data-rung-id={rung.id}
      data-has-error={hasError || undefined}
      className={`group/rung relative flex rounded-lg border bg-surface ${
        hasError ? 'border-fault/60' : worst || unassigned ? 'border-alarm-border' : p.selected ? 'border-text-muted' : 'border-line'
      }`}
    >
      <div data-rung-gutter className="flex w-12 shrink-0 flex-col items-center border-r border-line py-2">
        <button
          type="button"
          onClick={() => select({ rungId: rung.id, elementId: null })}
          aria-label={`Select rung ${index}`}
          aria-pressed={p.selected && p.selectedElement == null}
          className={`text-mono rounded-control px-1.5 py-0.5 ${p.selected ? 'bg-surface-2 text-text' : 'text-text-muted hover:text-text'}`}
        >
          {String(index).padStart(3, '0')}
        </button>
        {rung.label && <span className="mt-1 max-w-11 truncate text-[10px] text-text-muted" title={`Label ${rung.label}`}>{rung.label}</span>}
        <div className="mt-auto flex flex-col gap-0.5 opacity-0 transition-opacity group-focus-within/rung:opacity-100 group-hover/rung:opacity-100">
          <Button variant="ghost" size="icon-xs" aria-label={`Move rung ${index} up`} disabled={index === 0} onClick={() => moveRung(rung.id, -1)}>
            <ArrowUp />
          </Button>
          <Button variant="ghost" size="icon-xs" aria-label={`Move rung ${index} down`} disabled={index === p.count - 1} onClick={() => moveRung(rung.id, 1)}>
            <ArrowDown />
          </Button>
          <Button variant="ghost" size="icon-xs" aria-label={`Duplicate rung ${index}`} onClick={() => duplicateRung(rung.id)}>
            <Copy />
          </Button>
          <Button variant="ghost" size="icon-xs" aria-label={`Delete rung ${index}`} onClick={() => deleteRung(rung.id)}>
            <Trash2 />
          </Button>
        </div>
      </div>
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2 px-3 pt-2">
          <label htmlFor={`rc-${rung.id}`} className="sr-only">
            Rung {index} comment
          </label>
          <input
            id={`rc-${rung.id}`}
            ref={commentRef}
            value={comment}
            placeholder="Add a rung comment"
            onChange={(e) => setComment(e.target.value)}
            onBlur={() => comment !== (rung.comment ?? '') && setRungComment(rung.id, comment)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' || e.key === 'Escape') (e.target as HTMLInputElement).blur()
            }}
            className="min-w-0 flex-1 bg-transparent text-dense text-text-muted outline-none placeholder:text-text-muted/60 focus:text-text"
          />
          {worst && (
            <span className={`max-w-[50%] truncate rounded-control px-1.5 text-[11px] ${worst.severity === 'error' ? 'bg-fault/15 text-fault' : 'bg-alarm-bg text-alarm'}`} title={worst.message}>
              {worst.message}
            </span>
          )}
          {!worst && unassigned && <span className="rounded-control bg-alarm-bg px-1.5 text-[11px] text-alarm">unassigned operand</span>}
        </div>
        <div className="relative">
          <div className="overflow-x-auto px-1 py-1">
            <Ladder
              elements={rung.elements}
              layout={layout}
              trace={p.trace}
              live={p.live}
              read={p.read}
              addressOf={(t) => p.tagMap.get(t.toLowerCase())?.address}
              marks={p.marks}
              selectedId={p.selected ? p.selectedElement : null}
              dropTarget={drop}
              onSelect={(id) => {
                select({ rungId: rung.id, elementId: id })
                focusList()
              }}
              onActivate={onActivate}
              onDragTarget={setDrop}
              onDropInstruction={(afterId, m) => {
                setDrop(null)
                insertInstruction(m, { rungId: rung.id, afterId })
              }}
            />
          </div>
          {editingEl && editingNode && (
            <div
              className="absolute z-30"
              style={{ left: Math.min(editingNode.x, Math.max(0, layout.width - 270)), top: editingNode.y + editingNode.h + 8 }}
            >
              <OperandEditor
                el={editingEl}
                tags={p.tags}
                onCancel={() => {
                  setEditing(null)
                  focusList()
                }}
                onCommit={(next: Element) => {
                  updateInstruction(rung.id, editingEl.id, () => next)
                  setEditing(null)
                  focusList()
                }}
              />
            </div>
          )}
        </div>
        <RungTextBar rung={rung} index={index} problems={p.problems} />
      </div>
    </article>
  )
}

/** Returns keyboard focus to the rung list, unless an input (an open editor) has it. */
export function focusList() {
  requestAnimationFrame(() => {
    const a = document.activeElement as HTMLElement | null
    if (a && (a.tagName === 'INPUT' || a.tagName === 'TEXTAREA' || a.closest('[role=dialog]') || a.closest('[role=menu]'))) return
    document.getElementById('rung-list')?.focus({ preventScroll: true })
  })
}

export const RungCard = memo(RungCardImpl)
