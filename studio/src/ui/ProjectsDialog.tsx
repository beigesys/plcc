// SPDX-License-Identifier: MPL-2.0
import { useEffect } from 'react'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { useEditor } from '@/state/editor'
import { refreshLists, setProjectsNote, useProjects } from '@/state/persistence'
import { ProjectsPanel } from './ProjectsPanel'

export function ProjectsDialog() {
  const open = useEditor((s) => s.projectsOpen)
  const setOpen = useEditor((s) => s.setProjectsOpen)
  const folderAccess = useProjects((s) => s.folderAccess)

  useEffect(() => {
    if (open) void refreshLists()
  }, [open])

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        setOpen(o)
        if (!o) setProjectsNote(null)
      }}
    >
      <DialogContent className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>Projects</DialogTitle>
          <DialogDescription>
            {folderAccess
              ? 'A project is a folder on disk: project.toml, project.json and devices/. Keep it in git, edit it elsewhere; the studio notices.'
              : 'Projects are kept in this browser’s storage.'}
          </DialogDescription>
        </DialogHeader>
        <ProjectsPanel onOpened={() => setOpen(false)} />
      </DialogContent>
    </Dialog>
  )
}
