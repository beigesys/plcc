// SPDX-License-Identifier: MPL-2.0
//
// "Add device → Detect": open a console, ask `info`, close. The answer names
// the device id, the manifest version the runtime was built for, its runtime
// kind and ABI, and the image sizes; the caller matches it to the catalog.

import { CATALOG, type CatalogEntry } from '@/devices/catalog'
import { checkIdentity, parseInfoLine, type DeviceIdentity, type IdentityCheck } from './protocol'
import type { SerialTransport } from './transport'

/** Asks a device who it is. The transport is opened and closed here. */
export async function identifyDevice(transport: SerialTransport, timeoutMs = 2000): Promise<DeviceIdentity> {
  await transport.open()
  const unsubs: (() => void)[] = []
  try {
    return await new Promise<DeviceIdentity>((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`no answer to "info" within ${timeoutMs} ms`)), timeoutMs)
      unsubs.push(() => clearTimeout(timer))
      unsubs.push(
        transport.onLine((line) => {
          const id = parseInfoLine(line)
          if (id) resolve(id)
          else if (line.startsWith('?')) reject(new Error(`the device does not know "info" (${line.trim()}); its runtime predates device manifests`))
        }),
        transport.onClose((reason) => reject(new Error(reason ?? 'connection closed'))),
      )
      transport.writeLine('info').catch(reject)
    })
  } finally {
    for (const u of unsubs) u()
    try {
      await transport.close()
    } catch {
      // Already gone.
    }
  }
}

export interface Detection {
  identity: DeviceIdentity
  /** The catalog manifest with the reported id, if any. */
  entry?: CatalogEntry
  /** The identity checked against that manifest. */
  check?: IdentityCheck
}

/** Matches an identity to the catalog. */
export function matchCatalog(identity: DeviceIdentity, catalog: CatalogEntry[] = CATALOG): Detection {
  const entry = catalog.find((e) => e.id === identity.device)
  return { identity, entry, check: entry ? checkIdentity(identity, entry.device) : undefined }
}
