// SPDX-License-Identifier: MPL-2.0
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import type { Task } from '@/model'
import { useEditor } from '@/state/editor'
import { useDraft } from './useDraft'

function updateTask(name: string, patch: Partial<Task>) {
  useEditor.getState().commit((p) => ({ ...p, tasks: p.tasks.map((t) => (t.name === name ? { ...t, ...patch } : t)) }))
}

function IntervalInput({ task }: { task: Task }) {
  const [v, setV] = useDraft(String(task.intervalMs))
  const commit = () => {
    const n = Number(v)
    if (Number.isFinite(n) && n >= 1 && n <= 60000) updateTask(task.name, { intervalMs: Math.round(n) })
    else setV(String(task.intervalMs))
  }
  return (
    <input
      aria-label={`${task.name} interval in milliseconds`}
      inputMode="numeric"
      value={v}
      onChange={(e) => setV(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => e.key === 'Enter' && (e.target as HTMLInputElement).blur()}
      className="h-7 w-24 rounded-control border border-line bg-bg px-2 text-mono outline-none focus-visible:border-text-muted"
    />
  )
}

export function TasksView() {
  const tasks = useEditor((s) => s.project.tasks)
  const programs = useEditor((s) => s.project.programs)
  return (
    <div className="min-h-0 flex-1 overflow-y-auto">
      <div className="border-b border-line bg-surface px-4 py-3">
        <h1 className="text-base font-semibold">Tasks</h1>
        <p className="text-dense text-text-muted">
          Cyclic tasks and the programs they run. The preview simulator scans at the first task's interval; each program runs its main routine.
        </p>
      </div>
      <div className="p-4">
        <div className="rounded-lg border border-line bg-surface">
          <Table className="text-dense">
            <TableHeader>
              <TableRow>
                <TableHead>Task</TableHead>
                <TableHead>Interval (ms)</TableHead>
                <TableHead>Programs</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {tasks.map((t) => (
                <TableRow key={t.name}>
                  <TableCell className="text-mono">{t.name}</TableCell>
                  <TableCell>
                    <IntervalInput task={t} />
                  </TableCell>
                  <TableCell>
                    <div className="flex flex-wrap gap-3">
                      {programs.map((p) => {
                        const on = t.programs.includes(p.name)
                        return (
                          <label key={p.name} className="flex items-center gap-1.5">
                            <input
                              type="checkbox"
                              checked={on}
                              onChange={() =>
                                updateTask(t.name, { programs: on ? t.programs.filter((x) => x !== p.name) : [...t.programs, p.name] })
                              }
                              className="accent-[var(--text)]"
                            />
                            <span className="text-mono">{p.name}</span>
                          </label>
                        )
                      })}
                    </div>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      </div>
    </div>
  )
}
