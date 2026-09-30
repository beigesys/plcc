// SPDX-License-Identifier: MPL-2.0
//
// Device manifests in the UI: "Add device" (catalog, import a .toml, detect
// over WebSerial), the catalog-update review with a diff, and the report of a
// device change (tags remapped by terminal, and the ones that could not be).

import { useMemo, useRef, useState } from 'react'
import { AlertTriangle, ArrowUpCircle, FileUp, Plus, Usb } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { CATALOG, type CatalogEntry } from '@/devices/catalog'
import { formatDiagnostic, loadManifest } from '@/devices/manifest'
import { addDevice, lineDiff, updateManifestFile, type ManifestUpdate, type RemapReport } from '@/devices/project'
import { useEditor } from '@/state/editor'
import { detectOverSerial, onlineSupported } from '@/state/online'
import type { Detection } from '@/serial'

type Tab = 'catalog' | 'import' | 'detect'

function commitAdd(text: string, name: string | undefined): string | undefined {
  const s = useEditor.getState()
  try {
    const r = addDevice(s.project, text, name)
    s.commit(() => r.project)
    s.setView({ kind: 'io', device: r.name })
    s.notify(`Added ${r.name}; its manifest is now part of the project`)
    return undefined
  } catch (e) {
    return e instanceof Error ? e.message : String(e)
  }
}

function CatalogList({ onAdd }: { onAdd(e: CatalogEntry): void }) {
  return (
    <ul className="max-h-72 divide-y divide-line overflow-y-auto rounded-lg border border-line" aria-label="Device catalog">
      {CATALOG.map((e) => (
        <li key={e.id} className="flex items-start gap-3 px-3 py-2">
          <div className="min-w-0 flex-1">
            <p className="font-medium">
              {e.device.device.name} <span className="text-[11px] text-text-muted">{e.device.device.vendor} · v{e.device.device.version}</span>
            </p>
            <p className="text-dense text-text-muted">{e.device.device.description}</p>
            <p className="text-[11px] text-text-muted text-mono">
              {e.id} · {e.device.target.triple} · {e.device.io.length} I/O points
              {e.origin === 'builtin' && ' · built-in copy'}
            </p>
          </div>
          <Button size="xs" onClick={() => onAdd(e)} aria-label={`Add ${e.device.device.name}`}>
            <Plus /> Add
          </Button>
        </li>
      ))}
    </ul>
  )
}

function ImportPane({ name, onDone }: { name: string; onDone(err?: string): void }) {
  const [text, setText] = useState('')
  const fileRef = useRef<HTMLInputElement>(null)
  const result = useMemo(() => (text.trim() ? loadManifest(text) : undefined), [text])
  return (
    <div className="space-y-2">
      <p className="text-dense text-text-muted">
        A manifest from elsewhere is untrusted data: check it before use. It can only narrow what the flasher allows, never widen it.
      </p>
      <div className="flex gap-2">
        <Button size="sm" variant="outline" onClick={() => fileRef.current?.click()}>
          <FileUp /> Open .toml…
        </Button>
        <input
          ref={fileRef}
          type="file"
          accept=".toml,application/toml,text/plain"
          className="hidden"
          aria-label="Import device manifest"
          onChange={(e) => {
            const f = e.target.files?.[0]
            e.target.value = ''
            if (f) void f.text().then(setText)
          }}
        />
      </div>
      <label htmlFor="manifest-text" className="sr-only">
        Manifest TOML
      </label>
      <textarea
        id="manifest-text"
        value={text}
        onChange={(e) => setText(e.target.value)}
        placeholder="…or paste a device manifest (TOML)"
        spellCheck={false}
        className="h-40 w-full rounded-control border border-line bg-bg p-2 text-mono text-[12px] outline-none focus-visible:border-text-muted"
      />
      {result && result.diagnostics.length > 0 && (
        <ul className="max-h-28 overflow-y-auto text-[11px]" aria-label="Manifest problems">
          {result.diagnostics.map((d, i) => (
            <li key={i} className={d.severity === 'error' ? 'text-fault' : 'text-alarm'}>
              {formatDiagnostic(d, 'manifest')}
            </li>
          ))}
        </ul>
      )}
      {result?.device && (
        <p className="text-dense">
          {result.device.device.name} v{result.device.device.version} ({result.device.device.id}), {result.device.io.length} I/O points.
        </p>
      )}
      <div className="flex justify-end">
        <Button size="sm" disabled={!result?.device} onClick={() => onDone(commitAdd(text, name))}>
          <Plus /> Add device
        </Button>
      </div>
    </div>
  )
}

function DetectPane({ name, onDone }: { name: string; onDone(err?: string): void }) {
  const [busy, setBusy] = useState(false)
  const [det, setDet] = useState<Detection | null>(null)
  const [err, setErr] = useState('')
  const detect = async () => {
    setBusy(true)
    setErr('')
    setDet(null)
    try {
      setDet(await detectOverSerial())
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="space-y-2">
      <p className="text-dense text-text-muted">
        Pick the device's USB serial port. Studio sends <span className="text-mono">info</span> and matches the answer to the catalog.
      </p>
      <Button size="sm" onClick={() => void detect()} disabled={busy || !onlineSupported()} title={onlineSupported() ? undefined : 'WebSerial needs Chrome or Edge'}>
        <Usb /> {busy ? 'Asking…' : 'Detect over USB…'}
      </Button>
      {err && <p className="text-dense text-fault">{err}</p>}
      {det && (
        <div className="space-y-1 rounded-lg border border-line p-2 text-dense">
          <p>
            The device says: <span className="text-mono">{det.identity.device}</span>, manifest v{det.identity.manifest}, runtime{' '}
            <span className="text-mono">{det.identity.runtime}</span> ABI {det.identity.abi}, image I={det.identity.image.I} Q={det.identity.image.Q} M=
            {det.identity.image.M}.
          </p>
          {!det.entry && <p className="text-alarm">No catalog manifest has that id. Import its manifest instead.</p>}
          {det.check?.mismatch.map((m) => (
            <p key={m} className="flex items-center gap-1 text-alarm">
              <AlertTriangle className="size-3" aria-hidden /> {m}
            </p>
          ))}
          {det.check?.notes.map((m) => (
            <p key={m} className="text-text-muted">
              {m}
            </p>
          ))}
          {det.entry && (
            <div className="flex justify-end">
              <Button size="sm" onClick={() => onDone(commitAdd(det.entry!.text, name))}>
                <Plus /> Add {det.entry.device.device.name} (v{det.entry.device.device.version})
              </Button>
            </div>
          )}
        </div>
      )}
    </div>
  )
}

export function AddDeviceDialog({ open, onOpenChange }: { open: boolean; onOpenChange(o: boolean): void }) {
  const [tab, setTab] = useState<Tab>('catalog')
  const [name, setName] = useState('')
  const [error, setError] = useState('')
  const done = (err?: string) => {
    if (err) return setError(err)
    setError('')
    setName('')
    onOpenChange(false)
  }
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-xl">
        <DialogHeader>
          <DialogTitle>Add device</DialogTitle>
          <DialogDescription>
            The device's manifest is copied into the project (<span className="text-mono">devices/&lt;id&gt;.toml</span>), so the project keeps working
            the same way when the catalog changes.
          </DialogDescription>
        </DialogHeader>
        <div role="tablist" aria-label="Add device from" className="flex rounded-control border border-line bg-surface p-0.5">
          {(
            [
              ['catalog', 'Catalog'],
              ['import', 'Import .toml'],
              ['detect', 'Detect'],
            ] as const
          ).map(([id, label]) => (
            <button
              key={id}
              type="button"
              role="tab"
              aria-selected={tab === id}
              onClick={() => setTab(id)}
              className={`flex-1 rounded-control px-3 py-1 text-dense ${tab === id ? 'bg-surface-2 text-text shadow-[inset_0_0_0_1px_var(--border)]' : 'text-text-muted'}`}
            >
              {label}
            </button>
          ))}
        </div>
        <div className="flex items-center gap-2">
          <label htmlFor="device-name" className="text-dense text-text-muted">
            Name in the project
          </label>
          <input
            id="device-name"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="(the device's name)"
            className="h-7 flex-1 rounded-control border border-line bg-bg px-2 outline-none focus-visible:border-text-muted"
          />
        </div>
        {tab === 'catalog' && <CatalogList onAdd={(e) => done(commitAdd(e.text, name))} />}
        {tab === 'import' && <ImportPane name={name} onDone={done} />}
        {tab === 'detect' && <DetectPane name={name} onDone={done} />}
        {error && (
          <p role="alert" className="text-dense whitespace-pre-line text-fault">
            {error}
          </p>
        )}
      </DialogContent>
    </Dialog>
  )
}

export function RemapReportView({ report, onDismiss }: { report: RemapReport; onDismiss?(): void }) {
  if (report.moved.length === 0 && report.warnings.length === 0) return null
  return (
    <section
      aria-label="Tag remapping"
      className={`rounded-lg border p-3 text-dense ${report.warnings.length ? 'border-alarm-border bg-alarm-bg' : 'border-line bg-surface'}`}
    >
      <div className="mb-1 flex items-center">
        <h2 className="font-semibold">
          {report.moved.length} tag{report.moved.length === 1 ? '' : 's'} remapped by terminal
          {report.warnings.length > 0 && `, ${report.warnings.length} not placed`}
        </h2>
        {onDismiss && (
          <Button size="xs" variant="ghost" className="ml-auto" onClick={onDismiss}>
            Dismiss
          </Button>
        )}
      </div>
      <ul className="space-y-0.5">
        {report.moved.map((m) => (
          <li key={m.tag} className="text-mono">
            {m.tag}: {m.from} → {m.to} ({m.terminal})
          </li>
        ))}
        {report.warnings.map((w) => (
          <li key={w} className="flex items-center gap-1 text-alarm">
            <AlertTriangle className="size-3 shrink-0" aria-hidden /> {w}
          </li>
        ))}
      </ul>
    </section>
  )
}

export function ManifestUpdateDialog({
  update,
  onClose,
  onUpdated,
}: {
  update: ManifestUpdate | null
  onClose(): void
  onUpdated(report: RemapReport): void
}) {
  const project = useEditor((s) => s.project)
  const [error, setError] = useState('')
  const diff = useMemo(() => (update ? lineDiff(project.deviceFiles[update.path] ?? '', update.latest.text) : []), [update, project.deviceFiles])
  const changed = diff.filter((d) => d.op !== ' ').length
  return (
    <Dialog open={update !== null} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="max-w-3xl">
        {update && (
          <>
            <DialogHeader>
              <DialogTitle>
                Update {update.current.device.name}: v{update.current.device.version} → v{update.latest.device.device.version}
              </DialogTitle>
              <DialogDescription>
                The catalog has a newer manifest for <span className="text-mono">{update.path}</span>. Nothing changes until you update; tags are remapped
                by terminal where addresses moved.
              </DialogDescription>
            </DialogHeader>
            <pre aria-label="Manifest diff" className="max-h-96 overflow-auto rounded-lg border border-line bg-bg p-2 text-mono text-[11px] leading-snug">
              {diff.map((d, i) => (
                <div
                  key={i}
                  /* Neutral: in these themes color means machine state (ISA-101). */
                  className={d.op === '+' ? 'bg-surface-2 font-semibold text-text' : d.op === '-' ? 'text-text-muted line-through' : 'text-text-muted opacity-70'}
                >
                  {d.op} {d.text}
                </div>
              ))}
            </pre>
            <p className="text-[11px] text-text-muted">{changed} changed line(s).</p>
            {error && <p className="text-dense text-fault">{error}</p>}
            <div className="flex justify-end gap-2">
              <Button variant="ghost" size="sm" onClick={onClose}>
                Not now
              </Button>
              <Button
                size="sm"
                onClick={() => {
                  const s = useEditor.getState()
                  try {
                    const r = updateManifestFile(s.project, update.path, update.latest.text)
                    s.commit(() => r.project)
                    s.notify(`${update.path} updated to version ${update.latest.device.device.version}`)
                    setError('')
                    onUpdated(r.report)
                    onClose()
                  } catch (e) {
                    setError(e instanceof Error ? e.message : String(e))
                  }
                }}
              >
                <ArrowUpCircle /> Update
              </Button>
            </div>
          </>
        )}
      </DialogContent>
    </Dialog>
  )
}
