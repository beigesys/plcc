// SPDX-License-Identifier: MPL-2.0
import { useCallback, useEffect, useRef, useState } from 'react'
import { Download, FolderOpen, Pencil, Plus, Trash2, Upload } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { demoProject } from '@/model'
import { useEditor } from '@/state/editor'
import {
  createProject, deleteProject, downloadZip, importZipFile, isPersistent, listProjects, openById, renameProject,
} from '@/state/persistence'
import type { ProjectSummary } from '@/store'

export function ProjectsDialog() {
  const open = useEditor((s) => s.projectsOpen)
  const setOpen = useEditor((s) => s.setProjectsOpen)
  const currentId = useEditor((s) => s.projectId)
  const project = useEditor((s) => s.project)
  const [items, setItems] = useState<ProjectSummary[]>([])
  const [name, setName] = useState('')
  const [renaming, setRenaming] = useState<string | null>(null)
  const [renameText, setRenameText] = useState('')
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null)
  const [error, setError] = useState('')
  const fileRef = useRef<HTMLInputElement>(null)

  const refresh = useCallback(() => {
    listProjects()
      .then(setItems)
      .catch((e: unknown) => setError(String(e)))
  }, [])

  useEffect(() => {
    if (open) refresh()
  }, [open, refresh, project.name])

  /** Runs an operation; resolves true on success, shows the error otherwise. */
  const guard = (p: Promise<unknown>): Promise<boolean> =>
    p.then(
      () => {
        setError('')
        refresh()
        return true
      },
      (e: unknown) => {
        setError(e instanceof Error ? e.message : String(e))
        return false
      },
    )

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent className="max-w-xl">
        <DialogHeader>
          <DialogTitle>Projects</DialogTitle>
          <DialogDescription>
            {isPersistent()
              ? 'Stored in this browser (Origin Private File System). Export a .zip to move or back up a project.'
              : 'OPFS is not available here, so projects live in memory until the tab closes. Export a .zip to keep one.'}
          </DialogDescription>
        </DialogHeader>
        <ul className="max-h-72 divide-y divide-line overflow-y-auto rounded-lg border border-line">
          {items.map((p) => (
            <li key={p.id} className="flex items-center gap-2 px-3 py-2">
              {renaming === p.id ? (
                <form
                  className="flex flex-1 gap-1"
                  onSubmit={(e) => {
                    e.preventDefault()
                    if (renameText.trim()) void guard(renameProject(p.id, renameText.trim()))
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
              ) : (
                <>
                  <span className="min-w-0 flex-1 truncate">
                    {p.name}
                    {p.id === currentId && <span className="ml-2 text-[11px] text-text-muted">open</span>}
                  </span>
                  {confirmDelete === p.id ? (
                    <>
                      <span className="text-dense text-alarm">Delete for good?</span>
                      <Button size="xs" variant="destructive" onClick={() => void guard(deleteProject(p.id)).then(() => setConfirmDelete(null))}>
                        Delete
                      </Button>
                      <Button size="xs" variant="ghost" onClick={() => setConfirmDelete(null)}>
                        Keep
                      </Button>
                    </>
                  ) : (
                    <>
                      <Button size="xs" variant="outline" disabled={p.id === currentId} onClick={() => void guard(openById(p.id)).then((ok) => ok && setOpen(false))}>
                        <FolderOpen /> Open
                      </Button>
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
                </>
              )}
            </li>
          ))}
          {items.length === 0 && <li className="px-3 py-4 text-center text-text-muted">No projects.</li>}
        </ul>
        <form
          className="flex gap-2"
          onSubmit={(e) => {
            e.preventDefault()
            const n = name.trim()
            if (!n) return
            void guard(createProject(n)).then((ok) => {
              if (!ok) return
              setName('')
              setOpen(false)
            })
          }}
        >
          <label htmlFor="new-project" className="sr-only">
            New project name
          </label>
          <input
            id="new-project"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="New project name"
            className="h-8 flex-1 rounded-control border border-line bg-bg px-2 outline-none focus-visible:border-text-muted"
          />
          <Button type="submit" size="sm">
            <Plus /> New
          </Button>
          <Button type="button" size="sm" variant="outline" onClick={() => void guard(createProject('Demo Opta', demoProject())).then((ok) => ok && setOpen(false))}>
            New demo
          </Button>
        </form>
        <div className="flex flex-wrap gap-2 border-t border-line pt-3">
          <Button size="sm" variant="outline" onClick={() => downloadZip(project)}>
            <Download /> Export “{project.name}” (.zip)
          </Button>
          <Button size="sm" variant="outline" onClick={() => fileRef.current?.click()}>
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
              if (f) void guard(importZipFile(f)).then((ok) => ok && setOpen(false))
            }}
          />
        </div>
        {error && (
          <p role="alert" className="text-dense text-fault">
            {error}
          </p>
        )}
      </DialogContent>
    </Dialog>
  )
}
