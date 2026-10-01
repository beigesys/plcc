// SPDX-License-Identifier: MPL-2.0
import { existsSync, readFileSync } from 'node:fs'
import path from 'node:path'
import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { defineConfig, type Plugin } from 'vite'

/**
 * plcc's compiler (packages/plcc-compiler-wasm/dist, built by its build.sh or
 * by CI) is served at <base>plcc-compiler/ and copied into the build as is:
 * ~10 MB gzip, fetched by the page only when Simulate or Download needs it.
 * Without it the studio still builds; the simulator then runs its preview
 * engine and says why.
 */
function plccCompiler(): Plugin {
  const dist = path.resolve(import.meta.dirname, '../packages/plcc-compiler-wasm/dist')
  const files = ['plcc-compiler.json', 'plcc-compiler.wasm.gz']
  const present = () => files.every((f) => existsSync(path.join(dist, f)))
  return {
    name: 'plcc-compiler',
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        const m = /\/plcc-compiler\/(plcc-compiler\.(?:json|wasm\.gz))(?:\?.*)?$/.exec(req.url ?? '')
        if (!m || !existsSync(path.join(dist, m[1]))) return next()
        res.setHeader('Content-Type', m[1].endsWith('.json') ? 'application/json' : 'application/octet-stream')
        res.end(readFileSync(path.join(dist, m[1])))
      })
    },
    generateBundle() {
      if (!present()) {
        this.warn(`plcc's compiler is not built (${dist}); Simulate will use the preview engine`)
        return
      }
      for (const f of files) this.emitFile({ type: 'asset', fileName: `plcc-compiler/${f}`, source: readFileSync(path.join(dist, f)) })
    },
  }
}

export default defineConfig({
  // GitHub Pages serves the app under /<repo>/; local and tailnet builds use /.
  base: process.env.STUDIO_BASE ?? '/',
  plugins: [react(), tailwindcss(), plccCompiler()],
  resolve: { alias: { '@': path.resolve(import.meta.dirname, './src') } },
  // One app bundle (React, Radix, the editor) is ~700 kB before gzip; fine for an IDE.
  build: { chunkSizeWarningLimit: 900 },
  worker: { format: 'es' },
  // The device catalog (../devices), the built-in manifests
  // (../crates/plcc-device/builtin) and the @plcc packages (../packages) are
  // bundled from outside the studio root.
  server: { fs: { allow: ['..'] } },
})
