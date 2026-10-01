// SPDX-License-Identifier: MPL-2.0
// Dialogs opened by commands: import (L5X, PLCopen XML, ST, TwinCAT), the
// last export's translation notes, a device manifest, device dialogs.

import { useState } from 'react'
import { strFromU8, unzipSync } from 'fflate'
import { AlertTriangle, FileUp, FolderOpen } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { catalogUpdates } from '@/devices/project'
import { emptyProject } from '@/model'
import type { Diagnostic } from '@/plcc/frontend'
import { importKind, ImportError, lastExportReport, mergeImport, readImport, type Imported, type ImportKind } from '@/state/convert'
import { useEditor } from '@/state/editor'
import { createProject } from '@/state/persistence'
import { AddDeviceDialog, ManifestUpdateDialog } from './DeviceDialogs'
import { DownloadDialog } from './DownloadDialog'

function DiagList({ diagnostics, label }: { diagnostics: Diagnostic[]; label: string }) {
  if (!diagnostics.length) return null
  return (
    <ul aria-label={label} className="max-h-64 space-y-1 overflow-y-auto rounded-lg border border-line bg-bg p-2 text-dense">
      {diagnostics.map((d, i) => (
        <li key={i} className={d.severity === 'error' ? 'text-fault' : d.severity === 'warning' ? 'text-alarm' : 'text-text-muted'}>
          {d.severity === 'error' ? 'error: ' : d.severity === 'warning' ? 'warning: ' : ''}
          {d.file && d.span ? `${d.file}:${d.span.start.line}:${d.span.start.col}: ` : ''}
          {d.message}
        </li>
      ))}
    </ul>
  )
}

const TWINCAT_EXT = /\.(plcproj|tcpou|tcdut|tcgvl|tcio|tctto)$/i

/** Files of a TwinCAT project folder (File System Access API, Chromium). */
async function readFolder(dir: FileSystemDirectoryHandle, prefix = '', out: Record<string, string> = {}): Promise<Record<string, string>> {
  for await (const [name, h] of (dir as unknown as { entries(): AsyncIterable<[string, FileSystemHandle]> }).entries()) {
    const path = prefix ? `${prefix}/${name}` : name
    if (h.kind === 'directory') {
      if (!/^(\.git|_Boot|_CompileInfo|_Libraries)$/i.test(name)) await readFolder(h as FileSystemDirectoryHandle, path, out)
    } else if (TWINCAT_EXT.test(name)) out[path] = await (await (h as FileSystemFileHandle).getFile()).text()
  }
  return out
}

function unzipTwincat(bytes: Uint8Array): Record<string, string> {
  const raw = unzipSync(bytes)
  const out: Record<string, string> = {}
  for (const [p, b] of Object.entries(raw)) if (TWINCAT_EXT.test(p)) out[p.replace(/\\/g, '/')] = strFromU8(b)
  return out
}

export function ImportDialog() {
  const dialog = useEditor((s) => s.dialog)
  const setDialog = useEditor((s) => s.setDialog)
  const [busy, setBusy] = useState(false)
  const [result, setResult] = useState<{ name: string; imported: Imported } | null>(null)
  const [error, setError] = useState<{ message: string; diagnostics: Diagnostic[] } | null>(null)
  const open = dialog?.kind === 'import'

  const run = async (name: string, files: Record<string, string>, kind: ImportKind) => {
    setBusy(true)
    setError(null)
    setResult(null)
    try {
      setResult({ name, imported: await readImport(files, kind) })
    } catch (e) {
      setError({ message: e instanceof Error ? e.message : String(e), diagnostics: e instanceof ImportError ? e.diagnostics : [] })
    } finally {
      setBusy(false)
    }
  }

  const onFile = async (f: File) => {
    const lower = f.name.toLowerCase()
    if (lower.endsWith('.zip')) {
      const files = unzipTwincat(new Uint8Array(await f.arrayBuffer()))
      if (!Object.keys(files).some((p) => p.toLowerCase().endsWith('.plcproj'))) {
        setError({ message: 'no TwinCAT .plcproj in the zip', diagnostics: [] })
        return
      }
      return run(f.name.replace(/\.zip$/i, ''), files, 'twincat')
    }
    const text = await f.text()
    const kind = importKind(f.name, text)
    if (!kind) return setError({ message: `${f.name}: not an .L5X, PLCopen .xml, .st or TwinCAT file`, diagnostics: [] })
    return run(f.name.replace(/\.[^.]+$/, ''), { [f.name.replace(/[^A-Za-z0-9._-]/g, '_')]: text }, kind)
  }

  const pickFolder = async () => {
    const w = window as unknown as { showDirectoryPicker?: () => Promise<FileSystemDirectoryHandle> }
    if (!w.showDirectoryPicker) return
    try {
      const dir = await w.showDirectoryPicker()
      const files = await readFolder(dir)
      if (!Object.keys(files).some((p) => p.toLowerCase().endsWith('.plcproj'))) {
        setError({ message: 'no .plcproj in that folder', diagnostics: [] })
        return
      }
      await run(dir.name, files, 'twincat')
    } catch {
      // cancelled
    }
  }

  const close = () => {
    setDialog(null)
    setResult(null)
    setError(null)
  }

  const add = () => {
    if (!result) return
    const s = useEditor.getState()
    const { project, report } = mergeImport(s.project, result.imported.model)
    s.commit(() => project)
    s.notify(`Imported ${report.programs.join(', ')} and ${report.tags} tag(s)${report.notes.length ? `; ${report.notes.join('; ')}` : ''}`)
    const first = project.pous.find((p) => p.name === report.programs[0])
    if (first?.routines[0]) s.setView({ kind: 'routine', program: first.name, routine: first.routines[0].name })
    close()
  }

  const asNew = async () => {
    if (!result) return
    const base = { ...emptyProject(result.name), pous: [], tasks: [] }
    const { project } = mergeImport(base, result.imported.model)
    await createProject(result.name, project)
    close()
  }

  const counts = result
    ? {
        programs: result.imported.model.pous.length,
        rungs: result.imported.model.pous.reduce((n, p) => n + p.routines.reduce((m, r) => m + r.rungs.length, 0), 0),
        tags: result.imported.model.globals.length,
      }
    : null
  const canFolder = typeof window !== 'undefined' && 'showDirectoryPicker' in window

  return (
    <Dialog open={open} onOpenChange={(o) => !o && close()}>
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <DialogTitle>Import</DialogTitle>
          <DialogDescription>
            Rockwell <span className="text-mono">.L5X</span>, PLCopen <span className="text-mono">.xml</span> (CODESYS, OpenPLC), Structured Text{' '}
            <span className="text-mono">.st</span> or a TwinCAT project. plcc reads it in your browser; IEC ladder and ST are translated to the Logix
            dialect the editor uses, and every difference is listed.
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-wrap gap-2">
          <Button asChild variant="outline" size="sm">
            <label>
              <FileUp /> Choose a file…
              <input
                type="file"
                className="sr-only"
                aria-label="Import file"
                accept=".L5X,.l5x,.xml,.st,.iecst,.zip,.plcproj"
                onChange={(e) => {
                  const f = e.target.files?.[0]
                  e.target.value = ''
                  if (f) void onFile(f)
                }}
              />
            </label>
          </Button>
          <Button variant="outline" size="sm" disabled={!canFolder} onClick={() => void pickFolder()} title={canFolder ? undefined : 'Folders need Chrome or Edge; import the project as a .zip instead'}>
            <FolderOpen /> TwinCAT project folder…
          </Button>
        </div>
        {busy && <p className="text-dense text-text-muted">Reading with plcc…</p>}
        {error && (
          <div role="alert" className="space-y-2">
            <p className="flex items-center gap-1.5 text-dense text-fault">
              <AlertTriangle className="size-4" aria-hidden /> {error.message}
            </p>
            <DiagList diagnostics={error.diagnostics} label="Import errors" />
          </div>
        )}
        {result && counts && (
          <div className="space-y-2" data-testid="import-result">
            <p className="text-dense">
              <span className="font-medium">{result.name}</span>: {counts.programs} program(s), {counts.rungs} rung(s), {counts.tags} controller tag(s).
            </p>
            <DiagList diagnostics={result.imported.diagnostics} label="Translation notes" />
            <div className="flex justify-end gap-2">
              <Button variant="ghost" size="sm" onClick={close}>
                Cancel
              </Button>
              <Button variant="outline" size="sm" onClick={() => void asNew()}>
                Open as a new project
              </Button>
              <Button size="sm" onClick={add}>
                Add to this project
              </Button>
            </div>
          </div>
        )}
      </DialogContent>
    </Dialog>
  )
}

export function ExportDialog() {
  const dialog = useEditor((s) => s.dialog)
  const setDialog = useEditor((s) => s.setDialog)
  const report = lastExportReport()
  return (
    <Dialog open={dialog?.kind === 'export' && !!report} onOpenChange={(o) => !o && setDialog(null)}>
      <DialogContent className="max-w-2xl">
        {report && (
          <>
            <DialogHeader>
              <DialogTitle>{report.ok ? `Exported ${report.file}` : `Could not export ${report.file}`}</DialogTitle>
              <DialogDescription>
                {report.diagnostics.length
                  ? 'plcc translated the ladder; these are the places where the result behaves differently or could not be translated.'
                  : 'No translation notes: the export behaves like the project.'}
              </DialogDescription>
            </DialogHeader>
            <DiagList diagnostics={report.diagnostics} label="Export notes" />
            <div className="flex justify-end">
              <Button size="sm" onClick={() => setDialog(null)}>
                Close
              </Button>
            </div>
          </>
        )}
      </DialogContent>
    </Dialog>
  )
}

function ManifestDialog() {
  const dialog = useEditor((s) => s.dialog)
  const setDialog = useEditor((s) => s.setDialog)
  const project = useEditor((s) => s.project)
  const dev = dialog?.kind === 'manifest' ? project.devices.find((d) => d.name === dialog.device) : undefined
  return (
    <Dialog open={!!dev} onOpenChange={(o) => !o && setDialog(null)}>
      <DialogContent className="max-w-3xl">
        {dev && (
          <>
            <DialogHeader>
              <DialogTitle>{dev.name} manifest</DialogTitle>
              <DialogDescription>
                <span className="text-mono">{dev.manifest}</span> (docs/device-manifest.md), the project's own copy.
              </DialogDescription>
            </DialogHeader>
            <pre aria-label="Manifest" className="max-h-[60vh] overflow-auto rounded-lg border border-line bg-bg p-2 text-mono text-[11px]">
              {project.deviceFiles[dev.manifest] ?? '(missing)'}
            </pre>
          </>
        )}
      </DialogContent>
    </Dialog>
  )
}

export function DialogHost() {
  const dialog = useEditor((s) => s.dialog)
  const setDialog = useEditor((s) => s.setDialog)
  const project = useEditor((s) => s.project)
  const update =
    dialog?.kind === 'updateManifest'
      ? (catalogUpdates(project).find((u) => u.path === project.devices.find((d) => d.name === dialog.device)?.manifest) ?? null)
      : null
  return (
    <>
      <ImportDialog />
      <ExportDialog />
      <ManifestDialog />
      <DownloadDialog />
      {dialog?.kind === 'addDevice' && (
        <AddDeviceDialog key={dialog.pane ?? 'catalog'} open initialTab={dialog.pane} onOpenChange={(o) => !o && setDialog(null)} />
      )}
      <ManifestUpdateDialog update={update} onClose={() => setDialog(null)} onUpdated={() => {}} />
    </>
  )
}
