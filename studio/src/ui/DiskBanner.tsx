// SPDX-License-Identifier: MPL-2.0
//
// When the project's folder needs the user: write access was lost, the files
// changed on disk while there were unsaved edits, or the folder is gone.

import { AlertTriangle } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { useEditor } from '@/state/editor'
import { closeProject, grantAccess, keepMine, reloadFromDisk, saveToFolder } from '@/state/persistence'
import { runProjectAction } from './projectActions'

export function DiskBanner() {
  const disk = useEditor((s) => s.disk)
  if (!disk) return null
  let text: string
  let actions: React.ReactNode
  if (disk.kind === 'permission') {
    text = `The studio can no longer write to “${disk.folder}”. Your edits are kept here, unsaved, until you allow access again.`
    actions = <Button size="xs" onClick={() => void grantAccess()}>Grant access</Button>
  } else if (disk.kind === 'gone') {
    text = `The folder “${disk.folder}” was moved or deleted. Your edits are kept here, unsaved.`
    actions = (
      <>
        <Button size="xs" onClick={() => void runProjectAction(() => saveToFolder())}>Save to another folder…</Button>
        <Button size="xs" variant="ghost" onClick={() => void closeProject()}>Close project</Button>
      </>
    )
  } else {
    text = disk.error
      ? `${disk.paths.join(', ')} changed on disk and cannot be read (${disk.error}). Your edits are not saved.`
      : `${disk.paths.join(', ')} changed on disk while you had unsaved edits.`
    actions = (
      <>
        <Button size="xs" variant="outline" onClick={() => void reloadFromDisk()}>Reload from disk</Button>
        <Button size="xs" onClick={() => void keepMine()}>Keep mine and overwrite</Button>
      </>
    )
  }
  return (
    <div role="alert" data-testid="disk-banner" data-kind={disk.kind} className="flex items-center gap-3 border-b border-alarm-border bg-alarm-bg px-3 py-1.5 text-dense">
      <AlertTriangle className="size-4 shrink-0 text-alarm" />
      <span className="min-w-0 flex-1">{text}</span>
      <span className="flex shrink-0 gap-1.5">{actions}</span>
    </div>
  )
}
