// SPDX-License-Identifier: MPL-2.0
import { useState } from 'react'
import { Plug, PlugZap, Unplug } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { useEditor } from '@/state/editor'
import { useLive } from '@/state/live'
import { connectOnline, disconnectOnline, onlineSupported } from '@/state/online'
import { IoMappingView } from './IoMappingView'
import { RoutineView } from './RoutineView'
import { TagsView } from './TagsView'
import { TasksView } from './TasksView'

function OnlineBanner() {
  const online = useLive((s) => s.online)
  const project = useEditor((s) => s.project)
  const [busy, setBusy] = useState(false)
  const profile = project.devices[0]?.profile ?? 'arduino-opta'
  const connected = online.state === 'online' || online.state === 'error'
  const connect = async (kind: 'webserial' | 'fake') => {
    setBusy(true)
    try {
      await connectOnline(project, profile, kind)
    } finally {
      setBusy(false)
    }
  }
  return (
    <div
      className={`flex flex-wrap items-center gap-3 border-b px-4 py-2 text-dense ${
        online.state === 'error' || online.fault ? 'border-alarm-border bg-alarm-bg text-alarm' : 'border-line bg-surface'
      }`}
      role="region"
      aria-label="Online connection"
    >
      {!connected ? (
        <>
          <Plug className="size-4 text-text-muted" aria-hidden />
          <span>
            Online watches a device over its USB console (<span className="text-mono">img</span> every 100 ms). Values come from %I/%Q/%M addresses.
          </span>
          {online.error && <span className="text-fault">{online.error}</span>}
          <span className="ml-auto flex gap-2">
            <Button size="sm" onClick={() => connect('webserial')} disabled={busy || !onlineSupported()} title={onlineSupported() ? undefined : 'WebSerial needs Chrome or Edge'}>
              <PlugZap /> Connect USB…
            </Button>
            <Button size="sm" variant="outline" onClick={() => connect('fake')} disabled={busy}>
              Demo device
            </Button>
          </span>
        </>
      ) : (
        <>
          <PlugZap className="size-4" aria-hidden />
          <span>
            {online.transport === 'fake' ? 'Demo device (fake Opta console running the preview simulator)' : 'Connected over USB serial'}
            {online.state === 'error' && ` — ${online.error ?? 'device not responding'}`}
            {online.fault && ` — ${online.fault}`}
          </span>
          <span className="text-text-muted">Tags without an address show no value online.</span>
          <Button size="sm" variant="outline" className="ml-auto" onClick={() => void disconnectOnline()}>
            <Unplug /> Disconnect
          </Button>
        </>
      )}
    </div>
  )
}

export function MainView() {
  const view = useEditor((s) => s.view)
  const mode = useEditor((s) => s.mode)
  return (
    <main className="flex min-h-0 min-w-0 flex-col bg-bg" aria-label="Editor">
      {mode === 'online' && <OnlineBanner />}
      {view.kind === 'routine' && <RoutineView />}
      {view.kind === 'tags' && <TagsView />}
      {view.kind === 'io' && <IoMappingView device={view.device} />}
      {view.kind === 'tasks' && <TasksView />}
    </main>
  )
}
