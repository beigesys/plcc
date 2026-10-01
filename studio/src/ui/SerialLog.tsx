// SPDX-License-Identifier: MPL-2.0
// The serial session's debug log: every line sent and received over the
// device console, with connection events and timeouts.

import { useEffect, useRef, useState, useSyncExternalStore } from 'react'
import { Button } from '@/components/ui/button'
import { serialLog, subscribeSerialLog } from '@/state/online'
import { saveText } from '@/state/convert'

let version = 0
function useLog() {
  return useSyncExternalStore(
    (cb) =>
      subscribeSerialLog(() => {
        version++
        cb()
      }),
    () => version,
  )
}

function time(at: number) {
  const d = new Date(at)
  return `${d.toTimeString().slice(0, 8)}.${String(d.getMilliseconds()).padStart(3, '0')}`
}

export function SerialLog() {
  useLog()
  const [polls, setPolls] = useState(false)
  const [frozen, setFrozen] = useState<typeof serialLog | null>(null)
  const paused = frozen !== null
  const box = useRef<HTMLDivElement>(null)
  const isPoll = (t: string) => t === 'img' || /^I: /.test(t)
  const shown = (frozen ?? serialLog.slice(-500)).filter((e) => polls || !isPoll(e.text))
  useEffect(() => {
    const el = box.current
    if (el && !paused) el.scrollTop = el.scrollHeight
  })
  return (
    <section aria-label="Serial log" className="w-full border-t border-line pt-2">
      <div className="mb-1 flex items-center gap-3 text-dense">
        <h2 className="text-[11px] font-semibold tracking-wider text-text-muted uppercase">Serial log</h2>
        <label className="flex items-center gap-1 text-text-muted">
          <input type="checkbox" checked={polls} onChange={(e) => setPolls(e.target.checked)} className="accent-[var(--text)]" /> show img polls
        </label>
        <label className="flex items-center gap-1 text-text-muted">
          <input type="checkbox" checked={paused} onChange={(e) => setFrozen(e.target.checked ? serialLog.slice(-500) : null)} className="accent-[var(--text)]" /> pause
        </label>
        <Button
          size="xs"
          variant="ghost"
          className="ml-auto"
          onClick={() => saveText('serial-log.txt', serialLog.map((e) => `${time(e.at)} ${e.dir === 'tx' ? '>' : e.dir === 'rx' ? '<' : '#'} ${e.text}`).join('\n'))}
        >
          Save
        </Button>
      </div>
      <div ref={box} data-testid="serial-log" className="h-40 overflow-y-auto rounded-control border border-line bg-bg p-2 text-mono text-[11px] leading-[16px]">
        {shown.length === 0 && <span className="text-text-muted">Nothing yet.</span>}
        {shown.map((e, i) => (
          <div key={i} className={e.dir === 'event' ? 'text-alarm' : e.dir === 'tx' ? 'text-text' : 'text-text-muted'}>
            <span className="text-text-muted">{time(e.at)} </span>
            {e.dir === 'tx' ? '> ' : e.dir === 'rx' ? '< ' : '# '}
            {e.text}
          </div>
        ))}
      </div>
    </section>
  )
}
