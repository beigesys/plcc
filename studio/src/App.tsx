// SPDX-License-Identifier: MPL-2.0
import { useEffect } from 'react'
import { primaryDevice } from '@/devices/project'
import { useEditor } from '@/state/editor'
import { startSimulator, stopSimulator, simSend } from '@/state/live'
import { disconnectOnline, setOnlineProject } from '@/state/online'
import { initPersistence } from '@/state/persistence'
import { runCheck, scheduleCheck } from '@/state/problems'
import { buildForSimulator, resetSimBuild } from '@/state/simulate'
import { DialogHost } from '@/ui/Dialogs'
import { ProblemsPanel } from '@/ui/ProblemsPanel'
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
  // The simulator restarts when the controller's manifest changes (image sizes).
  const deviceText = useEditor((s) => s.project.deviceFiles[s.project.devices[0]?.manifest ?? ''])

  useEffect(() => {
    initPersistence().catch((e: unknown) => useEditor.getState().notify(`Could not open projects: ${String(e)}`, 'fault'))
  }, [])

  // Simulator lifecycle follows the mode and the open project.
  useEffect(() => {
    if (mode !== 'simulate') return
    const { project } = useEditor.getState()
    // The preview engine runs at once; plcc's build replaces it when it is ready.
    startSimulator(project, primaryDevice(project), project.tasks[0]?.interval_ms ?? 10)
    void buildForSimulator(project, primaryDevice(project))
    let timer: ReturnType<typeof setTimeout> | undefined
    let rebuild: ReturnType<typeof setTimeout> | undefined
    let prev = project
    const unsub = useEditor.subscribe((s) => {
      if (s.project === prev) return
      prev = s.project
      clearTimeout(timer)
      timer = setTimeout(() => {
        simSend({ type: 'project', project: s.project })
        simSend({ type: 'period', periodMs: s.project.tasks[0]?.interval_ms ?? 10 })
      }, 80)
      // Edits rebuild the program once typing settles (a cold restart, forces kept).
      clearTimeout(rebuild)
      rebuild = setTimeout(() => void buildForSimulator(s.project, primaryDevice(s.project)), 700)
    })
    return () => {
      unsub()
      clearTimeout(timer)
      clearTimeout(rebuild)
      resetSimBuild()
      stopSimulator()
    }
  }, [mode, projectId, deviceText])

  // plcc checks the project after edits settle (in a worker).
  useEffect(() => {
    void runCheck(useEditor.getState().project)
    let prev = useEditor.getState().project
    return useEditor.subscribe((s) => {
      if (s.project === prev) return
      prev = s.project
      scheduleCheck(s.project)
    })
  }, [projectId])

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
    <div className="grid h-full min-w-[1100px] grid-rows-[52px_minmax(0,1fr)_auto_30px] bg-bg text-text">
      <TopBar />
      <div className="grid min-h-0 grid-cols-[232px_minmax(0,1fr)_300px]">
        <LeftNav />
        <MainView />
        <Inspector />
      </div>
      <ProblemsPanel />
      <StatusBar />
      <CommandPalette />
      <ProjectsDialog />
      <DialogHost />
    </div>
  )
}
