// SPDX-License-Identifier: MPL-2.0
//
// The device catalog, bundled at build time: every manifest in the
// repository's `devices/` directory (a git submodule), falling back to the
// built-in Opta and Simulator copies in crates/plcc-device/builtin when the
// submodule is not checked out. A catalog entry wins over a built-in one with
// the same id.

import { loadManifest, type Device } from './manifest'

export interface CatalogEntry {
  /** `device.id`. */
  id: string
  /** File name in the catalog (`arduino-opta.toml`). */
  file: string
  text: string
  device: Device
  origin: 'catalog' | 'builtin'
}

const catalogFiles = import.meta.glob('../../../devices/*.toml', { query: '?raw', import: 'default', eager: true }) as Record<
  string,
  string
>
const builtinFiles = import.meta.glob('../../../crates/plcc-device/builtin/*.toml', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>

function entries(files: Record<string, string>, origin: CatalogEntry['origin']): CatalogEntry[] {
  const out: CatalogEntry[] = []
  for (const [path, text] of Object.entries(files)) {
    const file = path.split('/').pop() ?? path
    const { device, diagnostics } = loadManifest(text)
    if (!device) {
      // A broken catalog file is a build-time bug; keep the rest usable.
      console.error(`device catalog: ${file}: ${diagnostics.map((d) => d.message).join('; ')}`)
      continue
    }
    out.push({ id: device.device.id, file, text, device, origin })
  }
  return out
}

/** Builds a catalog from file maps (exported for tests). */
export function buildCatalog(catalog: Record<string, string>, builtin: Record<string, string>): CatalogEntry[] {
  const out = entries(catalog, 'catalog')
  for (const b of entries(builtin, 'builtin')) if (!out.some((e) => e.id === b.id)) out.push(b)
  return out.sort((a, b) => a.device.device.name.localeCompare(b.device.device.name))
}

export const CATALOG: CatalogEntry[] = buildCatalog(catalogFiles, builtinFiles)

export function catalogEntry(id: string): CatalogEntry | undefined {
  return CATALOG.find((e) => e.id === id)
}

/** The Simulator manifest: the device of a project with none. */
export function simulatorEntry(): CatalogEntry {
  const e = catalogEntry('simulator')
  if (!e) throw new Error('the simulator manifest is missing from the build')
  return e
}
