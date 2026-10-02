// SPDX-License-Identifier: MPL-2.0
import { useProjects } from '@/state/persistence'
import { Logo } from './Logo'
import { ProjectsPanel } from './ProjectsPanel'

/** What shows when no project is open. */
export function StartScreen() {
  const folderAccess = useProjects((s) => s.folderAccess)
  return (
    <main aria-label="Start" className="flex h-full justify-center overflow-y-auto bg-bg px-6 py-14 text-text">
      <div className="w-full max-w-2xl space-y-8">
        <header className="space-y-2">
          <div className="flex items-center gap-2.5">
            <Logo />
            <h1 className="text-xl font-semibold tracking-tight">plcc studio</h1>
          </div>
          <p className="text-text-muted">
            Ladder logic for PLCs, compiled by plcc, in the browser.{' '}
            {folderAccess
              ? 'A project is a folder on your disk, so it can live in git, be edited by other tools, and be backed up like any other files.'
              : 'Projects are kept in this browser’s storage.'}
          </p>
        </header>
        <ProjectsPanel />
      </div>
    </main>
  )
}
