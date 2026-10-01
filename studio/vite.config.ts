// SPDX-License-Identifier: MPL-2.0
import path from 'node:path'
import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

export default defineConfig({
  // GitHub Pages serves the app under /<repo>/; local and tailnet builds use /.
  base: process.env.STUDIO_BASE ?? '/',
  plugins: [react(), tailwindcss()],
  resolve: { alias: { '@': path.resolve(import.meta.dirname, './src') } },
  // One app bundle (React, Radix, the editor) is ~560 kB before gzip; fine for a local tool.
  build: { chunkSizeWarningLimit: 800 },
  worker: { format: 'es' },
  // The device catalog (../devices) and the built-in manifests
  // (../crates/plcc-device/builtin) are bundled from outside the studio root.
  server: { fs: { allow: ['..'] } },
})
