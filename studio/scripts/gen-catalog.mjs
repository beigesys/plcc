// SPDX-License-Identifier: MPL-2.0
// Writes src/model/logix-catalog.json: plcc's Logix instruction catalog
// (plcc_ladder::catalog::all, through packages/plcc-wasm), which the rung
// text reader and the palette use. Run after `npm run build` in
// packages/plcc-wasm; src/model/catalog.test.ts checks the file is current.
import { readFileSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const pkg = join(here, '../../packages/plcc-wasm/pkg')
const glue = await import(join(pkg, 'plcc_wasm.js'))
glue.initSync({ module: readFileSync(join(pkg, 'plcc_wasm_bg.wasm')) })
const specs = JSON.parse(glue.catalog('logix'))
const out = join(here, '../src/model/logix-catalog.json')
writeFileSync(out, `${JSON.stringify(specs, null, 1)}\n`)
console.log(`${specs.length} instructions → ${out}`)
