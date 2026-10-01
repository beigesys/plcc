// SPDX-License-Identifier: MPL-2.0
import { useEffect, useRef, useState, type ReactNode } from 'react'
import { ArrowUpCircle, Cable, Cpu, FileCode2, FolderOpen, Plus, Rows3, Tags, Timer, Trash2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { catalogUpdates, removeDevice, resolveDevice } from '@/devices/project'
import { isStRoutine, newRoutine, type RoutineKind } from '@/model'
import { useEditor, type View } from '@/state/editor'
import { useLive } from '@/state/live'
import { countBySeverity, useProblems } from '@/state/problems'
import { deleteRoutine, renameRoutine, type Target } from '@/state/registry'
import { WithCommandMenu } from './CommandMenu'
import { AddDeviceDialog } from './DeviceDialogs'
import { StateDot } from './StateDot'

function sameView(a: View, b: View): boolean {
  if (a.kind !== b.kind) return false
  if (a.kind === 'routine' && b.kind === 'routine') return a.program === b.program && a.routine === b.routine
  if (a.kind === 'io' && b.kind === 'io') return a.device === b.device
  return true
}

function Item({
  view, icon, children, trailing, menu, label,
}: { view: View; icon: ReactNode; children: ReactNode; trailing?: ReactNode; menu?: Target; label?: string }) {
  const current = useEditor((s) => s.view)
  const setView = useEditor((s) => s.setView)
  const active = sameView(current, view)
  const button = (
      <button
        type="button"
        aria-current={active ? 'page' : undefined}
        onClick={() => setView(view)}
        className={`flex min-w-0 flex-1 items-center gap-2 rounded-control px-2 py-1.5 text-left text-dense ${
          active ? 'bg-surface-2 text-text shadow-[inset_0_0_0_1px_var(--border)]' : 'text-text-muted hover:bg-surface hover:text-text'
        }`}
      >
        <span className="text-text-muted [&_svg]:size-4" aria-hidden>
          {icon}
        </span>
        <span className="truncate">{children}</span>
      </button>
  )
  return (
    <li className="group flex items-center">
      {menu ? (
        <WithCommandMenu target={menu} label={label}>
          {button}
        </WithCommandMenu>
      ) : (
        button
      )}
      {trailing}
    </li>
  )
}

function RenameRoutine({ program, routine }: { program: string; routine: string }) {
  const [name, setName] = useState(routine)
  const [error, setError] = useState('')
  const ref = useRef<HTMLInputElement>(null)
  useEffect(() => {
    ref.current?.focus()
    ref.current?.select()
  }, [])
  const done = () => useEditor.getState().setFocus(null)
  return (
    <li>
      <form
        className="space-y-1 rounded-control border border-line bg-surface p-1.5"
        onSubmit={(e) => {
          e.preventDefault()
          const err = renameRoutine(program, routine, name)
          if (err) setError(err)
          else done()
        }}
      >
        <label htmlFor="rename-routine" className="sr-only">
          New name for {routine}
        </label>
        <Input id="rename-routine" ref={ref} value={name} onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.key === 'Escape' && done()} onBlur={done} className="h-7" />
        {error && <p className="text-dense text-alarm">{error}</p>}
      </form>
    </li>
  )
}

function Section({ title, children, action }: { title: string; children: ReactNode; action?: ReactNode }) {
  return (
    <section className="mb-4">
      <div className="mb-1 flex items-center justify-between px-2">
        <h2 className="text-[11px] font-semibold tracking-wider text-text-muted uppercase">{title}</h2>
        {action}
      </div>
      <ul className="space-y-0.5">{children}</ul>
    </section>
  )
}

function NewRoutineForm({ program, onDone }: { program: string; onDone(): void }) {
  const [name, setName] = useState('')
  const [kind, setKind] = useState<RoutineKind>('ladder')
  const [error, setError] = useState('')
  const submit = () => {
    const n = name.trim()
    const s = useEditor.getState()
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(n)) return setError('Use letters, digits and _')
    if (s.project.pous.some((p) => p.routines.some((r) => r.name.toLowerCase() === n.toLowerCase()))) {
      return setError('A routine with that name exists')
    }
    s.commit((p) => ({
      ...p,
      pous: p.pous.map((pr) => (pr.name === program ? { ...pr, routines: [...pr.routines, newRoutine(n, kind)] } : pr)),
    }))
    s.setView({ kind: 'routine', program, routine: n })
    onDone()
  }
  return (
    <form
      className="space-y-1.5 rounded-control border border-line bg-surface p-2"
      onSubmit={(e) => {
        e.preventDefault()
        submit()
      }}
    >
      <label className="sr-only" htmlFor="new-routine">
        Routine name
      </label>
      <Input id="new-routine" autoFocus placeholder="RoutineName" value={name} onChange={(e) => setName(e.target.value)} className="h-7" />
      <div className="flex gap-1" role="radiogroup" aria-label="Routine language">
        {(['ladder', 'st'] as const).map((k) => (
          <button
            key={k}
            type="button"
            role="radio"
            aria-checked={kind === k}
            onClick={() => setKind(k)}
            className={`flex-1 rounded-control border px-2 py-0.5 text-dense ${kind === k ? 'border-text-muted text-text' : 'border-line text-text-muted'}`}
          >
            {k === 'ladder' ? 'Ladder' : 'ST'}
          </button>
        ))}
      </div>
      {error && <p className="text-dense text-alarm">{error}</p>}
      <div className="flex justify-end gap-1">
        <Button type="button" size="xs" variant="ghost" onClick={onDone}>
          Cancel
        </Button>
        <Button type="submit" size="xs">
          Add
        </Button>
      </div>
    </form>
  )
}

export function LeftNav() {
  const project = useEditor((s) => s.project)
  const mode = useEditor((s) => s.mode)
  const onlineState = useLive((s) => s.online.state)
  const simRunning = useLive((s) => s.stats?.running ?? false)
  const openProjects = useEditor((s) => s.setProjectsOpen)
  const [adding, setAdding] = useState<string | null>(null)
  const [addingDevice, setAddingDevice] = useState(false)
  const updates = catalogUpdates(project)
  const focus = useEditor((s) => s.focus)
  const problems = useProblems((s) => s.problems)
  const routineCount = (program: string, routine: string) =>
    countBySeverity(problems.filter((p) => p.place.kind === 'element' && p.place.program === program && p.place.routine === routine))

  return (
    <aside aria-label="Project navigation" className="flex min-h-0 flex-col border-r border-line bg-nav">
      <div className="min-h-0 flex-1 overflow-y-auto px-2 py-3">
        <Section
          title="Devices"
          action={
            <Button variant="ghost" size="icon-xs" aria-label="Add device" onClick={() => setAddingDevice(true)}>
              <Plus />
            </Button>
          }
        >
          {project.devices.map((d, i) => {
            const state =
              i > 0 ? 'idle' : mode === 'simulate' && simRunning ? 'power' : mode === 'online' ? (onlineState === 'online' ? 'power' : onlineState === 'error' ? 'alarm' : 'idle') : 'idle'
            const r = resolveDevice(project, d.name)
            const update = updates.find((u) => u.path === d.manifest)
            return (
              <Item
                key={d.name}
                view={{ kind: 'io', device: d.name }}
                icon={<Cpu />}
                menu={{ kind: 'device', device: d.name }}
                label={`Device ${d.name}`}
                trailing={
                  <span className="flex items-center gap-1 pr-2">
                    {update && (
                      <span title={`Catalog has version ${update.latest.device.device.version} of this manifest`} aria-label="Manifest update available">
                        <ArrowUpCircle className="size-3.5 text-text-muted" />
                      </span>
                    )}
                    {project.devices.length > 1 && (
                      <Button
                        variant="ghost"
                        size="icon-xs"
                        className="opacity-0 group-hover:opacity-100 focus-visible:opacity-100"
                        aria-label={`Remove device ${d.name}`}
                        onClick={() => {
                          const s = useEditor.getState()
                          s.commit((p) => removeDevice(p, d.name))
                          s.notify(`Removed ${d.name}; its tags keep their addresses`)
                        }}
                      >
                        <Trash2 />
                      </Button>
                    )}
                    <StateDot state={r.problem ? 'alarm' : state} />
                  </span>
                }
              >
                {d.name} <span className="text-text-muted">· {r.device.device.name}</span>
              </Item>
            )
          })}
        </Section>
        <AddDeviceDialog open={addingDevice} onOpenChange={setAddingDevice} />

        {project.pous.map((prog) => (
          <Section
            key={prog.name}
            title={prog.name}
            action={
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label={`Add routine to ${prog.name}`}
                onClick={() => setAdding(adding === prog.name ? null : prog.name)}
              >
                <Plus />
              </Button>
            }
          >
            {prog.routines.map((r, ri) => {
              if (focus?.kind === 'renameRoutine' && focus.program === prog.name && focus.routine === r.name) {
                return <RenameRoutine key={r.name} program={prog.name} routine={r.name} />
              }
              const c = routineCount(prog.name, r.name)
              return (
              <Item
                key={r.name}
                view={{ kind: 'routine', program: prog.name, routine: r.name }}
                icon={isStRoutine(r) ? <FileCode2 /> : <Rows3 />}
                menu={{ kind: 'routine', program: prog.name, routine: r.name }}
                label={`Routine ${r.name}`}
                trailing={
                  prog.routines.length > 1 ? (
                    <Button
                      variant="ghost"
                      size="icon-xs"
                      className="opacity-0 group-hover:opacity-100 focus-visible:opacity-100"
                      aria-label={`Delete routine ${r.name}`}
                      onClick={() => deleteRoutine(prog.name, r.name)}
                    >
                      <Trash2 />
                    </Button>
                  ) : undefined
                }
              >
                {r.name}
                {ri === 0 && <span className="ml-1.5 text-[11px] text-text-muted">main</span>}
                {c.errors > 0 && (
                  <span className="ml-1.5 rounded-control bg-fault/15 px-1 text-[11px] text-fault" aria-label={`${c.errors} errors`}>
                    {c.errors}
                  </span>
                )}
                {c.errors === 0 && c.warnings > 0 && (
                  <span className="ml-1.5 rounded-control bg-alarm-bg px-1 text-[11px] text-alarm" aria-label={`${c.warnings} warnings`}>
                    {c.warnings}
                  </span>
                )}
              </Item>
              )
            })}
            {adding === prog.name && (
              <li>
                <NewRoutineForm program={prog.name} onDone={() => setAdding(null)} />
              </li>
            )}
          </Section>
        ))}

        <Section title="Project">
          <Item view={{ kind: 'tags' }} icon={<Tags />}>
            Tags <span className="text-text-muted">· {project.globals.length}</span>
          </Item>
          <Item view={{ kind: 'io', device: project.devices[0]?.name ?? '' }} icon={<Cable />}>
            I/O mapping
          </Item>
          <Item view={{ kind: 'tasks' }} icon={<Timer />}>
            Tasks
          </Item>
        </Section>
      </div>
      <div className="border-t border-line p-2">
        <Button variant="ghost" className="w-full justify-start text-text-muted" onClick={() => openProjects(true)}>
          <FolderOpen />
          Projects
        </Button>
      </div>
    </aside>
  )
}
