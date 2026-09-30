// SPDX-License-Identifier: MPL-2.0
import { describe, expect, it } from 'vitest'
import { buildCatalog, CATALOG, catalogEntry } from './catalog'
import { evalExpr, expandTemplate, formatDiagnostic, loadManifest, locatePath } from './manifest'

// The Rust crate's golden expansions and the manifests they come from.
const raw = (g: Record<string, unknown>) =>
  Object.fromEntries(Object.entries(g).map(([k, v]) => [k.split('/').pop()!, v as string]))
const goldens = raw(import.meta.glob('../../../crates/plcc-device/tests/data/*.expanded.json', { query: '?raw', import: 'default', eager: true }))
const sources = {
  ...raw(import.meta.glob('../../../crates/plcc-device/tests/data/*.toml', { query: '?raw', import: 'default', eager: true })),
  ...raw(import.meta.glob('../../../crates/plcc-device/builtin/*.toml', { query: '?raw', import: 'default', eager: true })),
}
const builtinOpta = sources['arduino-opta.toml']
const builtinSim = sources['simulator.toml']

describe('manifest loader matches plcc-device', () => {
  const names = Object.keys(goldens)
  it('has golden files', () => expect(names.length).toBeGreaterThanOrEqual(3))
  for (const g of names) {
    const stem = g.replace('.expanded.json', '')
    it(`expands ${stem} like the Rust validator`, () => {
      const src = sources[`${stem}.toml`]
      expect(src).toBeDefined()
      const r = loadManifest(src)
      expect(r.diagnostics).toEqual([])
      expect(r.device).toEqual(JSON.parse(goldens[g]))
    })
  }
})

describe('expressions', () => {
  it('evaluates integer arithmetic on n', () => {
    expect(evalExpr('n-1', 1)).toBe(0)
    expect(evalExpr('(n-1)/8', 10)).toBe(1)
    expect(evalExpr('(n-1)%8', 10)).toBe(1)
    expect(evalExpr('-n', 3)).toBe(-3)
    expect(evalExpr('-7/2', 0)).toBe(-3)
    expect(expandTemplate('%IX{(n-1)/8}.{(n-1)%8}', 9)).toBe('%IX1.0')
    expect(expandTemplate('a {{b}}', 1)).toBe('a {b}')
  })
  it('reports bad expressions', () => {
    expect(() => evalExpr('m', 1)).toThrow(/only variable/)
    expect(() => evalExpr('n/0', 1)).toThrow(/division by zero/)
    expect(() => evalExpr('(n', 1)).toThrow(/missing `\)`/)
    expect(() => expandTemplate('{n', 1)).toThrow(/closing/)
  })
})

const BASE = `[device]
id = "t"
name = "T"
vendor = "v"
version = 1
schema = 1

[target]
triple = "thumbv7em-none-eabi"
image = { I = 2, Q = 1, M = 4 }

[target.runtime]
kind = "k"
abi = 1

[[io]]
repeat = 4
id = "I{n}"
terminal = "I{n}"
label = "In {n}"
group = "g"
dir = "in"
kind = "digital"
type = "BOOL"
address = "%IX0.{n-1}"
`

function errs(text: string): string[] {
  return loadManifest(text).diagnostics.map((d) => formatDiagnostic(d, 't.toml'))
}

describe('manifest validation', () => {
  it('accepts the base manifest', () => expect(errs(BASE)).toEqual([]))
  it('reports TOML syntax errors with a line', () => {
    const e = loadManifest(BASE.replace('[target]', '[target')).diagnostics
    expect(e[0].line).toBe(8)
  })
  it('points semantic errors at their line', () => {
    const e = errs(BASE.replace('%IX0.{n-1}', '%QX0.{n-1}'))
    expect(e[0]).toMatch(/^t\.toml:25: error: io\[0\]\.address: .* is in %Q, but dir = "in" points live in %I/)
    expect(errs(BASE.replace('type = "BOOL"', 'type = "INT"'))[0]).toMatch(/^t\.toml:24: .*INT does not fit/)
    expect(errs(BASE.replace('M = 4', 'M = 4, X = 1'))[0]).toMatch(/unknown field `X`/)
  })
  it('checks addresses against the image and ids for uniqueness', () => {
    expect(errs(BASE.replace('%IX0.{n-1}', '%IX{n}.0')).join('\n')).toMatch(/ends at byte 3, past the 2-byte %I area/)
    expect(errs(BASE.replace('id = "I{n}"', 'id = "I"')).join('\n')).toMatch(/must contain `\{n\}`/)
    expect(errs(BASE.replace('repeat = 4\n', '')).join('\n')).toMatch(/has no `repeat`/)
    expect(errs(BASE.replace('schema = 1', 'schema = 2')).join('\n')).toMatch(/format 2 is not supported/)
  })
  it('rejects a flash area over a protected region', () => {
    const flash = `\n[flash]\nmethod = "dfuse"\nusb = [{ vid = 0x2341, pid = 0x0364 }]\naddress = 0x08000000\nmax_size = 0x1C0000\nprotected = [{ start = 0x08000000, size = 0x40000 }]\n`
    expect(errs(BASE + flash).join('\n')).toMatch(/overlaps protected region/)
    expect(errs(BASE + flash.replace('address = 0x08000000', 'address = 0x08040000'))).toEqual([])
  })
  it('locates paths in the text', () => {
    expect(locatePath(BASE, 'target.image.M')).toBe(10)
    expect(locatePath(BASE, 'target.runtime.abi')).toBe(14)
    expect(locatePath(BASE, 'io[0].dir')).toBe(22)
  })
})

describe('catalog', () => {
  it('bundles the Opta and the Simulator', () => {
    expect(catalogEntry('arduino-opta')?.device.target.image).toEqual({ I: 18, Q: 1, M: 64 })
    expect(catalogEntry('simulator')?.device.target.triple).toBe('wasm32-unknown-unknown')
    expect(CATALOG.every((e) => e.device.device.id === e.id)).toBe(true)
  })
  it('falls back to the built-in copies without the devices/ submodule', () => {
    const opta = builtinOpta
    const sim = builtinSim
    const cat = buildCatalog({}, { 'b/arduino-opta.toml': opta, 'b/simulator.toml': sim })
    expect(cat.map((e) => [e.id, e.origin])).toEqual([
      ['arduino-opta', 'builtin'],
      ['simulator', 'builtin'],
    ])
    const newer = opta.replace('version = 1', 'version = 2')
    const both = buildCatalog({ 'd/arduino-opta.toml': newer }, { 'b/arduino-opta.toml': opta, 'b/simulator.toml': sim })
    expect(both.find((e) => e.id === 'arduino-opta')).toMatchObject({ origin: 'catalog', device: { device: { version: 2 } } })
  })
})
