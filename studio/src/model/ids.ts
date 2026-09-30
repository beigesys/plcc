// SPDX-License-Identifier: MPL-2.0

let counter = 0

/** A short id, unique within a session and very likely across sessions. */
export function newId(prefix = 'e'): string {
  counter = (counter + 1) % 0x10000
  const rand = Math.floor(Math.random() * 0x100000000).toString(36)
  return `${prefix}${Date.now().toString(36).slice(-5)}${counter.toString(36)}${rand}`
}
