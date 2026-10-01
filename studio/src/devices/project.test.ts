// SPDX-License-Identifier: MPL-2.0
import { describe, expect, it } from 'vitest'
import { demoProject, emptyProject } from '@/model'
import { buildCatalog, catalogEntry } from './catalog'
import {
  addDevice, catalogUpdates, changeDevice, lineDiff, primaryDevice, remapTags, removeDevice, resolveDevice, updateManifestFile,
} from './project'
import { loadManifest } from './manifest'

const opta = catalogEntry('arduino-opta')!
const sim = catalogEntry('simulator')!

/** The Opta manifest with the relays moved one bit up and a new version. */
function optaV2(): string {
  return opta.text.replace('version = 1', 'version = 2').replace('address = "%QX0.{n-1}"', 'address = "%QX0.{n}"').replace('address = "%QX0.4"', 'address = "%QX0.6"')
}

describe('project devices', () => {
  it('resolves the demo project device from its manifest file', () => {
    const p = demoProject()
    const r = resolveDevice(p)
    expect(r.problem).toBeUndefined()
    expect(r.device.device.id).toBe('arduino-opta')
    expect(primaryDevice(p).target.image).toEqual({ I: 18, Q: 1, M: 64 })
  })

  it('falls back to the Simulator for a broken manifest, and says why', () => {
    const p = demoProject()
    p.deviceFiles['devices/arduino-opta.toml'] = '[device'
    const r = resolveDevice(p)
    expect(r.device.device.id).toBe('simulator')
    expect(r.problem).toMatch(/devices\/arduino-opta.toml: invalid TOML/)
  })

  it('adds catalog devices with unique names and shares identical manifests', () => {
    let p = emptyProject('x')
    const a = addDevice(p, opta.text)
    p = a.project
    expect(a.name).toBe('Arduino Opta')
    const b = addDevice(p, opta.text)
    p = b.project
    expect(b.name).toBe('Arduino Opta 2')
    expect(p.devices.map((d) => d.manifest)).toEqual(['devices/simulator.toml', 'devices/arduino-opta.toml', 'devices/arduino-opta.toml'])
    expect(Object.keys(p.deviceFiles).sort()).toEqual(['devices/arduino-opta.toml', 'devices/simulator.toml'])
    // A different manifest under an id the project already has is not added silently.
    expect(() => addDevice(p, optaV2())).toThrow(/already has devices\/arduino-opta.toml \(version 1\)/)
    expect(() => addDevice(p, 'not toml [')).toThrow(/invalid TOML/)
    p = removeDevice(p, 'Arduino Opta')
    expect(p.deviceFiles['devices/arduino-opta.toml']).toBeDefined()
    p = removeDevice(p, 'Arduino Opta 2')
    expect(p.deviceFiles['devices/arduino-opta.toml']).toBeUndefined()
  })

  it('remaps tags by terminal when the device changes, and warns about the rest', () => {
    const p = demoProject()
    const r = changeDevice(p, 'Opta', sim.text)
    expect(r.project.devices).toEqual([{ name: 'Opta', manifest: 'devices/simulator.toml' }])
    expect(r.project.deviceFiles).toEqual({ 'devices/simulator.toml': sim.text })
    // The Opta's terminals (R1, LED, I2 analog) do not exist on the Simulator.
    expect(r.report.moved).toEqual([])
    expect(r.report.warnings.join('\n')).toMatch(/Motor: terminal R1 is not on Simulator; %QX0.0 is DO0/)
    expect(r.report.warnings.join('\n')).toMatch(/High: terminal LED is not on Simulator/)
    // Addresses are kept, never dropped.
    expect(r.project.globals.map((t) => t.address)).toEqual(p.globals.map((t) => t.address))
  })

  it('moves tags to the new address of the same terminal', () => {
    const p = demoProject()
    const r = updateManifestFile(p, 'devices/arduino-opta.toml', optaV2())
    expect(r.report.moved).toEqual([
      { tag: 'Motor', from: '%QX0.0', to: '%QX0.1', terminal: 'R1' },
      { tag: 'High', from: '%QX0.4', to: '%QX0.6', terminal: 'LED' },
    ])
    expect(r.project.globals.find((t) => t.name === 'Motor')?.address).toBe('%QX0.1')
    expect(r.project.deviceFiles['devices/arduino-opta.toml']).toBe(optaV2())
    expect(() => updateManifestFile(p, 'devices/arduino-opta.toml', sim.text)).toThrow(/is simulator, not devices\/arduino-opta.toml/)
  })

  it('offers catalog updates only for newer versions', () => {
    const p = demoProject()
    expect(catalogUpdates(p)).toEqual([])
    const newer = buildCatalog({ 'arduino-opta.toml': optaV2(), 'simulator.toml': sim.text }, {})
    const u = catalogUpdates(p, newer)
    expect(u).toHaveLength(1)
    expect(u[0]).toMatchObject({ path: 'devices/arduino-opta.toml', current: { device: { version: 1 } }, latest: { device: { device: { version: 2 } } } })
    const older = buildCatalog({ 'arduino-opta.toml': opta.text.replace('version = 1', 'version = 1') }, {})
    expect(catalogUpdates({ ...p, deviceFiles: { 'devices/arduino-opta.toml': optaV2() } }, older)).toEqual([])
  })

  it('diffs manifests by line', () => {
    const d = lineDiff('a\nb\nc', 'a\nx\nc\nd')
    expect(d).toEqual([
      { op: ' ', text: 'a' },
      { op: '-', text: 'b' },
      { op: '+', text: 'x' },
      { op: ' ', text: 'c' },
      { op: '+', text: 'd' },
    ])
    const real = lineDiff(opta.text, optaV2()).filter((x) => x.op !== ' ')
    expect(real.map((x) => x.op + x.text)).toEqual([
      '-version = 1',
      '+version = 2',
      '-address = "%QX0.{n-1}"',
      '+address = "%QX0.{n}"',
      '-address = "%QX0.4"',
      '+address = "%QX0.6"',
    ])
  })

  it('remaps only points whose direction and size match', () => {
    const from = loadManifest(opta.text).device!
    const to = loadManifest(opta.text.replace('type = "INT"\naddress = "%IW{n}"', 'type = "BOOL"\naddress = "%IX1.{n-1}"').replace('kind = "analog"', 'kind = "digital"').replace('range = [0, 4095]\neng = [0.0, 10.877]\nunits = "V"\n', '')).device!
    const { tags, report } = remapTags([{ name: 'Level', data_type: 'INT', section: 'global', address: '%IW2' }], from, to)
    // I2 exists on both, but only as a bit on `to`: a word tag must not land on a bit.
    expect(tags[0].address).toBe('%IW2')
    expect(report.warnings[0]).toMatch(/Level: terminal I2 is not on Arduino Opta/)
  })
})
