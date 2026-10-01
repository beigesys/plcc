// SPDX-License-Identifier: MPL-2.0
//
// A device manifest's [flash] section (docs/device-manifest.md) as a
// DeviceProfile. Manifests are untrusted data (downloaded, imported, part of a
// shared project), so they may only NARROW the built-in limits of their
// bootloader's USB id (floors.ts): a higher address, a smaller size, a subset
// of the ids. Anything wider is rejected with the reason; an id webdfu has no
// limits for is refused outright.

import { SafetyError } from "./dfuse";
import { type Floor, floorFor, usbKey } from "./floors";
import { hex } from "./layout";
import type { DeviceProfile } from "./profiles";

/** `[flash]` of an expanded manifest (plcc_device::Flash as JSON). */
export interface ManifestFlash {
  method: string;
  usb: { vid: number; pid: number }[];
  alt?: number;
  layout?: string;
  address: number;
  max_size: number;
  leave?: boolean;
  reboot?: string;
  runtime_usb?: { vid: number; pid: number }[];
  protected?: { start: number; size: number; reason?: string }[];
  /** [flash.program]: the slot program images are written to (docs/program-image.md). */
  program?: { format: number; address: number; max_size: number; ram?: { start: number; size: number }; services?: number };
}

/** A manifest that asks for more than webdfu allows. Nothing was flashed. */
export class ManifestError extends SafetyError {
  constructor(message: string) {
    super(message);
    this.name = "ManifestError";
  }
}

const isInt = (n: unknown): n is number => typeof n === "number" && Number.isInteger(n);

/**
 * The profile a manifest's [flash] describes, after checking it against the
 * built-in floors. `name` labels messages (the manifest's device name).
 */
export function profileFromManifest(flash: ManifestFlash, name = "device"): DeviceProfile {
  const fail = (m: string): never => {
    throw new ManifestError(`${name} manifest [flash]: ${m}`);
  };
  if (flash.method !== "dfuse") fail(`method "${String(flash.method)}" is not supported (only "dfuse")`);
  if (!Array.isArray(flash.usb) || flash.usb.length === 0) fail("no bootloader USB ids");
  const floors: Floor[] = [];
  for (const u of flash.usb) {
    if (!isInt(u?.vid) || !isInt(u?.pid)) fail("a USB id is not { vid, pid }");
    const f = floorFor(u.vid, u.pid);
    if (!f) {
      fail(
        `webdfu has no built-in flash limits for USB ${usbKey(u.vid, u.pid)}, so it will not flash it; ` +
          "limits are added in code (packages/webdfu/src/floors.ts), never from a manifest",
      );
    }
    floors.push(f!);
  }
  // Several ids: the narrowest of their floors.
  const minAddress = Math.max(...floors.map((f) => f.minAddress));
  const end = Math.min(...floors.map((f) => f.end));
  const alts = floors.map((f) => f.alts).reduce((a, b) => a.filter((x) => b.includes(x)));
  const layouts = [...new Set(floors.map((f) => f.layoutName))];
  if (layouts.length !== 1) fail("its USB ids belong to bootloaders with different memory layouts");
  const layoutName = layouts[0];

  const alt = flash.alt ?? 0;
  if (!isInt(alt) || !alts.includes(alt)) fail(`alternate setting ${String(alt)} is not allowed (allowed: ${alts.join(", ")})`);
  if (flash.layout !== undefined && !flash.layout.startsWith(layoutName)) {
    fail(`layout "${flash.layout}" is not "${layoutName}"`);
  }
  const { address, max_size: maxSize } = flash;
  if (!isInt(address) || !isInt(maxSize) || address < 0 || maxSize <= 0) fail("address and max_size must be positive integers");
  if (address < minAddress) {
    fail(`address ${hex(address)} is below ${hex(minAddress)}, the lowest address webdfu writes on this bootloader (its protected area)`);
  }
  if (address + maxSize > end) {
    fail(`${hex(address)} + max_size 0x${maxSize.toString(16)} ends at ${hex(address + maxSize)}, past ${hex(end)}, the end of the application flash`);
  }
  for (const r of flash.protected ?? []) {
    if (isInt(r.start) && isInt(r.size) && address < r.start + r.size && r.start < address + maxSize) {
      fail(`the application area overlaps its own protected region ${hex(r.start)}..${hex(r.start + r.size)}`);
    }
  }
  const allowedRuntime = floors.flatMap((f) => f.runtime);
  const runtimeFilters = (flash.runtime_usb ?? []).map((u) => {
    const ok = allowedRuntime.some((a) => a.usbVendorId === u.vid && a.usbProductId === u.pid);
    if (!ok) fail(`runtime USB id ${usbKey(u.vid, u.pid)} is not one this bootloader's board runs as`);
    return { usbVendorId: u.vid, usbProductId: u.pid };
  });
  if (flash.reboot === "1200-baud-touch" && runtimeFilters.length === 0) fail("1200-baud-touch needs runtime_usb");

  return {
    name,
    dfuFilters: flash.usb.map((u) => ({ vendorId: u.vid, productId: u.pid })),
    runtimeFilters,
    alt,
    layoutName,
    // Nothing below the application area, even where the floor would allow it.
    minAddress: Math.max(minAddress, address),
    address,
    maxSize,
    leave: flash.leave ?? true,
  };
}

/** Flash words: a program slot must start and end on them (STM32H7: 32 bytes). */
const FLASH_WORD = 32;

/**
 * The profile for writing a device's program slot ([flash.program],
 * docs/program-image.md): only the slot can be erased or written, and
 * leaving DFU mode starts the runtime at the application address. Built on
 * profileFromManifest, so every check of the application area applies; the
 * slot itself must lie inside the built-in floor, past the application area
 * (the runtime) and outside every protected region. It can only narrow.
 */
export function programProfileFromManifest(flash: ManifestFlash, name = "device"): DeviceProfile {
  const app = profileFromManifest(flash, name);
  const fail = (m: string): never => {
    throw new ManifestError(`${name} manifest [flash.program]: ${m}`);
  };
  const p = flash.program;
  if (!p) fail("missing: the device has no program slot, so program images cannot be written to it");
  const { address, max_size: size, format } = p!;
  if (format !== 1) fail(`program image format ${String(format)} is not one webdfu writes (1)`);
  if (!isInt(address) || !isInt(size) || address < 0 || size <= 0) fail("address and max_size must be positive integers");
  if (address % FLASH_WORD !== 0) fail(`address ${hex(address)} is not ${FLASH_WORD}-byte aligned`);
  const floor = floorFor(app.dfuFilters[0].vendorId, app.dfuFilters[0].productId)!;
  const minAddress = Math.max(...app.dfuFilters.map((f) => floorFor(f.vendorId, f.productId)!.minAddress));
  const end = Math.min(...app.dfuFilters.map((f) => floorFor(f.vendorId, f.productId)!.end));
  if (address < minAddress) fail(`address ${hex(address)} is below ${hex(minAddress)}, the lowest address webdfu writes on ${floor.name}`);
  if (address + size > end) fail(`${hex(address)} + max_size 0x${size.toString(16)} ends at ${hex(address + size)}, past ${hex(end)}`);
  if (address < app.address + app.maxSize && app.address < address + size) {
    fail(`the slot ${hex(address)}..${hex(address + size)} overlaps the application (runtime) area ${hex(app.address)}..${hex(app.address + app.maxSize)}`);
  }
  for (const r of flash.protected ?? []) {
    if (isInt(r.start) && isInt(r.size) && address < r.start + r.size && r.start < address + size) {
      fail(`the slot overlaps the protected region ${hex(r.start)}..${hex(r.start + r.size)}`);
    }
  }
  return {
    ...app,
    name: `${name} program slot`,
    minAddress: address,
    address,
    maxSize: size,
    entry: app.address,
  };
}
