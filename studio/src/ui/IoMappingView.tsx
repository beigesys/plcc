// SPDX-License-Identifier: MPL-2.0
import { useEffect, useMemo, useState } from 'react'
import { AlertTriangle, ArrowUpCircle } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { CATALOG } from '@/devices/catalog'
import type { Device, IoPoint } from '@/devices/manifest'
import { catalogUpdates, changeDevice, resolveDevice, type ManifestUpdate, type RemapReport } from '@/devices/project'
import { newTag, parseAddress, type Tag } from '@/model'
import { useEditor } from '@/state/editor'
import { useLive, type LiveState } from '@/state/live'
import { pointMismatch, pointState, tagsAt } from './io'
import { StateDot } from './StateDot'
import { ManifestUpdateDialog, RemapReportView } from './DeviceDialogs'

const NEW = '__new__'
const NONE = ''

function bind(point: IoPoint, tagName: string) {
  const s = useEditor.getState()
  s.commit((p) => ({
    ...p,
    globals: p.globals.map((t) => {
      if (t.address?.toUpperCase() === point.address.toUpperCase() && t.name !== tagName) return { ...t, address: undefined }
      if (t.name === tagName) return { ...t, address: point.address }
      return t
    }),
  }))
}

function createAndBind(point: IoPoint, name: string): string | undefined {
  const s = useEditor.getState()
  if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name)) return 'Invalid tag name'
  if (s.project.globals.some((t) => t.name.toLowerCase() === name.toLowerCase())) return `${name} exists; pick it from the list`
  s.commit((p) => ({
    ...p,
    globals: [
      ...p.globals.map((t) => (t.address?.toUpperCase() === point.address.toUpperCase() ? { ...t, address: undefined } : t)),
      newTag(name, point.type, { address: point.address, comment: point.label }),
    ],
  }))
  return undefined
}


function Terminal({ point, image, onPick }: { point: IoPoint; image: LiveState['image']; onPick(): void }) {
  const { state } = pointState(point, image)
  return (
    <button
      type="button"
      onClick={onPick}
      className="flex w-12 flex-col items-center gap-1 rounded-control border border-line bg-surface-2 py-1.5 hover:border-text-muted"
      aria-label={`${point.terminal} ${point.label}`}
    >
      <StateDot state={image ? state : 'unknown'} />
      <span className="text-mono text-[11px]">{point.terminal}</span>
    </button>
  )
}

function DeviceFace({ profile, image }: { profile: Device; image: LiveState['image'] }) {
  const ins = profile.io.filter((p) => p.dir === 'in' && p.kind === 'digital')
  const outs = profile.io.filter((p) => p.dir === 'out')
  const pick = (p: IoPoint) => document.getElementById(`io-${p.id}`)?.focus()
  return (
    <figure className="rounded-xl border border-line bg-surface p-4" aria-label={`${profile.device.name} terminals`}>
      <div className="flex flex-wrap gap-1.5">
        {ins.map((p) => (
          <Terminal key={p.id} point={p} image={image} onPick={() => pick(p)} />
        ))}
      </div>
      <div className="my-3 flex items-center gap-3 rounded-lg border border-dashed border-line px-3 py-4">
        <span className="font-semibold">{profile.device.name}</span>
        <span className="text-dense text-text-muted">{profile.device.description}</span>
      </div>
      <div className="flex flex-wrap gap-1.5">
        {outs.map((p) => (
          <Terminal key={p.id} point={p} image={image} onPick={() => pick(p)} />
        ))}
      </div>
    </figure>
  )
}

function PointRow({ point, tags, image }: { point: IoPoint; tags: Tag[]; image: LiveState['image'] }) {
  const bound = tagsAt(tags, point.address)
  const current = bound[0]
  const [creating, setCreating] = useState(false)
  const [name, setName] = useState('')
  const [err, setErr] = useState('')
  const { state, value } = pointState(point, image)
  const warnings = [
    ...bound.map((t) => pointMismatch(point, t)).filter((x): x is string => !!x),
    ...(bound.length > 1 ? [`${bound.length} tags share ${point.address}: ${bound.map((t) => t.name).join(', ')}`] : []),
  ]
  const compatible = (t: Tag) => !pointMismatch(point, t)
  const sorted = [...tags].sort((a, b) => Number(compatible(b)) - Number(compatible(a)) || a.name.localeCompare(b.name))
  return (
    <TableRow className={warnings.length ? 'bg-alarm-bg/40' : undefined}>
      <TableCell className="w-8">
        <StateDot state={image ? state : 'unknown'} />
      </TableCell>
      <TableCell className="text-mono">{point.terminal}</TableCell>
      <TableCell>{point.label}</TableCell>
      <TableCell className="text-mono">{point.address}</TableCell>
      <TableCell className="text-mono text-text-muted">
        {point.type}
        {point.range && ` ${point.range[0]}..${point.range[1]}`}
        {point.eng && point.units && ` = ${point.eng[0]}..${point.eng[1]} ${point.units}`}
      </TableCell>
      <TableCell className="w-64">
        {creating ? (
          <form
            className="flex gap-1"
            onSubmit={(e) => {
              e.preventDefault()
              const r = createAndBind(point, name.trim())
              if (r) setErr(r)
              else {
                setCreating(false)
                setName('')
                setErr('')
              }
            }}
          >
            <input
              autoFocus
              aria-label={`New tag for ${point.terminal}`}
              value={name}
              onChange={(e) => setName(e.target.value)}
              onKeyDown={(e) => e.key === 'Escape' && setCreating(false)}
              placeholder={`New ${point.type} tag`}
              className="h-7 min-w-0 flex-1 rounded-control border border-line bg-bg px-2 text-mono outline-none focus-visible:border-text-muted"
            />
            {err && <span className="text-[11px] text-alarm">{err}</span>}
          </form>
        ) : (
          <select
            id={`io-${point.id}`}
            aria-label={`Tag bound to ${point.terminal}`}
            value={current?.name ?? NONE}
            onChange={(e) => {
              const v = e.target.value
              if (v === NEW) setCreating(true)
              else if (v === NONE) {
                useEditor.getState().commit((p) => ({
                  ...p,
                  globals: p.globals.map((t) => (t.address?.toUpperCase() === point.address.toUpperCase() ? { ...t, address: undefined } : t)),
                }))
              } else bind(point, v)
            }}
            className="h-7 w-full rounded-control border border-line bg-bg px-1.5 text-mono outline-none focus-visible:border-text-muted"
          >
            <option value={NONE} className="bg-surface-2">
              — unbound —
            </option>
            {sorted.map((t) => (
              <option key={t.name} value={t.name} className="bg-surface-2">
                {t.name} ({t.data_type}
                {t.address && t.address.toUpperCase() !== point.address.toUpperCase() ? ` at ${t.address}` : ''}
                {compatible(t) ? '' : ', mismatch'})
              </option>
            ))}
            <option value={NEW} className="bg-surface-2">
              + New tag…
            </option>
          </select>
        )}
        {warnings.map((w) => (
          <p key={w} className="mt-0.5 flex items-center gap-1 text-[11px] text-alarm">
            <AlertTriangle className="size-3" aria-hidden /> {w}
          </p>
        ))}
      </TableCell>
      <TableCell className="w-20 text-right text-mono">
        {value === undefined ? <span className="text-text-muted">—</span> : typeof value === 'boolean' ? (value ? '1' : '0') : value}
      </TableCell>
    </TableRow>
  )
}

export function IoMappingView({ device }: { device: string }) {
  const project = useEditor((s) => s.project)
  const image = useLive((s) => s.image)
  const [report, setReport] = useState<RemapReport | null>(null)
  const [reviewing, setReviewing] = useState<ManifestUpdate | null>(null)
  const resolved = useMemo(() => resolveDevice(project, device), [project, device])
  const dev = resolved.ref
  const profile = resolved.device
  const update = useMemo(() => catalogUpdates(project).find((u) => u.path === dev?.manifest), [project, dev])
  const groups = useMemo(() => {
    const m = new Map<string, IoPoint[]>()
    for (const p of profile.io) m.set(p.group, [...(m.get(p.group) ?? []), p])
    return [...m.entries()]
  }, [profile])
  const known = new Set(profile.io.map((p) => p.address.toUpperCase()))
  const img = profile.target.image
  const stray = project.globals.filter((t) => {
    if (!t.address) return false
    const a = parseAddress(t.address)
    if (!a) return true
    if (a.area === 'M') return a.byte + a.width > img.M
    return !known.has(t.address.toUpperCase())
  })

  const setProfile = (id: string) => {
    const entry = CATALOG.find((e) => e.id === id)
    if (!entry || !dev) return
    const s = useEditor.getState()
    try {
      const r = changeDevice(s.project, dev.name, entry.text)
      s.commit(() => r.project)
      setReport(r.report)
      s.notify(
        `${dev.name} is now ${entry.device.device.name}: ${r.report.moved.length} tag(s) remapped${r.report.warnings.length ? `, ${r.report.warnings.length} warning(s)` : ''}`,
        r.report.warnings.length ? 'alarm' : 'info',
      )
    } catch (e) {
      s.notify(e instanceof Error ? e.message : String(e), 'fault')
    }
  }
  const console_ = profile.console
  const focus = useEditor((s) => s.focus)
  // "Change address / map to terminal" from a tag's menu.
  useEffect(() => {
    if (focus?.kind !== 'point') return
    const s = useEditor.getState()
    s.setFocus(null)
    const tag = s.project.globals.find((t) => t.name.toLowerCase() === focus.tag.toLowerCase())
    const point = tag?.address ? profile.io.find((p) => p.address.toUpperCase() === tag.address?.toUpperCase()) : undefined
    if (point) {
      requestAnimationFrame(() => {
        const sel = document.getElementById(`io-${point.id}`)
        sel?.scrollIntoView({ block: 'center' })
        sel?.focus()
      })
    } else s.notify(`${focus.tag} is not on a terminal of ${profile.device.name}: pick it in a terminal's Tag list`)
  }, [focus, profile])

  return (
    <div className="min-h-0 flex-1 overflow-y-auto">
      <div className="flex flex-wrap items-end gap-3 border-b border-line bg-surface px-4 py-3">
        <div>
          <h1 className="text-base font-semibold">I/O mapping · {dev?.name}</h1>
          <p className="text-dense text-text-muted">
            Bind tags to the device's terminals. Binding sets the tag's address. Manifest{' '}
            <span className="text-mono">{dev?.manifest ?? 'built-in simulator'}</span> · {profile.device.id} v{profile.device.version} ·{' '}
            {profile.target.triple} · image I={img.I} Q={img.Q} M={img.M}
          </p>
        </div>
        <div className="ml-auto">
          <label htmlFor="profile" className="block text-[11px] text-text-muted">
            Change device (tags follow their terminals)
          </label>
          <select
            id="profile"
            value={CATALOG.some((e) => e.id === profile.device.id) ? profile.device.id : ''}
            onChange={(e) => setProfile(e.target.value)}
            className="h-8 rounded-control border border-line bg-bg px-2 outline-none focus-visible:border-text-muted"
          >
            {!CATALOG.some((e) => e.id === profile.device.id) && (
              <option value="" className="bg-surface-2">
                {profile.device.name} (imported)
              </option>
            )}
            {CATALOG.map((p) => (
              <option key={p.id} value={p.id} className="bg-surface-2">
                {p.device.device.name}
              </option>
            ))}
          </select>
        </div>
      </div>
      <div className="space-y-4 p-4">
        {resolved.problem && (
          <p role="alert" className="flex items-center gap-1.5 rounded-lg border border-alarm-border bg-alarm-bg p-3 text-dense text-alarm">
            <AlertTriangle className="size-4" aria-hidden /> {resolved.problem}. The Simulator stands in until it is fixed.
          </p>
        )}
        {update && (
          <div role="status" className="flex items-center gap-2 rounded-lg border border-line bg-surface p-3 text-dense">
            <ArrowUpCircle className="size-4 text-text-muted" aria-hidden />
            <span>
              The catalog has version {update.latest.device.device.version} of this manifest; the project has version {update.current.device.version}.
            </span>
            <Button size="xs" variant="outline" className="ml-auto" onClick={() => setReviewing(update)}>
              Review and update…
            </Button>
          </div>
        )}
        {report && <RemapReportView report={report} onDismiss={() => setReport(null)} />}
        <DeviceFace profile={profile} image={image} />
        {console_ && (
          <p className="text-dense text-text-muted">
            Online: USB console at {console_.baud} baud ({console_.commands.join(', ')}); <span className="text-mono">img</span> reports %I, %Q and
            the first {console_.img_m_bytes} bytes of %M (so %MW0..%MW{Math.floor(console_.img_m_bytes / 2) - 1} are visible online)
            {console_.commands.includes('mw') && (
              <>
                , <span className="text-mono">mw n v</span> writes %MWn
              </>
            )}
            .
          </p>
        )}
        {groups.map(([group, points]) => (
          <section key={group} aria-label={group}>
            <h2 className="mb-1 text-[11px] font-semibold tracking-wider text-text-muted uppercase">{group}</h2>
            <div className="rounded-lg border border-line bg-surface">
              <Table className="text-dense">
                <TableHeader>
                  <TableRow>
                    <TableHead>
                      <span className="sr-only">State</span>
                    </TableHead>
                    <TableHead>Terminal</TableHead>
                    <TableHead>Point</TableHead>
                    <TableHead>Address</TableHead>
                    <TableHead>Type</TableHead>
                    <TableHead>Tag</TableHead>
                    <TableHead className="text-right">Value</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {points.map((pt) => (
                    <PointRow key={pt.id} point={pt} tags={project.globals} image={image} />
                  ))}
                </TableBody>
              </Table>
            </div>
          </section>
        ))}
        {stray.length > 0 && (
          <section className="rounded-lg border border-alarm-border bg-alarm-bg p-3 text-dense text-alarm" aria-label="Addresses not on this device">
            <h2 className="mb-1 flex items-center gap-1.5 font-semibold">
              <AlertTriangle className="size-4" aria-hidden /> Addresses not on {profile.device.name}
            </h2>
            <ul className="list-inside list-disc">
              {stray.map((t) => (
                <li key={t.name}>
                  <span className="text-mono">{t.name}</span> at <span className="text-mono">{t.address}</span>
                </li>
              ))}
            </ul>
          </section>
        )}
      </div>
      <ManifestUpdateDialog update={reviewing} onClose={() => setReviewing(null)} onUpdated={setReport} />
    </div>
  )
}
