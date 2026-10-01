// SPDX-License-Identifier: MPL-2.0
//
// A project's devices: each is a manifest file at `devices/<id>.toml` in the
// project, listed in project.toml. Adding a device copies a manifest into the
// project; the catalog can later offer a newer version, which is applied only
// when the user asks. Changing a device remaps tags by terminal name.

import type { DeviceRef, Project, Tag } from '@/model/types'
import { CATALOG, simulatorEntry, type CatalogEntry } from './catalog'
import { loadManifest, type Device, type IoPoint, type LoadedManifest } from './manifest'

export const DEVICE_PATH_RE = /^devices\/[a-z0-9]+(-[a-z0-9]+)*\.toml$/

export function devicePath(id: string): string {
  return `devices/${id}.toml`
}

const cache = new Map<string, LoadedManifest>()

/** loadManifest, memoized by text (manifests are small and rarely change). */
export function loadCached(text: string): LoadedManifest {
  let r = cache.get(text)
  if (!r) {
    r = loadManifest(text)
    if (cache.size > 64) cache.clear()
    cache.set(text, r)
  }
  return r
}

export interface ResolvedDevice {
  ref: DeviceRef | undefined
  device: Device
  /** Set when the project's manifest is missing or invalid and the Simulator stands in. */
  problem?: string
}

/** The device named `name` (default: the project's first), with its manifest expanded. */
export function resolveDevice(project: Project, name?: string): ResolvedDevice {
  const ref = (name !== undefined ? project.devices.find((d) => d.name === name) : undefined) ?? project.devices[0]
  if (!ref) return { ref, device: simulatorEntry().device }
  const text = project.deviceFiles[ref.manifest]
  if (text === undefined) return { ref, device: simulatorEntry().device, problem: `${ref.manifest} is missing` }
  const r = loadCached(text)
  if (!r.device) {
    const first = r.diagnostics.find((d) => d.severity === 'error')
    return { ref, device: simulatorEntry().device, problem: `${ref.manifest}: ${first?.message ?? 'invalid manifest'}` }
  }
  return { ref, device: r.device }
}

/** The project's first (controller) device. */
export function primaryDevice(project: Project): Device {
  return resolveDevice(project).device
}

function uniqueName(project: Project, base: string): string {
  const taken = new Set(project.devices.map((d) => d.name.toLowerCase()))
  if (!taken.has(base.toLowerCase())) return base
  for (let i = 2; ; i++) if (!taken.has(`${base} ${i}`.toLowerCase())) return `${base} ${i}`
}

/** Validates manifest text; throws with the first error. */
export function requireDevice(text: string): Device {
  const r = loadManifest(text)
  if (!r.device) {
    const errs = r.diagnostics.filter((d) => d.severity === 'error')
    throw new Error(
      errs
        .slice(0, 3)
        .map((d) => `${d.line ? `line ${d.line}: ` : ''}${d.path ? `${d.path}: ` : ''}${d.message}`)
        .join('\n') || 'invalid manifest',
    )
  }
  return r.device
}

/**
 * Puts manifest `text` in the project at devices/<id>.toml. The same text
 * already there is reused; a different manifest with the same id is refused
 * (update it instead), so adding never changes an existing device silently.
 */
function placeManifest(project: Project, text: string): { project: Project; path: string; device: Device } {
  const device = requireDevice(text)
  const path = devicePath(device.device.id)
  const existing = project.deviceFiles[path]
  if (existing !== undefined && existing !== text) {
    const cur = loadCached(existing).device
    throw new Error(
      `The project already has ${path}${cur ? ` (version ${cur.device.version})` : ''}. Use its update notice to change it, or remove the devices that use it first.`,
    )
  }
  return { project: existing === undefined ? { ...project, deviceFiles: { ...project.deviceFiles, [path]: text } } : project, path, device }
}

/** Adds a device from manifest text (catalog entry, imported file or detected device). */
export function addDevice(project: Project, text: string, name?: string): { project: Project; name: string } {
  const placed = placeManifest(project, text)
  const n = uniqueName(placed.project, name?.trim() || placed.device.device.name)
  return { project: { ...placed.project, devices: [...placed.project.devices, { name: n, manifest: placed.path }] }, name: n }
}

/** Removes a device; its manifest file goes too when no other device uses it. */
export function removeDevice(project: Project, name: string): Project {
  const ref = project.devices.find((d) => d.name === name)
  if (!ref) return project
  const devices = project.devices.filter((d) => d !== ref)
  const deviceFiles = { ...project.deviceFiles }
  if (!devices.some((d) => d.manifest === ref.manifest)) delete deviceFiles[ref.manifest]
  return { ...project, devices, deviceFiles }
}

export interface RemapReport {
  /** Tags whose address changed. */
  moved: { tag: string; from: string; to: string; terminal: string }[]
  /** Tags that could not be placed on the new device (their address is kept). */
  warnings: string[]
}

const norm = (s: string) => s.trim().toUpperCase()

function sizeOf(address: string): string {
  const m = /^%[IQM]([XBWDL])/i.exec(address)
  return m ? m[1].toUpperCase() : 'X'
}

/** The point on `to` that takes over `p` from the old device: same terminal, closest kind. */
function counterpart(p: IoPoint, to: Device): IoPoint | undefined {
  const same = to.io.filter((q) => norm(q.terminal) === norm(p.terminal))
  const score = (q: IoPoint) =>
    (q.kind === p.kind ? 4 : 0) + (q.dir === p.dir ? 2 : 0) + (sizeOf(q.address) === sizeOf(p.address) ? 1 : 0)
  const best = same.sort((a, b) => score(b) - score(a))[0]
  // Different direction or address size is not the same kind of point.
  if (!best || best.dir !== p.dir || sizeOf(best.address) !== sizeOf(p.address)) return undefined
  return best
}

function fitsImage(address: string, d: Device): boolean {
  const m = /^%([IQM])([XBWDL]?)(\d+)(?:\.(\d))?$/i.exec(address.trim())
  if (!m) return false
  const area = m[1].toUpperCase() as 'I' | 'Q' | 'M'
  const w = { X: 1, '': 1, B: 1, W: 2, D: 4, L: 8 }[m[2].toUpperCase() as 'X'] ?? 1
  const byte = (m[2].toUpperCase() === 'X' || m[2] === '' ? 1 : w) * Number(m[3])
  return byte + w <= d.target.image[area]
}

/** Moves tag addresses from `from`'s points to `to`'s points with the same terminal. */
export function remapTags(tags: Tag[], from: Device, to: Device): { tags: Tag[]; report: RemapReport } {
  const report: RemapReport = { moved: [], warnings: [] }
  const out = tags.map((t) => {
    if (!t.address) return t
    const addr = norm(t.address)
    const old = from.io.find((p) => norm(p.address) === addr)
    const onNew = to.io.find((p) => norm(p.address) === addr)
    if (!old) {
      if (!onNew && !fitsImage(t.address, to)) {
        report.warnings.push(`${t.name} (${t.address}) is outside ${to.device.name}'s process image`)
      }
      return t
    }
    const q = counterpart(old, to)
    if (!q) {
      report.warnings.push(
        onNew
          ? `${t.name}: terminal ${old.terminal} is not on ${to.device.name}; ${t.address} is ${onNew.terminal} (${onNew.label}) there`
          : `${t.name}: terminal ${old.terminal} is not on ${to.device.name}; ${t.address} kept but not on the device`,
      )
      return t
    }
    if (norm(q.address) === addr) return t
    report.moved.push({ tag: t.name, from: t.address, to: q.address, terminal: q.terminal })
    return { ...t, address: q.address }
  })
  return { tags: out, report }
}

/** Switches device `name` to the manifest `text`, remapping tags by terminal. */
export function changeDevice(project: Project, name: string, text: string): { project: Project; report: RemapReport } {
  const ref = project.devices.find((d) => d.name === name)
  if (!ref) throw new Error(`no device "${name}"`)
  const before = resolveDevice(project, name).device
  const placed = placeManifest(removeDevice(project, name), text)
  const devices = project.devices.map((d) => (d === ref ? { name: d.name, manifest: placed.path } : d))
  const { tags, report } = remapTags(project.globals, before, placed.device)
  return { project: { ...placed.project, devices, globals: tags }, report }
}

/** Replaces a project manifest file with a newer text (a catalog update), remapping tags. */
export function updateManifestFile(project: Project, path: string, text: string): { project: Project; report: RemapReport } {
  const oldText = project.deviceFiles[path]
  const next = requireDevice(text)
  const old = oldText !== undefined ? loadCached(oldText).device : undefined
  if (devicePath(next.device.id) !== path) throw new Error(`the new manifest is ${next.device.id}, not ${path}`)
  const { tags, report } = old ? remapTags(project.globals, old, next) : { tags: project.globals, report: { moved: [], warnings: [] } }
  return { project: { ...project, deviceFiles: { ...project.deviceFiles, [path]: text }, globals: tags }, report }
}

export interface ManifestUpdate {
  path: string
  current: Device
  latest: CatalogEntry
}

/** Project manifests for which the catalog has a newer version. */
export function catalogUpdates(project: Project, catalog: CatalogEntry[] = CATALOG): ManifestUpdate[] {
  const out: ManifestUpdate[] = []
  for (const [path, text] of Object.entries(project.deviceFiles)) {
    const cur = loadCached(text).device
    if (!cur) continue
    const latest = catalog.find((e) => e.id === cur.device.id)
    if (latest && latest.device.device.version > cur.device.version) out.push({ path, current: cur, latest })
  }
  return out
}

export interface DiffLine {
  op: ' ' | '-' | '+'
  text: string
}

/** Line diff (LCS). Manifests are a few hundred lines, so O(n·m) is fine. */
export function lineDiff(a: string, b: string): DiffLine[] {
  const x = a.split('\n')
  const y = b.split('\n')
  const n = x.length
  const m = y.length
  const lcs: number[][] = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0))
  for (let i = n - 1; i >= 0; i--)
    for (let j = m - 1; j >= 0; j--) lcs[i][j] = x[i] === y[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1])
  const out: DiffLine[] = []
  let i = 0
  let j = 0
  while (i < n && j < m) {
    if (x[i] === y[j]) {
      out.push({ op: ' ', text: x[i] })
      i++
      j++
    } else if (lcs[i + 1][j] >= lcs[i][j + 1]) out.push({ op: '-', text: x[i++] })
    else out.push({ op: '+', text: y[j++] })
  }
  while (i < n) out.push({ op: '-', text: x[i++] })
  while (j < m) out.push({ op: '+', text: y[j++] })
  return out
}

/** Manifest text for a legacy `profile = "<id>"` device (projects saved before manifests). */
export function legacyProfileManifest(profile: string): { path: string; text: string } {
  const e = CATALOG.find((c) => c.id === profile) ?? simulatorEntry()
  return { path: devicePath(e.id), text: e.text }
}
