// SPDX-License-Identifier: MPL-2.0
//
// New / Open / Recent / browser storage: the start screen's content, and the
// Projects dialog's.

import { useRef, useState } from 'react'
import { Download, FilePlus2, FolderInput, FolderOpen, HardDrive, Pencil, Play, Plus, Trash2, Upload, X } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { useEditor } from '@/state/editor'
import {
  createBrowserProject, deleteBrowserProject, downloadZip, importZipFile, newFolderProject, openBrowserProject, openDemo,
  openFolderHandle, openFolderProject, openRecent, removeRecent, renameBrowserProject, saveToFolder, setProjectsNote,
  useProjects,
} from '@/state/persistence'
import type { RecentEntry } from '@/store'
import { runProjectAction } from './projectActions'

function ago(t: number): string {
  const s = Math.max(0, (Date.now() - t) / 1000)
  if (s < 60) return 'just now'
  if (s < 3600) return `${Math.round(s / 60)} min ago`
  if (s < 86400) return `${Math.round(s / 3600)} h ago`
  return new Date(t).toLocaleDateString()
}

function Note({ onOpened }: { onOpened: () => void }) {
  const note = useProjects((s) => s.note)
  if (!note) return null
  const tone = note.tone === 'error' ? 'border-fault/50 text-fault' : 'border-alarm-border text-text'
  return (
    <div role="alert" className={`flex flex-wrap items-center gap-2 rounded-lg border bg-surface px-3 py-2 text-dense ${tone}`}>
      <span className="min-w-0 flex-1">{note.text}</span>
      {note.tone === 'offer' && (
        <Button size="xs" onClick={() => void runProjectAction(() => openFolderHandle(note.handle)).then((ok) => ok && onOpened())}>
          Open it
        </Button>
      )}
      {note.tone === 'error' && note.removeKey && (
        <Button size="xs" variant="outline" onClick={() => void removeRecent(note.removeKey!).then(() => setProjectsNote(null))}>
          Remove from list
        </Button>
      )}
      <Button size="icon-xs" variant="ghost" aria-label="Dismiss" onClick={() => setProjectsNote(null)}>
        <X />
      </Button>
    </div>
  )
}

function RecentList({ onOpened }: { onOpened: () => void }) {
  const items = useProjects((s) => s.recent)
  const resumeKey = useProjects((s) => s.resumeKey)
  const current = useEditor((s) => (s.projectSource?.kind === 'folder' ? s.projectSource.key : null))
  if (!items.length) return null
  return (
    <section aria-labelledby="recent-heading" className="space-y-1.5">
      <h2 id="recent-heading" className="text-[11px] font-semibold tracking-wide text-text-muted uppercase">
        Recent folders
      </h2>
      <ul className="divide-y divide-line overflow-hidden rounded-lg border border-line">
        {items.map((e: RecentEntry) => (
          <li key={e.key} className="flex items-center gap-1 pr-2 hover:bg-surface-2">
            <button
              type="button"
              className="flex min-w-0 flex-1 items-center gap-2 px-3 py-2 text-left disabled:opacity-60"
              disabled={e.key === current}
              title={e.key === resumeKey ? 'Open last time: click to allow access again' : `Open ${e.folder}`}
              onClick={() => void runProjectAction(() => openRecent(e)).then((ok) => ok && onOpened())}
            >
              <FolderOpen className="size-4 shrink-0 text-text-muted" />
              <span className="min-w-0 flex-1 truncate">
                <span className="font-medium">{e.name || e.folder}</span>
                <span className="ml-2 text-text-muted">{e.folder}</span>
              </span>
              {e.key === resumeKey && <span className="rounded-control bg-alarm-bg px-1.5 text-[11px] text-alarm">click to reopen</span>}
              {e.key === current ? <span className="text-[11px] text-text-muted">open</span> : <span className="text-[11px] text-text-muted">{ago(e.openedAt)}</span>}
            </button>
            <Button size="icon-xs" variant="ghost" aria-label={`Remove ${e.folder} from recent`} onClick={() => void removeRecent(e.key)}>
              <X />
            </Button>
          </li>
        ))}
      </ul>
    </section>
  )
}

function BrowserList({ onOpened }: { onOpened: () => void }) {
  const items = useProjects((s) => s.browser)
  const folderAccess = useProjects((s) => s.folderAccess)
  const persistent = useProjects((s) => s.persistent)
  const current = useEditor((s) => (s.projectSource?.kind === 'browser' ? s.projectSource.id : null))
  const [renaming, setRenaming] = useState<string | null>(null)
  const [renameText, setRenameText] = useState('')
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null)
  const [name, setName] = useState('')
  const fileRef = useRef<HTMLInputElement>(null)

  return (
    <section aria-labelledby="browser-heading" className="space-y-1.5">
      <h2 id="browser-heading" className="flex items-center gap-1.5 text-[11px] font-semibold tracking-wide text-text-muted uppercase">
        <HardDrive className="size-3.5" /> Browser storage (no folder)
      </h2>
      <p className="text-dense text-text-muted">
        {!persistent
          ? 'This browser has no private storage here, so these projects last until the tab closes. Export a .zip to keep one.'
          : folderAccess
            ? 'Kept inside this browser, not in files you can see. For quick experiments; Save to folder… moves one to disk.'
            : 'Kept inside this browser. Export a .zip to back one up or move it to another machine.'}
      </p>
      <ul className="max-h-60 divide-y divide-line overflow-y-auto rounded-lg border border-line">
        {items.map((p) => (
          <li key={p.id} className="flex items-center gap-1.5 px-3 py-1.5">
            {renaming === p.id ? (
              <form
                className="flex flex-1 gap-1"
                onSubmit={(e) => {
                  e.preventDefault()
                  if (renameText.trim()) void runProjectAction(() => renameBrowserProject(p.id, renameText.trim()))
                  setRenaming(null)
                }}
              >
                <input
                  autoFocus
                  aria-label={`New name for ${p.name}`}
                  value={renameText}
                  onChange={(e) => setRenameText(e.target.value)}
                  onKeyDown={(e) => e.key === 'Escape' && (e.stopPropagation(), setRenaming(null))}
                  className="h-7 flex-1 rounded-control border border-line bg-bg px-2 outline-none focus-visible:border-text-muted"
                />
                <Button size="xs" type="submit">
                  Save
                </Button>
              </form>
            ) : confirmDelete === p.id ? (
              <>
                <span className="min-w-0 flex-1 truncate">{p.name}</span>
                <span className="text-dense text-alarm">Delete for good?</span>
                <Button size="xs" variant="destructive" onClick={() => void runProjectAction(() => deleteBrowserProject(p.id)).then(() => setConfirmDelete(null))}>
                  Delete
                </Button>
                <Button size="xs" variant="ghost" onClick={() => setConfirmDelete(null)}>
                  Keep
                </Button>
              </>
            ) : (
              <>
                <span className="min-w-0 flex-1 truncate">
                  {p.name}
                  {p.id === current && <span className="ml-2 text-[11px] text-text-muted">open</span>}
                </span>
                <Button size="xs" variant="outline" disabled={p.id === current} onClick={() => void runProjectAction(() => openBrowserProject(p.id)).then((ok) => ok && onOpened())}>
                  Open
                </Button>
                {folderAccess && (
                  <Button size="xs" variant="ghost" title="Copy it into a folder on disk and work there" onClick={() => void runProjectAction(() => saveToFolder(p.id)).then((ok) => ok && onOpened())}>
                    <FolderInput /> Save to folder…
                  </Button>
                )}
                <Button
                  size="icon-xs"
                  variant="ghost"
                  aria-label={`Rename ${p.name}`}
                  onClick={() => {
                    setRenaming(p.id)
                    setRenameText(p.name)
                  }}
                >
                  <Pencil />
                </Button>
                <Button size="icon-xs" variant="ghost" aria-label={`Delete ${p.name}`} onClick={() => setConfirmDelete(p.id)}>
                  <Trash2 />
                </Button>
              </>
            )}
          </li>
        ))}
        {items.length === 0 && <li className="px-3 py-3 text-center text-text-muted">None yet.</li>}
      </ul>
      <form
        className="flex gap-2"
        onSubmit={(e) => {
          e.preventDefault()
          const n = name.trim()
          if (!n) return
          void runProjectAction(() => createBrowserProject(n)).then((ok) => {
            if (!ok) return
            setName('')
            onOpened()
          })
        }}
      >
        <label htmlFor="new-browser-project" className="sr-only">
          New project in browser storage
        </label>
        <input
          id="new-browser-project"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder={folderAccess ? 'New project in browser storage' : 'New project name'}
          className="h-7 min-w-0 flex-1 rounded-control border border-line bg-bg px-2 text-dense outline-none focus-visible:border-text-muted"
        />
        <Button type="submit" size="sm" variant="outline">
          <Plus /> Create
        </Button>
        <Button type="button" size="sm" variant="outline" onClick={() => fileRef.current?.click()}>
          <Upload /> Import .zip
        </Button>
        <input
          ref={fileRef}
          type="file"
          accept=".zip,application/zip"
          className="hidden"
          aria-label="Import project zip"
          onChange={(e) => {
            const f = e.target.files?.[0]
            e.target.value = ''
            if (f) void runProjectAction(() => importZipFile(f)).then((ok) => ok && onOpened())
          }}
        />
      </form>
    </section>
  )
}

/** New, Open, Try the demo, Recent, browser storage. `onOpened` runs after a project opens. */
export function ProjectsPanel({ onOpened = () => {} }: { onOpened?: () => void }) {
  const folderAccess = useProjects((s) => s.folderAccess)
  const source = useEditor((s) => s.projectSource)
  const projectOpen = useEditor((s) => s.projectId !== null)
  const project = useEditor((s) => s.project)
  return (
    <div className="space-y-4">
      <div className="flex flex-wrap gap-2">
        {folderAccess ? (
          <>
            <Button onClick={() => void runProjectAction(() => newFolderProject()).then((ok) => ok && onOpened())}>
              <FilePlus2 /> New project…
            </Button>
            <Button variant="outline" onClick={() => void runProjectAction(() => openFolderProject()).then((ok) => ok && onOpened())}>
              <FolderOpen /> Open project…
            </Button>
          </>
        ) : null}
        <Button variant={folderAccess ? 'ghost' : 'default'} onClick={() => void runProjectAction(() => openDemo()).then((ok) => ok && onOpened())}>
          <Play /> Try the demo
        </Button>
        {projectOpen && source?.kind === 'browser' && folderAccess && (
          <Button variant="ghost" onClick={() => void runProjectAction(() => saveToFolder()).then((ok) => ok && onOpened())}>
            <FolderInput /> Save “{project.name}” to folder…
          </Button>
        )}
        {projectOpen && (
          <Button variant="ghost" onClick={() => downloadZip(project)}>
            <Download /> Export “{project.name}” (.zip)
          </Button>
        )}
      </div>
      {!folderAccess && (
        <p className="rounded-lg border border-alarm-border bg-alarm-bg px-3 py-2 text-dense text-text">
          This browser cannot open folders on disk (the File System Access API is in Chrome and Edge, not Firefox or
          Safari). Projects are kept in browser storage instead: create one below, and use Export / Import .zip to back
          them up, move them, or put them in git.
        </p>
      )}
      <Note onOpened={onOpened} />
      <RecentList onOpened={onOpened} />
      <BrowserList onOpened={onOpened} />
    </div>
  )
}
