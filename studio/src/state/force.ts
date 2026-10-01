// SPDX-License-Identifier: MPL-2.0
// Writing and forcing tags from the editor: Simulate (the simulator worker)
// and Online (the device console, %M bits only).

import { findTag } from '@/model'
import { useEditor } from './editor'
import { simSend, useLive } from './live'
import { onlineCanWrite, onlineForceBit } from './online'

/** Whether a tag can be toggled / forced in the current mode. */
export function canForce(tag: string): boolean {
  const s = useEditor.getState()
  const t = findTag(s.project, tag)
  if (!t || t.data_type.toUpperCase() !== 'BOOL') return false
  if (s.mode === 'simulate') return true
  if (s.mode === 'online') return !!t.address && onlineCanWrite(t.address)
  return false
}

/** Toggles a BOOL tag in Simulate (write) or Online (%M force). */
export function toggleTag(tag: string) {
  const s = useEditor.getState()
  const t = findTag(s.project, tag)
  const cur = useLive.getState().values[tag.toLowerCase()]
  if (s.mode === 'simulate') {
    simSend({ type: 'write', ref: tag, value: !cur })
  } else if (s.mode === 'online') {
    if (!t?.address || !onlineCanWrite(t.address)) {
      s.notify(`${tag} is not a %M bit the device console can write`, 'alarm')
      return
    }
    onlineForceBit(t.address, !cur).catch((e: unknown) => s.notify(String(e), 'fault'))
  }
}

/** Forces a BOOL tag on / off, or removes the force (null; Simulate only). */
export function forceTag(tag: string, on: boolean | null) {
  const s = useEditor.getState()
  const t = findTag(s.project, tag)
  if (s.mode === 'simulate') simSend({ type: 'force', tag, value: on })
  else if (s.mode === 'online' && t?.address && on !== null) {
    onlineForceBit(t.address, on).catch((e: unknown) => s.notify(e instanceof Error ? e.message : String(e), 'fault'))
  }
}
