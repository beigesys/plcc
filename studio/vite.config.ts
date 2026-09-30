// SPDX-License-Identifier: MPL-2.0
import path from 'node:path'
import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: { alias: { '@': path.resolve(import.meta.dirname, './src') } },
  // One app bundle (React, Radix, the editor) is ~560 kB before gzip; fine for a local tool.
  build: { chunkSizeWarningLimit: 800 },
  worker: { format: 'es' },
})
