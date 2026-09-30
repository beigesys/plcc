// SPDX-License-Identifier: MPL-2.0
//
// Built-in flash limits per bootloader USB id. These are code, not data: a
// device manifest (docs/device-manifest.md) or any DeviceProfile can only
// narrow them, never widen them, and a USB id without an entry here cannot be
// flashed at all. Adding a board means adding an entry, in review.

/** The widest a flash to one bootloader may ever go. */
export interface Floor {
  /** Board name for messages. */
  name: string;
  /** Lowest address any erase or write may touch. */
  minAddress: number;
  /** One past the highest address any write may touch. */
  end: number;
  /** DFU alternate settings that may be written through. */
  alts: readonly number[];
  /** The alternate's DfuSe layout name must start with this. */
  layoutName: string;
  /** USB ids of the running application that a 1200-baud touch may be sent to. */
  runtime: readonly { usbVendorId: number; usbProductId: number }[];
}

const ARDUINO_RUNTIME = [0x0064, 0x0164, 0x0264].flatMap((pid) => [
  { usbVendorId: 0x2341, usbProductId: pid },
  { usbVendorId: 0x35d1, usbProductId: pid },
]);

/**
 * Arduino Opta (STM32H747) bootloader. The first 256 KiB hold the Arduino
 * bootloader (sector 0 is read-only in the DfuSe layout, sector 1 is not but
 * is still bootloader); applications live at 0x08040000 up to the end of the
 * 2 MiB flash (mbed_opta 4.6.0: upload.address, APPLICATION_SIZE 0x1C0000).
 */
const OPTA_FLOOR: Floor = {
  name: "Arduino Opta",
  minAddress: 0x08040000,
  end: 0x08200000,
  alts: [0],
  layoutName: "Internal Flash",
  runtime: ARDUINO_RUNTIME,
};

/** Keyed `vvvv:pppp` (lowercase hex). */
export const FLOORS: Record<string, Floor> = {
  "2341:0364": OPTA_FLOOR,
  "35d1:0364": OPTA_FLOOR,
};

export function usbKey(vendorId: number, productId: number): string {
  const h = (n: number) => n.toString(16).padStart(4, "0");
  return `${h(vendorId)}:${h(productId)}`;
}

/** The built-in limits for a bootloader USB id, if webdfu knows it. */
export function floorFor(vendorId: number, productId: number): Floor | undefined {
  return Object.prototype.hasOwnProperty.call(FLOORS, usbKey(vendorId, productId)) ? FLOORS[usbKey(vendorId, productId)] : undefined;
}
