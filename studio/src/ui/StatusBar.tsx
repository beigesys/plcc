// SPDX-License-Identifier: MPL-2.0
import { useEffect, useRef, useState } from 'react'
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
  // Worker clocks tick in 0.1 ms steps without cross-origin isolation; an
  // average over many scans still resolves below that.
  if (ms < 1) return `${(ms * 1000).toFixed(0)} µs`
  return `${ms.toFixed(2)} ms`
}

interface Shown {
  scanAvg: number
  scanMax: number
  jitterAvg: number
  jitterMax: number
}

/**
 * Stats arrive ~30 times a second and a single scan time flips between the
 * worker clock's 0.1 ms steps, so the raw numbers are unreadable. Collect
 * every sample and show the average and worst case once a second.
 */
function useSmoothedStats(stats: { lastScanMs: number; jitterMs: number } | null | undefined, active: boolean) {
  const samples = useRef<{ scan: number[]; jitter: number[] }>({ scan: [], jitter: [] })
  const [shown, setShown] = useState<Shown | null>(null)

  useEffect(() => {
    if (!active || !stats) return
    samples.current.scan.push(stats.lastScanMs)
    samples.current.jitter.push(stats.jitterMs)
  }, [stats, active])

  useEffect(() => {
    if (!active) return
    const t = setInterval(() => {
      const { scan, jitter } = samples.current
      if (scan.length === 0) return
      const avg = (xs: number[]) => xs.reduce((a, b) => a + b, 0) / xs.length
      setShown({
        scanAvg: avg(scan),
        scanMax: Math.max(...scan),
        jitterAvg: avg(jitter),
        jitterMax: Math.max(...jitter),
      })
      samples.current = { scan: [], jitter: [] }
    }, 1000)
    return () => clearInterval(t)
  }, [active])

  return active ? shown : null
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
  const shown = useSmoothedStats(stats, mode === 'simulate')

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
    <footer className="flex items-center gap-5 overflow-hidden border-t border-line bg-nav px-3 text-dense whitespace-nowrap" aria-label="Status">
      {comms}
      {mode === 'simulate' && (
        <>
          <span title="Average and worst case over the last second" className="flex items-center gap-5">
            <Field label="Scan">
              <span className="inline-block w-[15ch] tabular-nums">
                {shown ? `${fmtMs(shown.scanAvg)} · max ${fmtMs(shown.scanMax)}` : '—'}
              </span>
            </Field>
            <Field label="Period">{stats ? `${stats.periodMs} ms` : '—'}</Field>
            <Field label="Jitter">
              <span className="inline-block w-[15ch] tabular-nums">
                {shown ? `${fmtMs(shown.jitterAvg)} · max ${fmtMs(shown.jitterMax)}` : '—'}
              </span>
            </Field>
          </span>
          {(overrun || (stats?.overruns ?? 0) > 0) && (
            <span className="rounded-control bg-alarm-bg px-1.5 text-alarm">Scan overrun ×{stats?.overruns ?? 0}</span>
          )}
        </>
      )}
      <span className="ml-auto flex min-w-0 items-center gap-5">
        {notice && (
          <span
            role="status"
            title={notice.text}
            className={`min-w-0 truncate ${notice.tone === 'fault' ? 'text-fault' : notice.tone === 'alarm' ? 'text-alarm' : 'text-text-muted'}`}
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
