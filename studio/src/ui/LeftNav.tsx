// SPDX-License-Identifier: MPL-2.0
import { useState, type ReactNode } from 'react'
import { Cable, Cpu, FileCode2, FolderOpen, Plus, Rows3, Tags, Timer, Trash2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { getProfile } from '@/devices/profiles'
import type { RoutineKind } from '@/model'
import { findProgram, useEditor, type View } from '@/state/editor'
import { useLive } from '@/state/live'
import { StateDot } from './StateDot'

function sameView(a: View, b: View): boolean {
  if (a.kind !== b.kind) return false
  if (a.kind === 'routine' && b.kind === 'routine') return a.program === b.program && a.routine === b.routine
  if (a.kind === 'io' && b.kind === 'io') return a.device === b.device
  return true
}

function Item({ view, icon, children, trailing }: { view: View; icon: ReactNode; children: ReactNode; trailing?: ReactNode }) {
  const current = useEditor((s) => s.view)
  const setView = useEditor((s) => s.setView)
  const active = sameView(current, view)
  return (
    <li className="group flex items-center">
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
      {trailing}
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
    if (s.project.programs.some((p) => p.routines.some((r) => r.name.toLowerCase() === n.toLowerCase()))) {
      return setError('A routine with that name exists')
    }
    s.commit((p) => ({
      ...p,
      programs: p.programs.map((pr) =>
        pr.name === program ? { ...pr, routines: [...pr.routines, { name: n, kind, rungs: [], st: kind === 'st' ? '' : undefined }] } : pr,
      ),
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

  const deleteRoutine = (program: string, routine: string) => {
    const s = useEditor.getState()
    const prog = findProgram(s.project, program)
    if (!prog || prog.routines.length <= 1) return s.notify('A program needs at least one routine', 'alarm')
    s.commit((p) => ({
      ...p,
      programs: p.programs.map((pr) =>
        pr.name === program
          ? {
              ...pr,
              routines: pr.routines.filter((r) => r.name !== routine),
              main: pr.main === routine ? (pr.routines.find((r) => r.name !== routine)?.name ?? '') : pr.main,
            }
          : pr,
      ),
    }))
    const next = findProgram(useEditor.getState().project, program)?.routines[0]
    if (next) s.setView({ kind: 'routine', program, routine: next.name })
  }

  return (
    <aside aria-label="Project navigation" className="flex min-h-0 flex-col border-r border-line bg-nav">
      <div className="min-h-0 flex-1 overflow-y-auto px-2 py-3">
        <Section title="Devices">
          {project.devices.map((d) => {
            const state =
              mode === 'simulate' && simRunning ? 'power' : mode === 'online' ? (onlineState === 'online' ? 'power' : onlineState === 'error' ? 'alarm' : 'idle') : 'idle'
            return (
              <Item
                key={d.name}
                view={{ kind: 'io', device: d.name }}
                icon={<Cpu />}
                trailing={
                  <span className="pr-2">
                    <StateDot state={state} />
                  </span>
                }
              >
                {d.name} <span className="text-text-muted">· {getProfile(d.profile).name}</span>
              </Item>
            )
          })}
        </Section>

        {project.programs.map((prog) => (
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
            {prog.routines.map((r) => (
              <Item
                key={r.name}
                view={{ kind: 'routine', program: prog.name, routine: r.name }}
                icon={r.kind === 'st' ? <FileCode2 /> : <Rows3 />}
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
                {prog.main === r.name && <span className="ml-1.5 text-[11px] text-text-muted">main</span>}
              </Item>
            ))}
            {adding === prog.name && (
              <li>
                <NewRoutineForm program={prog.name} onDone={() => setAdding(null)} />
              </li>
            )}
          </Section>
        ))}

        <Section title="Project">
          <Item view={{ kind: 'tags' }} icon={<Tags />}>
            Tags <span className="text-text-muted">· {project.tags.length}</span>
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
