// SPDX-License-Identifier: MPL-2.0
import { ChevronRight, Download, Palette, Check } from 'lucide-react'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { resolveDevice } from '@/devices/project'
import { useEditor, type Mode } from '@/state/editor'
import { useLive } from '@/state/live'
import { THEMES } from '@/state/theme'
import { Logo } from './Logo'
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

export function TopBar() {
  const project = useEditor((s) => s.project)
  const view = useEditor((s) => s.view)
  const openProjects = useEditor((s) => s.setProjectsOpen)
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
      <nav aria-label="Breadcrumb" className="flex min-w-0 flex-1 items-center gap-1 text-dense">
        <button
          type="button"
          className="truncate rounded-control px-1.5 py-0.5 font-medium hover:bg-surface-2"
          onClick={() => openProjects(true)}
          title="Projects"
        >
          {project.name}
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
          {/* A disabled button gets no pointer events; the span carries the tooltip. */}
          <span tabIndex={0} aria-label="Download (coming soon: compiles and flashes from the browser)">
            <Button disabled className="pointer-events-none">
              <Download />
              Download
            </Button>
          </span>
        </TooltipTrigger>
        <TooltipContent>Coming soon: runs in your browser, no install</TooltipContent>
      </Tooltip>
    </header>
  )
}
