// SPDX-License-Identifier: MPL-2.0
import { memo, useMemo, useState } from 'react'
import { ArrowDown, ArrowUp, Copy, Trash2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import type { Trace } from '@/engine'
import { findElement, flatten, printRung, RungTextError, type Instruction, type Rung, type Tag } from '@/model'
import type { Scalar } from '@/runtime/messages'
import {
  deleteRung, duplicateRung, insertInstruction, moveRung, setRungComment, setRungText, updateInstruction,
} from '@/state/commands'
import { useEditor } from '@/state/editor'
import { Ladder } from './ladder/Ladder'
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
  errors: Record<string, string>
  selected: boolean
  selectedElement: string | null
  editingElement: string | null
  onToggleTag?(tag: string): void
}

function RungTextBar({ rung, index }: { rung: Rung; index: number }) {
  const printed = printRung(rung.body)
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
    </div>
  )
}

function RungCardImpl(p: RungCardProps) {
  const { rung, index } = p
  const select = useEditor((s) => s.select)
  const setEditing = useEditor((s) => s.setEditing)
  const layout = useMemo(() => layoutRung(rung.body, p.width), [rung.body, p.width])
  const [drop, setDrop] = useState<string | null>(null)
  const [comment, setComment] = useDraft(rung.comment)

  const els = useMemo(() => flatten(rung.body), [rung.body])
  const ids = new Set(els.map((e) => e.id))
  const hasFault = Object.keys(p.errors).some((id) => ids.has(id))
  const unassigned = els.some((e) => (e.type === 'contact' || e.type === 'coil') && !e.tag)

  const editingEl = p.editingElement ? findElement(rung.body, p.editingElement) : undefined
  const editingNode = editingEl ? allNodes(layout).find((n) => n.el.id === editingEl.id) : undefined

  const onActivate = (id: string) => {
    const el = findElement(rung.body, id)
    if (p.live && el?.type === 'contact' && el.tag && p.onToggleTag) {
      p.onToggleTag(el.tag)
      return
    }
    if (el && el.type !== 'parallel') setEditing({ rungId: rung.id, elementId: id })
  }

  return (
    <article
      aria-label={`Rung ${index}${rung.comment ? `: ${rung.comment}` : ''}`}
      data-rung-id={rung.id}
      className={`group/rung relative flex rounded-lg border bg-surface ${
        hasFault || unassigned ? 'border-alarm-border' : p.selected ? 'border-text-muted' : 'border-line'
      }`}
    >
      <div className="flex w-12 shrink-0 flex-col items-center border-r border-line py-2">
        <button
          type="button"
          onClick={() => select({ rungId: rung.id, elementId: null })}
          aria-label={`Select rung ${index}`}
          aria-pressed={p.selected && !p.selectedElement}
          className={`text-mono rounded-control px-1.5 py-0.5 ${p.selected ? 'bg-surface-2 text-text' : 'text-text-muted hover:text-text'}`}
        >
          {String(index).padStart(3, '0')}
        </button>
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
            value={comment}
            placeholder="Add a rung comment"
            onChange={(e) => setComment(e.target.value)}
            onBlur={() => comment !== rung.comment && setRungComment(rung.id, comment)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' || e.key === 'Escape') (e.target as HTMLInputElement).blur()
            }}
            className="min-w-0 flex-1 bg-transparent text-dense text-text-muted outline-none placeholder:text-text-muted/60 focus:text-text"
          />
          {hasFault && <span className="rounded-control bg-alarm-bg px-1.5 text-[11px] text-alarm">{Object.entries(p.errors).find(([id]) => ids.has(id))?.[1]}</span>}
          {!hasFault && unassigned && <span className="rounded-control bg-alarm-bg px-1.5 text-[11px] text-alarm">unassigned operand</span>}
        </div>
        <div className="relative">
        <div className="overflow-x-auto px-1 py-1">
          <Ladder
            body={rung.body}
            layout={layout}
            trace={p.trace}
            live={p.live}
            read={p.read}
            addressOf={(t) => p.tagMap.get(t.toLowerCase())?.address}
            errors={p.errors}
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
                onCommit={(next: Instruction) => {
                  updateInstruction(rung.id, editingEl.id, () => next)
                  setEditing(null)
                  focusList()
                }}
              />
            </div>
          )}
        </div>
        <RungTextBar rung={rung} index={index} />
      </div>
    </article>
  )
}

export function focusList() {
  requestAnimationFrame(() => document.getElementById('rung-list')?.focus())
}

export const RungCard = memo(RungCardImpl)
