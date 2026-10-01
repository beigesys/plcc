// SPDX-License-Identifier: MPL-2.0
/// <reference types="node" />
//
// Import and export through plcc's converter (packages/plcc-wasm, run in this
// thread; skipped when its wasm has not been built): every format File >
// Import takes, merged into a project, and every export.

import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs'
import { dirname, join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import * as wasm from '@plcc/plcc-wasm'
import { demoProject, isStRoutine, toLadderJson } from '@/model'
import { useFrontendDirect } from '@/plcc/frontend'
import { convertProject, importKind, mergeImport, readImport } from './convert'

const here = dirname(fileURLToPath(import.meta.url))
const pkg = join(here, '../../../packages/plcc-wasm/pkg/plcc_wasm_bg.wasm')
const fixtures = join(here, '../../../tests/fixtures')
const built = existsSync(pkg)
const read = (rel: string) => readFileSync(join(fixtures, rel), 'utf8')

function folder(dir: string): Record<string, string> {
  const out: Record<string, string> = {}
  const walk = (d: string) => {
    for (const n of readdirSync(d)) {
      const p = join(d, n)
      if (statSync(p).isDirectory()) walk(p)
      else out[relative(dir, p)] = readFileSync(p, 'utf8')
    }
  }
  walk(dir)
  return out
}

describe.skipIf(!built)('import and export through plcc convert', () => {
  beforeAll(async () => {
    await wasm.load(readFileSync(pkg))
    useFrontendDirect({
      check: wasm.check,
      convert: wasm.convert,
      loadDevice: wasm.loadDevice,
      validateDevice: wasm.validateDevice,
      locateLadder: wasm.locateLadder,
      catalog: wasm.catalog,
      version: wasm.version,
    })
  })
  afterAll(() => useFrontendDirect(null))

  /** Merges `files` into the demo and checks the result with plcc. */
  async function importInto(files: Record<string, string>, kind: Parameters<typeof readImport>[1]) {
    const imp = await readImport(files, kind)
    const { project, report } = mergeImport(demoProject(), imp.model)
    const check = await wasm.check({ files: { 'project.json': toLadderJson(project) }, entry: ['project.json'] })
    return { imp, project, report, check }
  }

  it('recognizes the formats', () => {
    expect(importKind('a.L5X')).toBe('l5x')
    expect(importKind('a.xml', '<RSLogix5000Content')).toBe('l5x')
    expect(importKind('a.xml', '<project xmlns="http://www.plcopen.org/xml/tc6_0201">')).toBe('plcopen')
    expect(importKind('a.st')).toBe('st')
    expect(importKind('Demo.plcproj')).toBe('twincat')
    expect(importKind('a.zip')).toBe('twincat')
    expect(importKind('a.txt')).toBeUndefined()
  })

  it('imports an L5X export as it is: it checks clean next to the demo', async () => {
    const { report, check, project } = await importInto({ 'timers_counters.L5X': read('l5x/timers_counters.L5X') }, 'l5x')
    expect(report.programs).toEqual(['MainProgram_2'])
    expect(report.notes.join(' ')).toMatch(/imported as MainProgram_2/)
    expect(project.tasks.flatMap((t) => t.programs)).toContain('MainProgram_2')
    expect(check.diagnostics.filter((d) => d.severity === 'error')).toEqual([])
  })

  it('imports an L5X ST routine as an ST routine', async () => {
    const { imp } = await importInto({ 'st_routine.L5X': read('l5x/st_routine.L5X') }, 'l5x')
    expect(imp.model.pous.some((p) => p.routines.some(isStRoutine))).toBe(true)
  })

  it('imports PLCopen LD, translated to Logix with notes', async () => {
    const { imp, check } = await importInto({ 'ld_seal_in.xml': read('plcopen/ld_seal_in.xml') }, 'plcopen')
    expect(imp.model.dialect).toBe('logix')
    expect(imp.model.pous.length).toBeGreaterThan(0)
    expect(imp.diagnostics.some((d) => d.stage === 'convert')).toBe(true)
    expect(check.diagnostics.filter((d) => d.severity === 'error')).toEqual([])
  })

  it('imports Structured Text, drawable statements as rungs', async () => {
    const { imp } = await importInto({ 'blink.st': read('programs/blink.st') }, 'st')
    expect(imp.model.pous.flatMap((p) => p.routines.flatMap((r) => r.rungs)).length).toBeGreaterThan(0)
  })

  it('imports a TwinCAT project through its ST', async () => {
    const files = folder(join(fixtures, 'twincat/Demo'))
    expect(Object.keys(files).some((f) => f.endsWith('.plcproj'))).toBe(true)
    const { imp } = await importInto(files, 'twincat')
    expect(imp.model.pous.length).toBeGreaterThan(0)
  })

  it('exports L5X, PLCopen XML and ST that plcc reads back', async () => {
    const p = demoProject()
    const l5x = await convertProject(p, 'l5x')
    expect(l5x.ok).toBe(true)
    expect(l5x.text).toContain('<RSLogix5000Content')
    expect(l5x.text).toContain('Type="PERIODIC" Rate="10"')
    expect((await wasm.check({ files: { 'p.L5X': l5x.text! } })).ok).toBe(true)
    const xml = await convertProject(p, 'plcopen')
    expect(xml.ok).toBe(true)
    expect(xml.diagnostics.some((d) => d.severity === 'warning')).toBe(true) // Logix → IEC notes
    expect((await wasm.check({ files: { 'p.xml': xml.text! } })).diagnostics.filter((d) => d.severity === 'error')).toEqual([])
    const st = await convertProject(p, 'st', { dialect: 'iec' })
    expect(st.ok).toBe(true)
    expect(st.text).toMatch(/RunTimer\(IN :=/)
    const routine = await convertProject(p, 'st', { scope: { program: 'MainProgram', routine: 'MainRoutine' } })
    expect(routine.text).toMatch(/METHOD R_MainRoutine/)
  })
})
