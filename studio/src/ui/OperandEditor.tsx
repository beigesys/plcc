// SPDX-License-Identifier: MPL-2.0
//
// Inline editor for one instruction's operands, anchored under the element.
// Enter on the last field commits, Escape cancels.

import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { boxSpec, isUnassigned, type Element, type Tag } from '@/model'
import { changedType, changeTypeOptions, mnemonicOf } from '@/state/commands'
import { TagInput } from './TagInput'

export interface OperandEditorProps {
  el: Element
  tags: Tag[]
  onCommit(next: Element): void
  onCancel(): void
}

const shown = (v: string | undefined) => (isUnassigned(v) ? '' : (v ?? ''))
const stored = (v: string) => (v.trim() === '' ? '?' : v)

export function OperandEditor({ el, tags, onCommit, onCancel }: OperandEditorProps) {
  const [draft, setDraft] = useState<Element>(el)

  const commit = () => onCommit(draft)
  const keyCancel = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      onCancel()
    }
  }

  const current = mnemonicOf(draft)
  const swaps = changeTypeOptions(draft)
  const typeRow = current && swaps.length > 0 && (
    <div className="flex flex-wrap gap-1" role="radiogroup" aria-label="Instruction">
      {[current, ...swaps].sort().map((m) => (
        <button
          key={m}
          type="button"
          role="radio"
          aria-checked={current === m}
          onClick={() => setDraft((d) => changedType(d, m))}
          className={`rounded-control border px-1.5 py-0.5 text-mono ${current === m ? 'border-text-muted bg-surface-2 text-text' : 'border-line text-text-muted hover:text-text'}`}
        >
          {m}
        </button>
      ))}
    </div>
  )

  let body: React.ReactNode = null
  if (draft.type === 'contact' || draft.type === 'coil') {
    body = (
      <>
        {typeRow}
        <TagInput
          label="Tag"
          value={shown(draft.operand)}
          tags={tags}
          autoFocus
          newTagType="BOOL"
          placeholder="Tag or Tag.member"
          onChange={(v) => setDraft((d) => (d.type === 'contact' || d.type === 'coil' ? { ...d, operand: stored(v) } : d))}
          onSubmit={commit}
          onCancel={onCancel}
        />
      </>
    )
  } else if (draft.type === 'block') {
    const spec = boxSpec(draft)
    body = (
      <>
        {typeRow}
        {draft.pins.map((pin, i) => {
          const ps = spec.pins[i]
          return (
            <TagInput
              key={i}
              label={ps?.label ?? pin.name}
              value={shown(pin.value)}
              tags={tags}
              autoFocus={i === 0}
              newTagType={ps?.newTagType}
              placeholder={ps && ps.default !== '?' ? ps.default : 'Tag'}
              onChange={(v) =>
                setDraft((d) => (d.type === 'block' ? { ...d, pins: d.pins.map((x, j) => (j === i ? { ...x, value: stored(v) } : x)) } : d))
              }
              onSubmit={i === draft.pins.length - 1 ? commit : focusNext}
              onCancel={onCancel}
            />
          )
        })}
        {draft.pins.length === 0 && <p className="text-dense text-text-muted">{draft.name} has no operands.</p>}
      </>
    )
  } else if (draft.type === 'jump') {
    body = (
      <TagInput
        label="Label"
        value={shown(draft.label)}
        tags={[]}
        autoFocus
        placeholder="Label name"
        onChange={(v) => setDraft((d) => (d.type === 'jump' ? { ...d, label: stored(v) } : d))}
        onSubmit={commit}
        onCancel={onCancel}
      />
    )
  } else if (draft.type === 'st') {
    body = (
      <div>
        <label htmlFor="st-code" className="mb-0.5 block text-[11px] text-text-muted">
          Structured Text (Logix ST), run while the rung is true
        </label>
        <textarea
          id="st-code"
          autoFocus
          rows={5}
          value={draft.code}
          onChange={(e) => setDraft((d) => (d.type === 'st' ? { ...d, code: e.target.value } : d))}
          onKeyDown={(e) => {
            keyCancel(e)
            if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) commit()
          }}
          className="w-full rounded-control border border-line bg-bg p-2 text-mono text-text outline-none"
        />
      </div>
    )
  }

  return (
    <div
      role="dialog"
      aria-label="Edit instruction"
      className="w-64 space-y-2 rounded-lg border border-line bg-surface p-2.5 shadow-xl"
      onKeyDown={keyCancel}
      onClick={(e) => e.stopPropagation()}
    >
      {body}
      <div className="flex items-center justify-between pt-0.5">
        <span className="text-[11px] text-text-muted">Enter to apply · Esc to cancel</span>
        <div className="flex gap-1">
          <Button size="xs" variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
          <Button size="xs" onClick={commit}>
            Apply
          </Button>
        </div>
      </div>
    </div>
  )
}

function focusNext() {
  const inputs = Array.from(document.querySelectorAll<HTMLInputElement>('[aria-label="Edit instruction"] input'))
  const next = inputs[inputs.findIndex((x) => x === document.activeElement) + 1]
  next?.focus()
  next?.select()
}
