// SPDX-License-Identifier: MPL-2.0
import { useMemo, useState } from 'react'
import {
  Command, CommandDialog, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList, CommandShortcut,
} from '@/components/ui/command'
import { PALETTE, parseQuickEntry, printRung } from '@/model'
import { addRung, addRungFromQuickEntry, deleteSelection, insertInstruction, wrapSelectionInBranch } from '@/state/commands'
import { useEditor, type Mode } from '@/state/editor'
import { downloadZip } from '@/state/persistence'
import { THEMES } from '@/state/theme'

/** Command palette (Ctrl+K). Typing instructions (`XIC Start OTE Motor`) offers to add them as a rung. */
export function CommandPalette() {
  const open = useEditor((s) => s.paletteOpen)
  const setOpen = useEditor((s) => s.setPaletteOpen)
  const project = useEditor((s) => s.project)
  const [query, setQuery] = useState('')

  const quick = useMemo(() => {
    const q = query.trim()
    if (!q || (!q.includes(' ') && !q.includes('('))) return null
    try {
      return printRung(parseQuickEntry(q))
    } catch {
      return null
    }
  }, [query])

  const run = (fn: () => void) => {
    setOpen(false)
    setQuery('')
    fn()
    requestAnimationFrame(() => document.getElementById('rung-list')?.focus())
  }
  const s = () => useEditor.getState()
  const inRoutine = useEditor((st) => st.view.kind === 'routine')

  return (
    <CommandDialog
      open={open}
      onOpenChange={(o) => {
        setOpen(o)
        if (!o) setQuery('')
      }}
      title="Command palette"
      description="Insert instructions, add rungs, navigate, switch modes"
    >
      <Command loop shouldFilter={true}>
        <CommandInput placeholder="Instruction, command, or XIC Start XIO Stop OTE Motor" value={query} onValueChange={setQuery} />
        <CommandList className="max-h-[420px]">
          <CommandEmpty>No match. Instructions separated by spaces add a rung.</CommandEmpty>
          {quick && inRoutine && (
            <CommandGroup heading="Quick entry">
              <CommandItem value={`__quick ${query}`} onSelect={() => run(() => addRungFromQuickEntry(query))} forceMount>
                Add rung <span className="text-mono text-text-muted">{quick}</span>
              </CommandItem>
            </CommandGroup>
          )}
          {inRoutine && (
            <CommandGroup heading="Insert instruction">
              {PALETTE.map((p) => (
                <CommandItem key={p.mnemonic} value={`insert ${p.mnemonic} ${p.label} ${p.group}`} onSelect={() => run(() => insertInstruction(p.mnemonic))}>
                  <span className="w-12 text-mono">{p.mnemonic}</span>
                  <span>{p.label}</span>
                  <CommandShortcut>{p.group}</CommandShortcut>
                </CommandItem>
              ))}
            </CommandGroup>
          )}
          {inRoutine && (
            <CommandGroup heading="Edit">
              <CommandItem value="new rung add" onSelect={() => run(() => addRung())}>
                New rung <CommandShortcut>N</CommandShortcut>
              </CommandItem>
              <CommandItem value="branch parallel wrap" onSelect={() => run(wrapSelectionInBranch)}>
                Add parallel branch around selection <CommandShortcut>P</CommandShortcut>
              </CommandItem>
              <CommandItem value="delete remove selection" onSelect={() => run(deleteSelection)}>
                Delete selection <CommandShortcut>Del</CommandShortcut>
              </CommandItem>
              <CommandItem value="undo" onSelect={() => run(() => s().undo())}>
                Undo <CommandShortcut>Ctrl Z</CommandShortcut>
              </CommandItem>
              <CommandItem value="redo" onSelect={() => run(() => s().redo())}>
                Redo <CommandShortcut>Ctrl Y</CommandShortcut>
              </CommandItem>
            </CommandGroup>
          )}
          <CommandGroup heading="Go to">
            {project.programs.flatMap((p) =>
              p.routines.map((r) => (
                <CommandItem
                  key={`${p.name}/${r.name}`}
                  value={`go routine ${p.name} ${r.name}`}
                  onSelect={() => run(() => s().setView({ kind: 'routine', program: p.name, routine: r.name }))}
                >
                  {p.name} / {r.name}
                </CommandItem>
              )),
            )}
            <CommandItem value="go tags" onSelect={() => run(() => s().setView({ kind: 'tags' }))}>
              Tags
            </CommandItem>
            <CommandItem value="go io mapping devices" onSelect={() => run(() => s().setView({ kind: 'io', device: project.devices[0]?.name ?? '' }))}>
              I/O mapping
            </CommandItem>
            <CommandItem value="go tasks" onSelect={() => run(() => s().setView({ kind: 'tasks' }))}>
              Tasks
            </CommandItem>
          </CommandGroup>
          <CommandGroup heading="Mode">
            {(['offline', 'simulate', 'online'] as Mode[]).map((m) => (
              <CommandItem key={m} value={`mode ${m}`} onSelect={() => run(() => s().setMode(m))}>
                {m[0].toUpperCase() + m.slice(1)}
              </CommandItem>
            ))}
          </CommandGroup>
          <CommandGroup heading="Project">
            <CommandItem value="projects open new rename delete import" onSelect={() => run(() => s().setProjectsOpen(true))}>
              Projects…
            </CommandItem>
            <CommandItem value="export zip download project" onSelect={() => run(() => downloadZip(s().project))}>
              Export project as .zip
            </CommandItem>
          </CommandGroup>
          <CommandGroup heading="Theme">
            {THEMES.map((t) => (
              <CommandItem key={t.id} value={`theme ${t.name}`} onSelect={() => run(() => s().setTheme(t.id))}>
                {t.name} <CommandShortcut>{t.hint}</CommandShortcut>
              </CommandItem>
            ))}
          </CommandGroup>
        </CommandList>
      </Command>
    </CommandDialog>
  )
}
