// SPDX-License-Identifier: MPL-2.0
// Context menus drawn from the command registry: every item runs the same
// command as its keyboard shortcut and its palette entry, and shows that
// shortcut.

import type { ReactNode } from 'react'
import {
  ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuLabel, ContextMenuSeparator, ContextMenuShortcut, ContextMenuSub,
  ContextMenuSubContent, ContextMenuSubTrigger, ContextMenuTrigger,
} from '@/components/ui/context-menu'
import { isEnabled, menuFor, type MenuEntry, type Target } from '@/state/registry'

function Entries({ entries, target }: { entries: MenuEntry[]; target: Target }) {
  return (
    <>
      {entries.map((e, i) => {
        if ('separator' in e) return <ContextMenuSeparator key={`s${i}`} />
        if ('submenu' in e) {
          if (!e.items.length) return null
          return (
            <ContextMenuSub key={e.submenu}>
              <ContextMenuSubTrigger>{e.submenu}</ContextMenuSubTrigger>
              <ContextMenuSubContent className="max-h-[60vh] overflow-y-auto">
                <Entries entries={e.items} target={target} />
              </ContextMenuSubContent>
            </ContextMenuSub>
          )
        }
        const c = e.command
        return (
          <ContextMenuItem key={c.id} data-command={c.id} disabled={!isEnabled(c, target)} onSelect={() => c.run(target)}>
            {c.title}
            {c.shortcut && <ContextMenuShortcut>{c.shortcut}</ContextMenuShortcut>}
          </ContextMenuItem>
        )
      })}
    </>
  )
}

export function CommandMenuContent({ entries, target, label }: { entries: MenuEntry[]; target: Target; label?: string }) {
  if (!entries.length) return <ContextMenuContent className="hidden" />
  return (
    <ContextMenuContent className="min-w-56" aria-label={label}>
      {label && <ContextMenuLabel className="text-[11px] text-text-muted">{label}</ContextMenuLabel>}
      <Entries entries={entries} target={target} />
    </ContextMenuContent>
  )
}

/** Wraps `children` with the context menu of `target` (tags, routines, devices). */
export function WithCommandMenu({ target, label, children }: { target: Target; label?: string; children: ReactNode }) {
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>{children}</ContextMenuTrigger>
      <CommandMenuContent entries={menuFor(target)} target={target} label={label} />
    </ContextMenu>
  )
}
