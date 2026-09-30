// SPDX-License-Identifier: MPL-2.0

export type ThemeId = 'graphite' | 'control-room' | 'blueprint'

export const THEMES: { id: ThemeId; name: string; hint: string }[] = [
  { id: 'graphite', name: 'Graphite', hint: 'Dark, default' },
  { id: 'control-room', name: 'Control Room', hint: 'ISA-101 HMI, dark' },
  { id: 'blueprint', name: 'Blueprint', hint: 'Light' },
]

const KEY = 'plcc-studio.theme'

function stored(): ThemeId | undefined {
  try {
    const v = localStorage.getItem(KEY)
    return THEMES.some((t) => t.id === v) ? (v as ThemeId) : undefined
  } catch {
    return undefined
  }
}

/** The saved theme, or one that follows prefers-color-scheme until the user picks. */
export function initialTheme(): ThemeId {
  const s = stored()
  if (s) return s
  const light = typeof matchMedia === 'function' && matchMedia('(prefers-color-scheme: light)').matches
  return light ? 'blueprint' : 'graphite'
}

export function hasChosenTheme(): boolean {
  return stored() !== undefined
}

export function applyTheme(id: ThemeId, persist: boolean) {
  const root = document.documentElement
  root.dataset.theme = id
  root.classList.toggle('dark', id !== 'blueprint')
  if (persist) {
    try {
      localStorage.setItem(KEY, id)
    } catch {
      // storage blocked: the choice lasts for this session only
    }
  }
}
