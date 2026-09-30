// SPDX-License-Identifier: MPL-2.0
import { useEffect } from 'react'
import { useEditor } from '@/state/editor'
import { startSimulator, stopSimulator, simSend } from '@/state/live'
import { disconnectOnline, setOnlineProject } from '@/state/online'
import { initPersistence } from '@/state/persistence'
import { CommandPalette } from '@/ui/CommandPalette'
import { Inspector } from '@/ui/Inspector'
import { LeftNav } from '@/ui/LeftNav'
import { MainView } from '@/ui/MainView'
import { ProjectsDialog } from '@/ui/ProjectsDialog'
import { StatusBar } from '@/ui/StatusBar'
import { TopBar } from '@/ui/TopBar'

function isTyping(t: EventTarget | null): boolean {
  const el = t as HTMLElement | null
  if (!el) return false
  return el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.tagName === 'SELECT' || el.isContentEditable
}

export function App() {
  const mode = useEditor((s) => s.mode)
  const projectId = useEditor((s) => s.projectId)
  const profileId = useEditor((s) => s.project.devices[0]?.profile)

  useEffect(() => {
    initPersistence().catch((e: unknown) => useEditor.getState().notify(`Could not open projects: ${String(e)}`, 'fault'))
  }, [])

  // Simulator lifecycle follows the mode and the open project.
  useEffect(() => {
    if (mode !== 'simulate') return
    const { project } = useEditor.getState()
    startSimulator(project, project.devices[0]?.profile ?? 'simulator', project.tasks[0]?.intervalMs ?? 10)
    let timer: ReturnType<typeof setTimeout> | undefined
    let prev = project
    const unsub = useEditor.subscribe((s) => {
      if (s.project === prev) return
      prev = s.project
      clearTimeout(timer)
      timer = setTimeout(() => {
        simSend({ type: 'project', project: s.project })
        simSend({ type: 'period', periodMs: s.project.tasks[0]?.intervalMs ?? 10 })
      }, 80)
    })
    return () => {
      unsub()
      clearTimeout(timer)
      stopSimulator()
    }
  }, [mode, projectId, profileId])

  useEffect(() => {
    if (mode !== 'online') return
    setOnlineProject(useEditor.getState().project)
    const unsub = useEditor.subscribe((s) => setOnlineProject(s.project))
    return () => {
      unsub()
      void disconnectOnline()
    }
  }, [mode, projectId])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const s = useEditor.getState()
      const mod = e.ctrlKey || e.metaKey
      if (mod && e.key.toLowerCase() === 'k') {
        e.preventDefault()
        s.setPaletteOpen(!s.paletteOpen)
        return
      }
      if (isTyping(e.target)) return
      if (mod && !e.shiftKey && e.key.toLowerCase() === 'z') {
        e.preventDefault()
        s.undo()
      } else if (mod && (e.key.toLowerCase() === 'y' || (e.shiftKey && e.key.toLowerCase() === 'z'))) {
        e.preventDefault()
        s.redo()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  return (
    <div className="grid h-full min-w-[1100px] grid-rows-[52px_minmax(0,1fr)_30px] bg-bg text-text">
      <TopBar />
      <div className="grid min-h-0 grid-cols-[232px_minmax(0,1fr)_300px]">
        <LeftNav />
        <MainView />
        <Inspector />
      </div>
      <StatusBar />
      <CommandPalette />
      <ProjectsDialog />
    </div>
  )
}
