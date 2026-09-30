// SPDX-License-Identifier: MPL-2.0
import { useMemo, useState } from 'react'
import { Plus, Search, Trash2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { addressTypeMismatch, defaultInitial, renameTag, tagReferenceCount, type Tag } from '@/model'
import { useEditor } from '@/state/editor'
import { formatValue, useLive } from '@/state/live'
import { useDraft } from './useDraft'

export const TAG_TYPES = [
  'BOOL', 'SINT', 'INT', 'DINT', 'LINT', 'USINT', 'UINT', 'UDINT', 'ULINT', 'BYTE', 'WORD', 'DWORD', 'LWORD', 'REAL', 'LREAL',
  'TIME', 'TIMER', 'COUNTER',
]

const NAME = /^[A-Za-z_][A-Za-z0-9_]*$/

function CellInput({
  value, label, mono, invalid, onCommit,
}: { value: string; label: string; mono?: boolean; invalid?: boolean; onCommit(v: string): boolean | void }) {
  const [v, setV] = useDraft(value)
  const commit = () => {
    if (v === value) return
    if (onCommit(v) === false) setV(value)
  }
  return (
    <input
      aria-label={label}
      value={v}
      spellCheck={false}
      aria-invalid={invalid}
      onChange={(e) => setV(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === 'Enter') (e.target as HTMLInputElement).blur()
        if (e.key === 'Escape') {
          setV(value)
          ;(e.target as HTMLInputElement).blur()
        }
      }}
      className={`h-7 w-full rounded-control border border-transparent bg-transparent px-1.5 outline-none hover:border-line focus:border-text-muted focus:bg-bg ${
        mono ? 'text-mono' : ''
      } ${invalid ? 'text-alarm' : ''}`}
    />
  )
}

export function TypeSelect({ value, label, onChange }: { value: string; label: string; onChange(v: string): void }) {
  const options = TAG_TYPES.includes(value.toUpperCase()) ? TAG_TYPES : [value, ...TAG_TYPES]
  return (
    <select
      aria-label={label}
      value={value.toUpperCase()}
      onChange={(e) => onChange(e.target.value)}
      className="h-7 w-full rounded-control border border-transparent bg-transparent px-1 text-mono outline-none hover:border-line focus:border-text-muted focus:bg-bg"
    >
      {options.map((t) => (
        <option key={t} value={t} className="bg-surface-2 text-text">
          {t}
        </option>
      ))}
    </select>
  )
}

export function updateTag(name: string, patch: Partial<Tag>) {
  const s = useEditor.getState()
  s.commit((p) => ({ ...p, tags: p.tags.map((t) => (t.name === name ? { ...t, ...patch } : t)) }))
}

export function tagWarnings(tags: Tag[]): Map<string, string> {
  const out = new Map<string, string>()
  const byAddr = new Map<string, string[]>()
  for (const t of tags) {
    if (!t.address) continue
    const w = addressTypeMismatch(t.address, t.type)
    if (w) out.set(t.name, w)
    const k = t.address.toUpperCase()
    byAddr.set(k, [...(byAddr.get(k) ?? []), t.name])
  }
  for (const [addr, names] of byAddr) {
    if (names.length > 1) for (const n of names) if (!out.has(n)) out.set(n, `${addr} is also bound to ${names.filter((x) => x !== n).join(', ')}`)
  }
  return out
}

export function TagsView() {
  const tags = useEditor((s) => s.project.tags)
  const mode = useEditor((s) => s.mode)
  const values = useLive((s) => s.values)
  const [filter, setFilter] = useState('')
  const [newName, setNewName] = useState('')
  const [newType, setNewType] = useState('BOOL')
  const [error, setError] = useState('')
  const warnings = useMemo(() => tagWarnings(tags), [tags])
  const shown = tags.filter(
    (t) => !filter || `${t.name} ${t.type} ${t.address ?? ''} ${t.comment}`.toLowerCase().includes(filter.toLowerCase()),
  )

  const add = () => {
    const n = newName.trim()
    if (!NAME.test(n)) return setError('Tag names use letters, digits and _, not starting with a digit')
    if (tags.some((t) => t.name.toLowerCase() === n.toLowerCase())) return setError(`${n} exists`)
    useEditor.getState().commit((p) => ({ ...p, tags: [...p.tags, { name: n, type: newType, initial: defaultInitial(newType), comment: '' }] }))
    setNewName('')
    setError('')
  }

  const rename = (from: string, to: string): boolean => {
    const s = useEditor.getState()
    if (!NAME.test(to)) {
      s.notify('Invalid tag name', 'alarm')
      return false
    }
    if (tags.some((t) => t.name.toLowerCase() === to.toLowerCase() && t.name !== from)) {
      s.notify(`${to} exists`, 'alarm')
      return false
    }
    s.commit((p) => renameTag(p, from, to))
    return true
  }

  const remove = (name: string) => {
    const s = useEditor.getState()
    const refs = tagReferenceCount(s.project, name)
    s.commit((p) => ({ ...p, tags: p.tags.filter((t) => t.name !== name) }))
    if (refs) s.notify(`Deleted ${name}; ${refs} instruction${refs > 1 ? 's' : ''} still use it (Ctrl+Z to undo)`, 'alarm')
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex flex-wrap items-end gap-3 border-b border-line bg-surface px-4 py-3">
        <div>
          <h1 className="text-base font-semibold">Tags</h1>
          <p className="text-dense text-text-muted">Controller tags. An address binds a tag to the process image (%I inputs, %Q outputs, %M memory).</p>
        </div>
        <form
          className="ml-auto flex items-end gap-2"
          onSubmit={(e) => {
            e.preventDefault()
            add()
          }}
        >
          <div>
            <label htmlFor="new-tag" className="block text-[11px] text-text-muted">
              New tag
            </label>
            <input
              id="new-tag"
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
              placeholder="Name"
              className="h-8 w-40 rounded-control border border-line bg-bg px-2 text-mono outline-none focus-visible:border-text-muted"
            />
          </div>
          <div className="w-28">
            <TypeSelect value={newType} label="New tag type" onChange={setNewType} />
          </div>
          <Button type="submit" size="sm">
            <Plus /> Add
          </Button>
        </form>
        <div className="relative">
          <Search className="absolute top-2 left-2 size-4 text-text-muted" aria-hidden />
          <input
            aria-label="Filter tags"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="Filter"
            className="h-8 w-44 rounded-control border border-line bg-bg pr-2 pl-7 outline-none focus-visible:border-text-muted"
          />
        </div>
        {error && <p className="w-full text-dense text-alarm">{error}</p>}
      </div>
      <div className="min-h-0 flex-1 overflow-auto px-4 py-2">
        <Table className="text-dense">
          <TableHeader>
            <TableRow>
              <TableHead className="w-44">Name</TableHead>
              <TableHead className="w-28">Type</TableHead>
              <TableHead className="w-24">Initial</TableHead>
              <TableHead className="w-28">Address</TableHead>
              <TableHead>Comment</TableHead>
              <TableHead className="w-32">{mode === 'offline' ? 'Value' : 'Live value'}</TableHead>
              <TableHead className="w-10">
                <span className="sr-only">Actions</span>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {shown.map((t) => {
              const w = warnings.get(t.name)
              return (
                <TableRow key={t.name}>
                  <TableCell>
                    <CellInput value={t.name} label={`${t.name} name`} mono onCommit={(v) => rename(t.name, v.trim())} />
                  </TableCell>
                  <TableCell>
                    <TypeSelect value={t.type} label={`${t.name} type`} onChange={(v) => updateTag(t.name, { type: v, initial: defaultInitial(v) })} />
                  </TableCell>
                  <TableCell>
                    <CellInput value={t.initial} label={`${t.name} initial value`} mono onCommit={(v) => updateTag(t.name, { initial: v.trim() })} />
                  </TableCell>
                  <TableCell>
                    <CellInput
                      value={t.address ?? ''}
                      label={`${t.name} address`}
                      mono
                      invalid={!!w}
                      onCommit={(v) => updateTag(t.name, { address: v.trim().toUpperCase() || undefined })}
                    />
                    {w && <p className="px-1.5 text-[11px] text-alarm">{w}</p>}
                  </TableCell>
                  <TableCell>
                    <CellInput value={t.comment} label={`${t.name} comment`} onCommit={(v) => updateTag(t.name, { comment: v })} />
                  </TableCell>
                  <TableCell className="text-mono">
                    <LiveValue tag={t} values={values} />
                  </TableCell>
                  <TableCell>
                    <Button variant="ghost" size="icon-xs" aria-label={`Delete ${t.name}`} onClick={() => remove(t.name)}>
                      <Trash2 />
                    </Button>
                  </TableCell>
                </TableRow>
              )
            })}
          </TableBody>
        </Table>
        {shown.length === 0 && <p className="p-6 text-center text-text-muted">No tags{filter ? ' match' : ''}.</p>}
      </div>
    </div>
  )
}

function LiveValue({ tag, values }: { tag: Tag; values: Record<string, unknown> }) {
  const v = values[tag.name.toLowerCase()] as Parameters<typeof formatValue>[0]
  if (v === undefined) return <span className="text-text-muted">—</span>
  if (typeof v === 'boolean') return <span className={v ? 'text-power-text' : 'text-text-muted'}>{v ? '1' : '0'}</span>
  return <span>{formatValue(v, tag.type)}</span>
}
