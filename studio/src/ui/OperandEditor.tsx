// SPDX-License-Identifier: MPL-2.0
//
// Inline editor for one instruction's operands, anchored under the element.
// Enter on the last field commits, Escape cancels.

import { useState } from 'react'
import { Button } from '@/components/ui/button'
import {
  BOX_SPECS, COIL_BY_MNEMONIC, COIL_MNEMONIC, CONTACT_BY_MNEMONIC, CONTACT_MNEMONIC, type Instruction, type Tag,
} from '@/model'
import { TagInput } from './TagInput'

export interface OperandEditorProps {
  el: Instruction
  tags: Tag[]
  onCommit(next: Instruction): void
  onCancel(): void
}

export function OperandEditor({ el, tags, onCommit, onCancel }: OperandEditorProps) {
  const [draft, setDraft] = useState<Instruction>(el)

  const commit = () => onCommit(draft)
  const keyCancel = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      onCancel()
    }
  }

  let body: React.ReactNode = null
  if (draft.type === 'contact' || draft.type === 'coil') {
    const options = draft.type === 'contact' ? Object.values(CONTACT_MNEMONIC) : Object.values(COIL_MNEMONIC)
    const current = draft.type === 'contact' ? CONTACT_MNEMONIC[draft.kind] : COIL_MNEMONIC[draft.kind]
    body = (
      <>
        <div className="flex flex-wrap gap-1" role="radiogroup" aria-label="Instruction">
          {options.map((m) => (
            <button
              key={m}
              type="button"
              role="radio"
              aria-checked={current === m}
              onClick={() =>
                setDraft((d) =>
                  d.type === 'contact' ? { ...d, kind: CONTACT_BY_MNEMONIC[m] } : d.type === 'coil' ? { ...d, kind: COIL_BY_MNEMONIC[m] } : d,
                )
              }
              className={`rounded-control border px-1.5 py-0.5 text-mono ${current === m ? 'border-text-muted bg-surface-2 text-text' : 'border-line text-text-muted hover:text-text'}`}
            >
              {m}
            </button>
          ))}
        </div>
        <TagInput
          label="Tag"
          value={draft.tag}
          tags={tags}
          autoFocus
          newTagType="BOOL"
          placeholder="Tag or Tag.member"
          onChange={(v) => setDraft((d) => (d.type === 'contact' || d.type === 'coil' ? { ...d, tag: v } : d))}
          onSubmit={commit}
          onCancel={onCancel}
        />
      </>
    )
  } else if (draft.type === 'box') {
    const spec = BOX_SPECS[draft.instr]
    body = spec.operands.map((o, i) => (
      <TagInput
        key={o.key}
        label={o.label}
        value={draft.operands[o.key] ?? ''}
        tags={tags}
        autoFocus={i === 0}
        newTagType={o.newTagType}
        placeholder={o.default === '?' ? 'Tag' : o.default}
        onChange={(v) => setDraft((d) => (d.type === 'box' ? { ...d, operands: { ...d.operands, [o.key]: v } } : d))}
        onSubmit={i === spec.operands.length - 1 ? commit : focusNext}
        onCancel={onCancel}
      />
    ))
  } else if (draft.type === 'st') {
    body = (
      <div>
        <label htmlFor="st-code" className="mb-0.5 block text-[11px] text-text-muted">
          Structured Text (not simulated in this phase)
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
