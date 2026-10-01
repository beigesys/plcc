// SPDX-License-Identifier: MPL-2.0
import { useCallback, useMemo } from 'react'
import { traceRoutine, type Trace } from '@/engine'
import type { Routine, Tag } from '@/model'
import type { Scalar } from '@/runtime/messages'
import { useEditor } from './editor'
import { readRef, useLive } from './live'

/** Reads a live value (`Tag`, `Tag.DN`, `Word.3`), or undefined outside Simulate/Online. */
export function useLiveRead(): ((ref: string) => Scalar | undefined) | undefined {
  const values = useLive((s) => s.values)
  const source = useLive((s) => s.source)
  const read = useCallback((ref: string) => readRef(values, ref), [values])
  return source === 'none' ? undefined : read
}

/** Power flow for a routine: from the simulator, or computed from live device values. */
export function useTrace(routine: Routine | undefined): Trace | null {
  const mode = useEditor((s) => s.mode)
  const simTrace = useLive((s) => s.trace)
  const values = useLive((s) => s.values)
  const onlineState = useLive((s) => s.online.state)
  const onlineTrace = useMemo(() => {
    if (mode !== 'online' || !routine || onlineState === 'disconnected' || onlineState === 'connecting') return null
    return traceRoutine(routine, (ref) => readRef(values, ref))
  }, [mode, routine, values, onlineState])
  if (mode === 'simulate') return simTrace
  if (mode === 'online') return onlineTrace
  return null
}

export function useTagMap(): Map<string, Tag> {
  const tags = useEditor((s) => s.project.globals)
  return useMemo(() => new Map(tags.map((t) => [t.name.toLowerCase(), t])), [tags])
}
