// SPDX-License-Identifier: MPL-2.0
import { useEditor } from '@/state/editor'
import { describeError, RecentError, setProjectsNote, useProjects, type FolderResult } from '@/state/persistence'
import { ProjectFormatError } from '@/store'

/**
 * Runs a project action (new, open, ...). Errors become the projects
 * screen's note. Resolves true when a project was opened.
 */
export async function runProjectAction(fn: () => Promise<FolderResult | unknown>): Promise<boolean> {
  try {
    const r = await fn()
    if (r === 'cancelled' || r === 'offered') return false
    setProjectsNote(null)
    return true
  } catch (e) {
    const text = e instanceof ProjectFormatError ? `Cannot open it: ${e.message}` : describeError(e)
    setProjectsNote({ tone: 'error', text, removeKey: e instanceof RecentError ? e.key : undefined })
    return false
  }
}

/** A project action from a menu: an error or an offer opens the Projects dialog to show it. */
export function fileAction(fn: () => Promise<unknown>) {
  void runProjectAction(fn).then((ok) => {
    if (!ok && useProjects.getState().note) useEditor.getState().setProjectsOpen(true)
  })
}
