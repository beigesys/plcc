// SPDX-License-Identifier: MPL-2.0
import { useState } from 'react'

/**
 * Local editable copy of a value that resets whenever the source value
 * changes (undo, another view editing it). Uses React's "adjust state while
 * rendering" pattern instead of an effect.
 */
export function useDraft<T>(value: T): [T, (v: T) => void] {
  const [draft, setDraft] = useState(value)
  const [source, setSource] = useState(value)
  if (!Object.is(source, value)) {
    setSource(value)
    setDraft(value)
  }
  return [draft, setDraft]
}
