// SPDX-License-Identifier: MPL-2.0
import { ChevronRight, Download, Palette, Check, FileText, Folder, HardDrive } from 'lucide-react'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuShortcut,
  DropdownMenuSub, DropdownMenuSubContent, DropdownMenuSubTrigger, DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { resolveDevice } from '@/devices/project'
import { useEditor, type Mode } from '@/state/editor'
import { useLive } from '@/state/live'
import { describeProgram } from '@/serial'
import { exportRoutine } from '@/state/convert'
import {
  closeProject, downloadZip, newFolderProject, openFolderProject, openRecent, refreshLists, saveToFolder, useProjects,
} from '@/state/persistence'
import { THEMES } from '@/state/theme'
import { Logo } from './Logo'
import { fileAction } from './projectActions'
import { StateDot } from './StateDot'

const MODES: { id: Mode; label: string }[] = [
  { id: 'offline', label: 'Offline' },
  { id: 'simulate', label: 'Simulate' },
  { id: 'online', label: 'Online' },
]

function ModeSwitch() {
  const mode = useEditor((s) => s.mode)
  const setMode = useEditor((s) => s.setMode)
  return (
    <div role="radiogroup" aria-label="Mode" data-slot="toggle-group" className="flex rounded-control border border-line bg-surface p-0.5">
      {MODES.map((m) => (
        <button
          key={m.id}
          type="button"
          role="radio"
          aria-checked={mode === m.id}
          data-slot="toggle-group-item"
          onClick={() => setMode(m.id)}
          className={`rounded-control px-3 py-1 text-dense font-medium transition-colors ${
            mode === m.id ? 'bg-surface-2 text-text shadow-[inset_0_0_0_1px_var(--border)]' : 'text-text-muted hover:text-text'
          }`}
        >
          {m.label}
        </button>
      ))}
    </div>
  )
}

function DeviceChip() {
  const mode = useEditor((s) => s.mode)
  const project = useEditor((s) => s.project)
  const device = project.devices[0]
  const online = useLive((s) => s.online)
  const stats = useLive((s) => s.stats)
  const profile = resolveDevice(project).device.device
  let state: 'power' | 'idle' | 'alarm' | 'fault' = 'idle'
  let text = 'offline'
  if (mode === 'simulate') {
    state = stats?.running ? 'power' : 'idle'
    text = stats?.running ? 'simulating' : 'starting'
  } else if (mode === 'online') {
    const s = online.state
    state = s === 'online' ? (online.fault ? 'fault' : 'power') : s === 'error' ? 'alarm' : 'idle'
    text = online.fault ? 'PLC STOP' : s === 'online' ? (online.transport === 'fake' ? 'online (demo)' : 'online') : s
    const prog = online.identity?.program
    if (s === 'online' && prog && !online.fault) {
      text = describeProgram(prog).replace(/^program /, '')
      if (prog.state === 'fault') state = 'fault'
      else if (prog.state !== 'run') state = 'alarm'
    }
    if (s === 'online' && online.mismatch?.length) {
      state = 'alarm'
      text = 'wrong device?'
    }
  }
  return (
    <div className="flex h-8 items-center gap-2 rounded-control border border-line bg-surface px-3 text-dense" aria-live="polite">
      <StateDot state={state} />
      <span className="font-medium">{device?.name ?? profile.name}</span>
      <span className="text-text-muted">{text}</span>
    </div>
  )
}

function ThemeMenu() {
  const theme = useEditor((s) => s.theme)
  const setTheme = useEditor((s) => s.setTheme)
  return (
    <DropdownMenu>
      <Tooltip>
        <TooltipTrigger asChild>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="icon" aria-label="Theme">
              <Palette />
            </Button>
          </DropdownMenuTrigger>
        </TooltipTrigger>
        <TooltipContent>Theme</TooltipContent>
      </Tooltip>
      <DropdownMenuContent align="end" className="w-52">
        <DropdownMenuLabel>Theme</DropdownMenuLabel>
        {THEMES.map((t) => (
          <DropdownMenuItem key={t.id} onSelect={() => setTheme(t.id)}>
            <span className="flex-1">
              {t.name}
              <span className="ml-2 text-text-muted">{t.hint}</span>
            </span>
            {theme === t.id && <Check aria-label="current" />}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

function FileMenu() {
  const s = () => useEditor.getState()
  const folderAccess = useProjects((p) => p.folderAccess)
  const recent = useProjects((p) => p.recent)
  const source = useEditor((e) => e.projectSource)
  const currentKey = source?.kind === 'folder' ? source.key : null
  return (
    <DropdownMenu onOpenChange={(o) => o && void refreshLists()}>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="sm" aria-label="File">
          <FileText /> File
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-64">
        {folderAccess ? (
          <>
            <DropdownMenuItem onSelect={() => fileAction(() => newFolderProject())}>New project…</DropdownMenuItem>
            <DropdownMenuItem onSelect={() => fileAction(() => openFolderProject())}>Open project…</DropdownMenuItem>
            <DropdownMenuSub>
              <DropdownMenuSubTrigger disabled={recent.length === 0}>Open recent</DropdownMenuSubTrigger>
              <DropdownMenuSubContent className="w-64">
                {recent.slice(0, 8).map((e) => (
                  <DropdownMenuItem key={e.key} disabled={e.key === currentKey} onSelect={() => fileAction(() => openRecent(e))}>
                    <span className="min-w-0 flex-1 truncate">{e.name || e.folder}</span>
                    <DropdownMenuShortcut>{e.folder}</DropdownMenuShortcut>
                  </DropdownMenuItem>
                ))}
                <DropdownMenuSeparator />
                <DropdownMenuItem onSelect={() => s().setProjectsOpen(true)}>All projects…</DropdownMenuItem>
              </DropdownMenuSubContent>
            </DropdownMenuSub>
          </>
        ) : (
          <DropdownMenuItem onSelect={() => s().setProjectsOpen(true)}>New or open project…</DropdownMenuItem>
        )}
        <DropdownMenuItem onSelect={() => s().setProjectsOpen(true)}>Projects and browser storage…</DropdownMenuItem>
        {folderAccess && source?.kind === 'browser' && (
          <DropdownMenuItem onSelect={() => fileAction(() => saveToFolder())}>Save to folder…</DropdownMenuItem>
        )}
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => s().setDialog({ kind: 'import' })}>
          Import L5X, PLCopen, ST, TwinCAT…
        </DropdownMenuItem>
        <DropdownMenuSub>
          <DropdownMenuSubTrigger>Export project as</DropdownMenuSubTrigger>
          <DropdownMenuSubContent>
            <DropdownMenuItem onSelect={() => void exportRoutine('l5x')}>
              Rockwell L5X <DropdownMenuShortcut>.L5X</DropdownMenuShortcut>
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={() => void exportRoutine('plcopen')}>
              PLCopen XML (IEC ladder) <DropdownMenuShortcut>.xml</DropdownMenuShortcut>
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={() => void exportRoutine('st')}>
              Structured Text (IEC) <DropdownMenuShortcut>.st</DropdownMenuShortcut>
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={() => downloadZip(s().project)}>
              plcc studio project <DropdownMenuShortcut>.zip</DropdownMenuShortcut>
            </DropdownMenuItem>
          </DropdownMenuSubContent>
        </DropdownMenuSub>
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => void closeProject()}>Close project</DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

export function TopBar() {
  const project = useEditor((s) => s.project)
  const view = useEditor((s) => s.view)
  const openProjects = useEditor((s) => s.setProjectsOpen)
  const source = useEditor((s) => s.projectSource)
  const crumbs: string[] = []
  if (view.kind === 'routine') crumbs.push(view.program, view.routine)
  else if (view.kind === 'tags') crumbs.push('Tags')
  else if (view.kind === 'io') crumbs.push('I/O mapping', view.device)
  else if (view.kind === 'tasks') crumbs.push('Tasks')
  return (
    <header className="flex items-center gap-4 border-b border-line bg-nav px-3">
      <div className="flex w-[208px] items-center gap-2">
        <Logo />
        <span className="font-semibold tracking-tight">plcc studio</span>
      </div>
      <FileMenu />
      <nav aria-label="Breadcrumb" className="flex min-w-0 flex-1 items-center gap-1 text-dense">
        <button
          type="button"
          data-testid="project-crumb"
          className="flex min-w-0 items-center gap-1.5 rounded-control px-1.5 py-0.5 hover:bg-surface-2"
          onClick={() => openProjects(true)}
          title={source?.kind === 'folder' ? `Folder “${source.folder}” on disk` : 'In browser storage (no folder)'}
        >
          {source?.kind === 'folder' ? (
            <>
              <Folder className="size-3.5 shrink-0 text-text-muted" aria-hidden />
              <span className="truncate text-text-muted">{source.folder}</span>
              <ChevronRight className="size-3.5 shrink-0 text-text-muted" aria-hidden />
            </>
          ) : (
            <HardDrive className="size-3.5 shrink-0 text-text-muted" aria-label="Browser storage" />
          )}
          <span className="truncate font-medium">{project.name}</span>
        </button>
        {crumbs.map((c, i) => (
          <span key={i} className="flex min-w-0 items-center gap-1 text-text-muted">
            <ChevronRight className="size-3.5 shrink-0" aria-hidden />
            <span className={`truncate ${i === crumbs.length - 1 ? 'text-text' : ''}`}>{c}</span>
          </span>
        ))}
      </nav>
      <ModeSwitch />
      <DeviceChip />
      <ThemeMenu />
      <Tooltip>
        <TooltipTrigger asChild>
          <Button onClick={() => useEditor.getState().setDialog({ kind: 'download' })}>
            <Download />
            Download
          </Button>
        </TooltipTrigger>
        <TooltipContent>Compile for the device and flash it over USB, from the browser</TooltipContent>
      </Tooltip>
    </header>
  )
}
