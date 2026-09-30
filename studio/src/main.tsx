// SPDX-License-Identifier: MPL-2.0
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { TooltipProvider } from '@/components/ui/tooltip'
import { applyTheme, hasChosenTheme, initialTheme } from '@/state/theme'
import { App } from './App'
import './index.css'

applyTheme(initialTheme(), false)

// Until the user picks a theme, follow the OS light/dark setting.
if (typeof matchMedia === 'function') {
  matchMedia('(prefers-color-scheme: light)').addEventListener('change', (e) => {
    if (!hasChosenTheme()) applyTheme(e.matches ? 'blueprint' : 'graphite', false)
  })
}

const root = document.getElementById('root')
if (root) {
  createRoot(root).render(
    <StrictMode>
      <TooltipProvider delayDuration={300}>
        <App />
      </TooltipProvider>
    </StrictMode>,
  )
}
