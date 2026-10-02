// SPDX-License-Identifier: MPL-2.0
// A Structured Text code view/editor (CodeMirror 6) themed with the studio's
// CSS tokens, so it follows Graphite / Control Room / Blueprint.

import { useEffect, useRef } from 'react'
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands'
import { HighlightStyle, syntaxHighlighting } from '@codemirror/language'
import { EditorState, type Extension } from '@codemirror/state'
import { EditorView, highlightActiveLine, keymap, lineNumbers } from '@codemirror/view'
import { tags as t } from '@lezer/highlight'
import { structuredText } from '@/lang/st'

const highlight = HighlightStyle.define([
  { tag: t.keyword, color: 'var(--syn-keyword)', fontWeight: '500' },
  { tag: t.typeName, color: 'var(--syn-type)' },
  { tag: [t.number, t.atom], color: 'var(--syn-number)' },
  { tag: t.string, color: 'var(--syn-string)' },
  { tag: t.comment, color: 'var(--syn-comment)', fontStyle: 'italic' },
  { tag: t.meta, color: 'var(--syn-comment)' },
  { tag: t.function(t.variableName), color: 'var(--syn-function)' },
  { tag: t.operator, color: 'var(--text-muted)' },
])

const theme = EditorView.theme({
  '&': { height: '100%', fontSize: '12.5px', backgroundColor: 'transparent', color: 'var(--text)' },
  '.cm-scroller': { fontFamily: 'var(--app-font-mono)', lineHeight: '18px' },
  '.cm-content': { padding: '8px 0', caretColor: 'var(--text)' },
  '.cm-gutters': { backgroundColor: 'transparent', border: 'none', color: 'var(--text-muted)' },
  '.cm-activeLine': { backgroundColor: 'var(--surface-2)' },
  '.cm-activeLineGutter': { backgroundColor: 'transparent', color: 'var(--text)' },
  '&.cm-focused': { outline: 'none' },
  '&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection': {
    backgroundColor: 'var(--syn-selection) !important',
  },
})

export interface StCodeProps {
  value: string
  readOnly?: boolean
  onChange?: (text: string) => void
  onBlur?: (text: string) => void
  /** Scroll so this 1-based line is near the top. */
  scrollToLine?: number
  ariaLabel: string
  testId?: string
}

export function StCode({ value, readOnly = false, onChange, onBlur, scrollToLine, ariaLabel, testId }: StCodeProps) {
  const host = useRef<HTMLDivElement>(null)
  const view = useRef<EditorView | null>(null)
  const handlers = useRef({ onChange, onBlur })
  useEffect(() => {
    handlers.current = { onChange, onBlur }
  }, [onChange, onBlur])

  useEffect(() => {
    if (!host.current) return
    const extensions: Extension[] = [
      lineNumbers(),
      structuredText,
      syntaxHighlighting(highlight),
      theme,
      EditorView.contentAttributes.of({ 'aria-label': ariaLabel }),
      EditorState.readOnly.of(readOnly),
      EditorView.editable.of(!readOnly),
      EditorView.updateListener.of((u) => {
        if (u.docChanged) handlers.current.onChange?.(u.state.doc.toString())
      }),
      EditorView.domEventHandlers({
        blur: (_e, v) => {
          handlers.current.onBlur?.(v.state.doc.toString())
        },
      }),
    ]
    if (!readOnly) extensions.push(history(), highlightActiveLine(), keymap.of([...defaultKeymap, ...historyKeymap, indentWithTab]))
    const v = new EditorView({ parent: host.current, state: EditorState.create({ doc: value, extensions }) })
    view.current = v
    return () => {
      v.destroy()
      view.current = null
    }
    // The editor is created once per mode; value changes are applied below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [readOnly, ariaLabel])

  useEffect(() => {
    const v = view.current
    if (!v) return
    const current = v.state.doc.toString()
    if (current !== value) v.dispatch({ changes: { from: 0, to: current.length, insert: value } })
  }, [value])

  useEffect(() => {
    const v = view.current
    if (!v || !scrollToLine || scrollToLine > v.state.doc.lines) return
    const pos = v.state.doc.line(scrollToLine).from
    v.dispatch({ effects: EditorView.scrollIntoView(pos, { y: 'start', yMargin: 40 }) })
  }, [scrollToLine, value])

  return <div ref={host} data-testid={testId} className="min-h-0 flex-1 overflow-hidden" />
}
