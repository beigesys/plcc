// SPDX-License-Identifier: MPL-2.0
//
// Text input with tag autocomplete (an ARIA combobox). Suggests project tags,
// and members after `Timer.` for TIMER / COUNTER tags.

import { useId, useMemo, useState, type KeyboardEvent } from 'react'
import type { Tag } from '@/model'

const MEMBERS: Record<string, string[]> = {
  TIMER: ['DN', 'TT', 'EN', 'ACC', 'PRE'],
  COUNTER: ['DN', 'CU', 'CD', 'ACC', 'PRE', 'OV', 'UN'],
}

export interface Suggestion {
  value: string
  detail: string
}

export function suggest(tags: Tag[], text: string, limit = 8): Suggestion[] {
  const t = text.trim()
  const dot = t.indexOf('.')
  if (dot > 0) {
    const base = tags.find((x) => x.name.toLowerCase() === t.slice(0, dot).toLowerCase())
    const members = base ? MEMBERS[base.type.toUpperCase()] : undefined
    if (!base || !members) return []
    const m = t.slice(dot + 1).toUpperCase()
    return members.filter((x) => x.startsWith(m)).map((x) => ({ value: `${base.name}.${x}`, detail: x === 'ACC' || x === 'PRE' ? 'DINT' : 'BOOL' }))
  }
  const q = t.toLowerCase()
  const scored = tags
    .map((tag) => {
      const n = tag.name.toLowerCase()
      const score = !q ? 1 : n === q ? 0 : n.startsWith(q) ? 1 : n.includes(q) ? 2 : 9
      return { tag, score }
    })
    .filter((x) => x.score < 9)
    .sort((a, b) => a.score - b.score || a.tag.name.localeCompare(b.tag.name))
  return scored.slice(0, limit).map(({ tag }) => ({
    value: tag.name,
    detail: `${tag.type}${tag.address ? ` ${tag.address}` : ''}`,
  }))
}

export interface TagInputProps {
  id?: string
  label: string
  value: string
  tags: Tag[]
  placeholder?: string
  autoFocus?: boolean
  /** Type a new tag would get, for the hint. Undefined: operand is never a new tag. */
  newTagType?: string
  onChange(v: string): void
  /** Enter with no suggestion highlighted. */
  onSubmit?(): void
  onCancel?(): void
}

export function TagInput(props: TagInputProps) {
  const autoId = useId()
  const id = props.id ?? autoId
  const listId = `${id}-list`
  const [open, setOpen] = useState(false)
  const [active, setActive] = useState(-1)
  const items = useMemo(() => suggest(props.tags, props.value), [props.tags, props.value])
  const exact = props.tags.some((t) => t.name.toLowerCase() === props.value.trim().toLowerCase())
  const isName = /^[A-Za-z_][A-Za-z0-9_]*$/.test(props.value.trim())
  const showNew = !!props.newTagType && isName && !exact
  const visible = open && (items.length > 0 || showNew)

  const pick = (v: string) => {
    props.onChange(v)
    setActive(-1)
    setOpen(v.endsWith('.'))
  }

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      setOpen(true)
      setActive((a) => Math.min(a + 1, items.length - 1))
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      setActive((a) => Math.max(a - 1, -1))
    } else if (e.key === 'Enter') {
      e.preventDefault()
      if (visible && active >= 0 && items[active]) pick(items[active].value)
      else {
        setOpen(false)
        props.onSubmit?.()
      }
    } else if (e.key === 'Tab' && visible && active >= 0 && items[active]) {
      pick(items[active].value)
    } else if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      if (visible) setOpen(false)
      else props.onCancel?.()
    }
  }

  return (
    <div className="relative">
      <label htmlFor={id} className="mb-0.5 block text-[11px] text-text-muted">
        {props.label}
      </label>
      <input
        id={id}
        role="combobox"
        aria-expanded={visible}
        aria-controls={listId}
        aria-autocomplete="list"
        aria-activedescendant={active >= 0 ? `${listId}-${active}` : undefined}
        autoComplete="off"
        spellCheck={false}
        autoFocus={props.autoFocus}
        placeholder={props.placeholder}
        value={props.value}
        onChange={(e) => {
          props.onChange(e.target.value)
          setOpen(true)
          setActive(-1)
        }}
        onFocus={() => setOpen(true)}
        onBlur={() => setTimeout(() => setOpen(false), 120)}
        onKeyDown={onKeyDown}
        className="h-7 w-full rounded-control border border-line bg-bg px-2 text-mono text-text outline-none focus-visible:border-text-muted"
      />
      {visible && (
        <ul
          id={listId}
          role="listbox"
          aria-label={`${props.label} suggestions`}
          className="absolute top-full right-0 left-0 z-50 mt-1 max-h-56 overflow-auto rounded-control border border-line bg-surface-2 py-1 shadow-lg"
        >
          {items.map((s, i) => (
            <li
              key={s.value}
              id={`${listId}-${i}`}
              role="option"
              aria-selected={i === active}
              onMouseDown={(e) => {
                e.preventDefault()
                pick(s.value)
              }}
              className={`flex cursor-pointer items-center justify-between gap-2 px-2 py-1 text-mono ${i === active ? 'bg-[var(--select-bg)]' : ''}`}
            >
              <span className="truncate">{s.value}</span>
              <span className="shrink-0 text-[11px] text-text-muted">{s.detail}</span>
            </li>
          ))}
          {showNew && (
            <li role="presentation" className="px-2 py-1 text-[11px] text-text-muted">
              Enter creates {props.newTagType} tag <span className="text-mono text-text">{props.value.trim()}</span>
            </li>
          )}
        </ul>
      )}
    </div>
  )
}
