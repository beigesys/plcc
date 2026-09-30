// SPDX-License-Identifier: MPL-2.0
import { useEffect, useState } from 'react'
import { cursorText, useEditor } from '@/state/editor'
import { useLive } from '@/state/live'
import { StateDot } from './StateDot'

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <span className="flex items-center gap-1.5 whitespace-nowrap">
      <span className="text-text-muted">{label}</span>
      <span className="text-mono">{children}</span>
    </span>
  )
}

function fmtMs(ms: number | undefined): string {
  if (ms === undefined) return '—'
  if (ms < 1) return `${(ms * 1000).toFixed(0)} µs`
  return `${ms.toFixed(2)} ms`
}

function SaveText() {
  const status = useEditor((s) => s.saveStatus)
  const err = useEditor((s) => s.saveError)
  const text =
    status === 'saving' ? 'Saving…' : status === 'pending' ? 'Unsaved' : status === 'saved' ? 'Saved' : status === 'memory' ? 'In memory only' : status === 'error' ? 'Save failed' : ''
  return (
    <span className={status === 'error' ? 'text-fault' : status === 'memory' ? 'text-alarm' : 'text-text-muted'} title={err}>
      {text}
    </span>
  )
}

export function StatusBar() {
  const mode = useEditor((s) => s.mode)
  const cursor = useEditor(cursorText)
  const notice = useEditor((s) => s.notice)
  const stats = useLive((s) => s.stats)
  const overrun = useLive((s) => s.overrun)
  const online = useLive((s) => s.online)
  const [now, setNow] = useState(() => Date.now())

  useEffect(() => {
    if (mode !== 'online') return
    const t = setInterval(() => setNow(Date.now()), 500)
    return () => clearInterval(t)
  }, [mode])

  useEffect(() => {
    if (!notice) return
    const t = setTimeout(() => useEditor.setState({ notice: null }), 6000)
    return () => clearTimeout(t)
  }, [notice])

  let comms: React.ReactNode = <span className="text-text-muted">Offline — editing only</span>
  if (mode === 'simulate') {
    comms = (
      <span className="flex items-center gap-1.5">
        <StateDot state={stats?.running ? 'power' : 'idle'} />
        <span>Preview simulator</span>
        <span className="text-text-muted">(browser, not plcc output)</span>
      </span>
    )
  } else if (mode === 'online') {
    const age = online.lastUpdate ? now - online.lastUpdate : undefined
    const dot = online.state === 'online' ? (online.fault ? 'fault' : 'power') : online.state === 'error' ? 'alarm' : 'idle'
    comms = (
      <span className="flex items-center gap-1.5">
        <StateDot state={dot} />
        <span>
          {online.transport === 'fake' ? 'Demo device' : 'USB serial 115200'} · {online.state}
          {online.error && online.state !== 'online' ? ` — ${online.error}` : ''}
        </span>
        {online.fault && <span className="text-fault">{online.fault}</span>}
        {age !== undefined && online.state === 'online' && <span className="text-text-muted">updated {Math.max(0, age)} ms ago</span>}
      </span>
    )
  }

  return (
    <footer className="flex items-center gap-5 border-t border-line bg-nav px-3 text-dense" aria-label="Status">
      {comms}
      {mode === 'simulate' && (
        <>
          <Field label="Scan">{fmtMs(stats?.lastScanMs)}</Field>
          <Field label="Period">{stats ? `${stats.periodMs} ms` : '—'}</Field>
          <Field label="Jitter">{fmtMs(stats?.jitterMs)}</Field>
          {(overrun || (stats?.overruns ?? 0) > 0) && (
            <span className="rounded-control bg-alarm-bg px-1.5 text-alarm">Scan overrun ×{stats?.overruns ?? 0}</span>
          )}
        </>
      )}
      <span className="ml-auto flex items-center gap-5">
        {notice && (
          <span
            role="status"
            className={notice.tone === 'fault' ? 'text-fault' : notice.tone === 'alarm' ? 'text-alarm' : 'text-text-muted'}
          >
            {notice.text}
          </span>
        )}
        <span className="text-text-muted">{cursor}</span>
        <SaveText />
      </span>
    </footer>
  )
}
