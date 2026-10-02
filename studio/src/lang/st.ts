// SPDX-License-Identifier: MPL-2.0
// Structured Text (IEC 61131-3, CODESYS and Logix flavours) for CodeMirror:
// a stream tokenizer, enough for highlighting. plcc does the real parsing.

import { StreamLanguage, type StreamParser } from '@codemirror/language'

const KEYWORDS = new Set(
  `IF THEN ELSIF ELSE END_IF CASE OF END_CASE FOR TO BY DO END_FOR WHILE END_WHILE
   REPEAT UNTIL END_REPEAT EXIT CONTINUE RETURN AND OR XOR NOT MOD AND_THEN OR_ELSE
   PROGRAM END_PROGRAM FUNCTION END_FUNCTION FUNCTION_BLOCK END_FUNCTION_BLOCK
   METHOD END_METHOD PROPERTY END_PROPERTY INTERFACE END_INTERFACE CLASS END_CLASS
   ACTION END_ACTION EXTENDS IMPLEMENTS THIS SUPER ABSTRACT FINAL PUBLIC PRIVATE
   PROTECTED INTERNAL VAR VAR_INPUT VAR_OUTPUT VAR_IN_OUT VAR_GLOBAL VAR_EXTERNAL
   VAR_TEMP VAR_STAT VAR_INST VAR_ACCESS VAR_CONFIG END_VAR CONSTANT RETAIN
   NON_RETAIN PERSISTENT AT TYPE END_TYPE STRUCT END_STRUCT UNION END_UNION
   CONFIGURATION END_CONFIGURATION RESOURCE END_RESOURCE TASK WITH ON
   READ_ONLY READ_WRITE R_EDGE F_EDGE REF_TO REFERENCE POINTER ARRAY STRING WSTRING
   TRUE FALSE NULL`
    .split(/\s+/)
    .filter(Boolean),
)

const TYPES = new Set(
  `BOOL BYTE WORD DWORD LWORD SINT INT DINT LINT USINT UINT UDINT ULINT REAL LREAL
   TIME LTIME DATE TIME_OF_DAY TOD DATE_AND_TIME DT LDATE LTOD LDT CHAR WCHAR ANY
   TON TOF TP RTO CTU CTD CTUD R_TRIG F_TRIG SR RS TIMER COUNTER`
    .split(/\s+/)
    .filter(Boolean),
)

interface State {
  // Nesting depth of (* *) comments (they nest in IEC), or -1 inside a C-style block comment.
  comment: number
}

const parser: StreamParser<State> = {
  name: 'structured-text',
  startState: () => ({ comment: 0 }),
  token(stream, state) {
    if (state.comment === -1) {
      if (stream.skipTo('*/')) {
        stream.next()
        stream.next()
        state.comment = 0
      } else stream.skipToEnd()
      return 'comment'
    }
    if (state.comment > 0) {
      while (!stream.eol()) {
        if (stream.match('(*')) state.comment++
        else if (stream.match('*)')) {
          state.comment--
          if (state.comment === 0) break
        } else stream.next()
      }
      return 'comment'
    }
    if (stream.eatSpace()) return null
    if (stream.match('(*')) {
      state.comment = 1
      return parser.token(stream, state)
    }
    if (stream.match('/*')) {
      state.comment = -1
      return parser.token(stream, state)
    }
    if (stream.match('//')) {
      stream.skipToEnd()
      return 'comment'
    }
    if (stream.match('{')) {
      if (stream.skipTo('}')) stream.next()
      else stream.skipToEnd()
      return 'meta' // pragma
    }
    const quote = stream.peek()
    if (quote === "'" || quote === '"') {
      stream.next()
      while (!stream.eol()) {
        const ch = stream.next()
        if (ch === '$') stream.next()
        else if (ch === quote) break
      }
      return 'string'
    }
    // Direct addresses: %IX0.0, %QW4, %MD2
    if (stream.match(/^%[IQM][XBWDL]?[\d.]+/i)) return 'atom'
    // Typed and based literals: T#5s, TIME#1h2m, 16#FF, INT#5, D#2024-01-01
    if (stream.match(/^[A-Za-z_]+#[^\s;,)]+/) || stream.match(/^\d+#[0-9A-Fa-f_]+/)) return 'number'
    if (stream.match(/^\d[\d_]*(\.\d[\d_]*)?([eE][+-]?\d+)?/)) return 'number'
    const word = stream.match(/^[A-Za-z_][A-Za-z0-9_]*/) as RegExpMatchArray | null
    if (word) {
      const up = word[0].toUpperCase()
      if (KEYWORDS.has(up)) return 'keyword'
      if (TYPES.has(up)) return 'typeName'
      if (stream.match(/^\s*\(/, false)) return 'function'
      return 'variableName'
    }
    if (stream.match(/^(:=|=>|<>|<=|>=|\*\*|[+\-*/=<>&^.,;:[\]()])/)) return 'operator'
    stream.next()
    return null
  },
  languageData: { commentTokens: { line: '//', block: { open: '(*', close: '*)' } } },
}

export const structuredText = StreamLanguage.define(parser)
