// SPDX-License-Identifier: MPL-2.0

/**
 * What may be flashed to one kind of board, and where. A profile can only
 * narrow the built-in limits for its bootloader's USB id (floors.ts); build
 * one from a device manifest with profileFromManifest.
 */
export interface DeviceProfile {
  name: string;
  /** WebUSB filters matching the board in its DFU bootloader. */
  dfuFilters: { vendorId: number; productId: number }[];
  /** USB ids of the board running its application (a serial port), for the 1200-baud touch. */
  runtimeFilters: { usbVendorId: number; usbProductId: number }[];
  /** DFU alternate setting to write through. */
  alt: number;
  /** The alternate's DfuSe layout name must start with this (after `@`). */
  layoutName: string;
  /** Lowest address any erase or write may touch. Everything below is protected. */
  minAddress: number;
  /** Where application images go. */
  address: number;
  /** Largest image, bytes. */
  maxSize: number;
  /** Leave DFU mode (start the application) after a download. */
  leave: boolean;
}

/**
 * Arduino Opta (STM32H747, main core). Values from the Arduino mbed_opta core
 * 4.6.0 (`boards.txt`: `opta.upload.tool=dfu-util`, `upload.vid=0x2341`,
 * `upload.pid=0x0364`, `upload.interface=0`, `upload.address=0x08040000`,
 * `upload.use_1200bps_touch=true`; `platform.txt`: `dfu-util --device
 * {vid}:{pid} -D {bin} -a{interface} --dfuse-address={address}:leave`), and the bootloader's
 * alt 0 layout "@Internal Flash  2MB   /0x08000000/01*128Ka,15*128Kg".
 *
 * The first 256 KiB (0x08000000-0x0803FFFF) hold the Arduino bootloader:
 * sector 0 is read-only in the layout, sector 1 is not but is still
 * bootloader, so `minAddress` protects both.
 */
export const OPTA: DeviceProfile = {
  name: "Arduino Opta",
  dfuFilters: [
    { vendorId: 0x2341, productId: 0x0364 },
    { vendorId: 0x35d1, productId: 0x0364 },
  ],
  runtimeFilters: [0x0064, 0x0164, 0x0264].flatMap((pid) => [
    { usbVendorId: 0x2341, usbProductId: pid },
    { usbVendorId: 0x35d1, usbProductId: pid },
  ]),
  alt: 0,
  layoutName: "Internal Flash",
  minAddress: 0x08040000,
  address: 0x08040000,
  // 0x08040000-0x0817FFFF: the runtime. The last 512 KiB (0x08180000-) are
  // the program slot (OPTA_PROGRAM, docs/program-image.md), so flashing a
  // runtime never touches a downloaded program. (boards.txt says 1966080 =
  // 2 MiB - 128 KiB, which would run 128 KiB past the end of flash.)
  maxSize: 0x140000,
  leave: true,
};

export const PROFILES: Record<string, DeviceProfile> = { opta: OPTA };
