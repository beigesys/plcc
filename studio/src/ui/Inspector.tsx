// SPDX-License-Identifier: MPL-2.0
import { useState, type ReactNode } from 'react'
import { Button } from '@/components/ui/button'
import { Slider } from '@/components/ui/slider'
import { Switch } from '@/components/ui/switch'
import { getProfile, type IoPoint } from '@/devices/profiles'
import { BOX_SPECS, baseTag, parseAddress, type Instruction, type Tag } from '@/model'
import { selectedElement, useEditor } from '@/state/editor'
import { formatValue, simSend, useLive } from '@/state/live'
import { demoSetInput, isDemoDevice, onlineCanWrite, onlineForceBit, onlineWriteWord } from '@/state/online'
import { pointState, readImage } from './io'
import { StateDot } from './StateDot'
import { useDraft } from './useDraft'

function tagOf(el: Instruction | undefined): string | undefined {
  if (!el) return undefined
  if (el.type === 'contact' || el.type === 'coil') return baseTag(el.tag)
  if (el.type === 'box') {
    for (const o of BOX_SPECS[el.instr].operands) {
      const b = baseTag(el.operands[o.key] ?? '')
      if (b) return b
    }
  }
  return undefined
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex items-baseline justify-between gap-3 py-1">
      <dt className="text-text-muted">{label}</dt>
      <dd className="min-w-0 truncate text-right">{children}</dd>
    </div>
  )
}

function Panel({ title, children, aside }: { title: string; children: ReactNode; aside?: ReactNode }) {
  return (
    <section className="border-b border-line px-4 py-3" aria-label={title}>
      <div className="mb-2 flex items-center justify-between">
        <h2 className="text-[11px] font-semibold tracking-wider text-text-muted uppercase">{title}</h2>
        {aside}
      </div>
      {children}
    </section>
  )
}

function NumberWrite({ tag, onWrite }: { tag: Tag; onWrite(v: number): void }) {
  const [v, setV] = useState('')
  return (
    <form
      className="mt-2 flex gap-1.5"
      onSubmit={(e) => {
        e.preventDefault()
        const n = Number(v)
        if (v.trim() !== '' && Number.isFinite(n)) onWrite(n)
      }}
    >
      <label htmlFor="write-value" className="sr-only">
        New value for {tag.name}
      </label>
      <input
        id="write-value"
        inputMode="decimal"
        value={v}
        onChange={(e) => setV(e.target.value)}
        placeholder="Value"
        className="h-8 min-w-0 flex-1 rounded-control border border-line bg-bg px-2 text-mono outline-none focus-visible:border-text-muted"
      />
      <Button type="submit" size="sm" variant="outline">
        Write
      </Button>
    </form>
  )
}

function TagPanel({ tag }: { tag: Tag }) {
  const mode = useEditor((s) => s.mode)
  const notify = useEditor((s) => s.notify)
  const device = useEditor((s) => s.project.devices[0])
  const values = useLive((s) => s.values)
  const forces = useLive((s) => s.forces)
  const mKnown = useLive((s) => s.online.mKnown)
  const profile = getProfile(device?.profile ?? 'simulator')
  const point = tag.address ? profile.points.find((p) => p.address.toUpperCase() === tag.address?.toUpperCase()) : undefined
  const v = values[tag.name.toLowerCase()]
  const forced = forces[tag.name.toLowerCase()]
  const isBool = tag.type.toUpperCase() === 'BOOL'
  const a = tag.address ? parseAddress(tag.address) : undefined

  let forceNote = ''
  let canForce = false
  let canWrite = false
  if (mode === 'offline') forceNote = 'Force and write work in Simulate and Online.'
  else if (mode === 'simulate') {
    canForce = isBool || /INT|REAL|WORD|BYTE/i.test(tag.type)
    canWrite = !isBool && canForce
  } else {
    canForce = isBool && a?.area === 'M' && a.size === 'X' && (mKnown === 0 || onlineCanWrite(tag.address ?? ''))
    canWrite = a?.area === 'M' && a.size === 'W'
    if (!canForce && isBool) {
      forceNote =
        a?.area === 'M'
          ? `The console reports only the first ${mKnown || 8} %M bytes, so ${tag.address} cannot be read-modify-written.`
          : 'Online, only %M bits can be forced (via mw on the containing %MW word).'
    }
  }

  const force = (on: boolean | null) => {
    if (mode === 'simulate') simSend({ type: 'force', tag: tag.name, value: on === null ? null : isBool ? on : on ? 1 : 0 })
    else if (mode === 'online' && tag.address && on !== null)
      onlineForceBit(tag.address, on).catch((e: unknown) => notify(e instanceof Error ? e.message : String(e), 'fault'))
  }

  const write = (n: number) => {
    if (mode === 'simulate') simSend({ type: 'write', ref: tag.name, value: n })
    else if (mode === 'online' && a?.area === 'M' && a.size === 'W')
      onlineWriteWord(a.index, n).catch((e: unknown) => notify(e instanceof Error ? e.message : String(e), 'fault'))
  }

  return (
    <Panel
      title="Selected tag"
      aside={forced !== undefined && <span className="rounded-control bg-alarm-bg px-1.5 text-[11px] text-alarm">Forced</span>}
    >
      <div className="mb-1 flex items-center gap-2">
        {isBool && <StateDot state={v === undefined ? 'unknown' : v ? 'power' : 'idle'} />}
        <span className="text-mono text-[15px] font-medium">{tag.name}</span>
      </div>
      <dl className="text-dense">
        <Row label="Type">
          <span className="text-mono">{tag.type}</span>
        </Row>
        <Row label="Value">
          <span className={`text-mono ${isBool && v ? 'text-power-text' : ''}`}>{formatValue(v, tag.type)}</span>
        </Row>
        <Row label="Address">
          <span className="text-mono">{tag.address ?? '—'}</span>
        </Row>
        <Row label="Terminal">{point ? `${point.terminal} · ${point.label}` : '—'}</Row>
        {tag.comment && <Row label="Comment">{tag.comment}</Row>}
      </dl>
      {isBool && (
        <div className="mt-2 grid grid-cols-2 gap-1.5">
          <Button size="sm" variant="outline" disabled={!canForce} onClick={() => force(true)}>
            Force ON
          </Button>
          <Button size="sm" variant="outline" disabled={!canForce} onClick={() => force(false)}>
            Force OFF
          </Button>
          {mode === 'simulate' && forced !== undefined && (
            <Button size="sm" variant="ghost" className="col-span-2" onClick={() => force(null)}>
              Remove force
            </Button>
          )}
        </div>
      )}
      {!isBool && canWrite && <NumberWrite tag={tag} onWrite={write} />}
      {!isBool && mode === 'simulate' && canForce && forced !== undefined && (
        <Button size="sm" variant="ghost" className="mt-1.5 w-full" onClick={() => force(null)}>
          Remove force
        </Button>
      )}
      {mode === 'online' && isBool && canForce && (
        <p className="mt-1.5 text-[11px] text-text-muted">Online force writes the bit once (mw on %MW{a ? Math.floor(a.byte / 2) : 0}); the program may overwrite it.</p>
      )}
      {forceNote && <p className="mt-1.5 text-[11px] text-text-muted">{forceNote}</p>}
    </Panel>
  )
}

function IoPointRow({ p }: { p: IoPoint }) {
  const mode = useEditor((s) => s.mode)
  const image = useLive((s) => s.image)
  const onlineState = useLive((s) => s.online.state)
  const tags = useEditor((s) => s.project.tags)
  const bound = tags.find((t) => t.address?.toUpperCase() === p.address.toUpperCase())
  const { state } = pointState(p, image)
  const raw = readImage(image, p.address, p.type)
  const demo = mode === 'online' && onlineState !== 'disconnected' && isDemoDevice()
  const drive = p.dir === 'in' && (mode === 'simulate' || demo)
  const set = (value: boolean | number) => {
    if (mode === 'simulate') simSend({ type: 'image', address: p.address, value })
    else if (demo) demoSetInput(p.address, value)
  }
  const [slider, setSlider] = useDraft(typeof raw === 'number' ? raw : 0)

  return (
    <li className="py-1.5">
      <div className="flex items-center gap-2">
        <StateDot state={image ? state : 'unknown'} />
        <span className="w-10 text-mono text-text-muted">{p.terminal}</span>
        <span className="min-w-0 flex-1 truncate">{bound ? <span className="text-mono">{bound.name}</span> : <span className="text-text-muted">{p.label}</span>}</span>
        {drive && p.kind === 'digital' && (
          <Switch
            aria-label={`${p.terminal} ${p.label}`}
            checked={raw === true}
            onCheckedChange={(c) => set(c)}
            className="data-checked:bg-power"
          />
        )}
        {!(drive && p.kind === 'digital') && (
          <span className="w-12 text-right text-mono text-text-muted">{raw === undefined ? '—' : typeof raw === 'boolean' ? (raw ? '1' : '0') : raw}</span>
        )}
      </div>
      {drive && p.kind === 'analog' && (
        <div className="mt-1.5 flex items-center gap-2 pl-[18px]">
          <Slider
            aria-label={`${p.terminal} analog value`}
            min={p.range?.[0] ?? 0}
            max={p.range?.[1] ?? 4095}
            step={1}
            value={[slider]}
            onValueChange={([v]) => {
              setSlider(v)
              set(v)
            }}
          />
        </div>
      )}
    </li>
  )
}

function DevicePanel() {
  const mode = useEditor((s) => s.mode)
  const device = useEditor((s) => s.project.devices[0])
  const profile = getProfile(device?.profile ?? 'simulator')
  const [showAll, setShowAll] = useState(false)
  const points = profile.points.filter((p) => showAll || p.kind !== 'register')
  const title = mode === 'simulate' ? 'Virtual I/O' : `${device?.name ?? profile.name} I/O`
  return (
    <Panel
      title={title}
      aside={
        <button type="button" className="text-[11px] text-text-muted hover:text-text" onClick={() => setShowAll(!showAll)}>
          {showAll ? 'Hide registers' : 'Show registers'}
        </button>
      }
    >
      {mode === 'simulate' && <p className="mb-1 text-[11px] text-text-muted">Preview simulator: toggle inputs and drag analog values.</p>}
      {mode === 'offline' && <p className="mb-1 text-[11px] text-text-muted">States appear in Simulate and Online.</p>}
      <ul className="text-dense">
        {points.map((p) => (
          <IoPointRow key={p.id} p={p} />
        ))}
      </ul>
    </Panel>
  )
}

function ElementPanel({ el }: { el: Instruction }) {
  const errors = useLive((s) => s.errors)
  const mode = useEditor((s) => s.mode)
  const err = mode === 'simulate' ? errors[el.id] : undefined
  if (!err) return null
  return (
    <Panel title="Instruction">
      <p className="rounded-control border border-alarm-border bg-alarm-bg px-2 py-1 text-dense text-alarm">{err}</p>
    </Panel>
  )
}

export function Inspector() {
  const el = useEditor(selectedElement)
  const tags = useEditor((s) => s.project.tags)
  const name = tagOf(el)
  const tag = name ? tags.find((t) => t.name.toLowerCase() === name.toLowerCase()) : undefined
  return (
    <aside aria-label="Inspector" className="min-h-0 overflow-y-auto border-l border-line bg-nav">
      {tag ? (
        <TagPanel tag={tag} />
      ) : (
        <Panel title="Selected tag">
          <p className="text-dense text-text-muted">
            {el ? 'This instruction has no tag yet. Press Enter to edit it.' : 'Select an instruction to see its tag.'}
          </p>
        </Panel>
      )}
      {el && <ElementPanel el={el} />}
      <DevicePanel />
    </aside>
  )
}
