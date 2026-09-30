// SPDX-License-Identifier: MPL-2.0
//
// The Arduino "1200-baud touch": opening the board's USB CDC serial port at
// 1200 baud and closing it (DTR dropped) reboots it into its bootloader.
//
// On the Opta this is implemented by the Arduino mbed core the runtime is
// built with (mbed_opta 4.6.0): cores/arduino/USB/USBSerial.cpp registers a
// line-coding callback in Serial.begin(); a baud rate of 1200 wakes a thread
// that waits for DTR to drop (port closed) and at least 200 ms, then calls
// _ontouch1200bps_() (variants/OPTA/variant.cpp), which writes 0xDF59 to RTC
// backup register DR0 and resets; the bootloader sees the magic and stays in
// DFU mode (USB 2341:0364). The plcc Opta runtime calls Serial.begin(115200)
// in setup(), so a running PLC program can always be touched.

import type { Sleep } from "./dfu";
import { realSleep } from "./dfu";
import type { DeviceProfile } from "./profiles";
import type { SerialPortLike, UsbDeviceLike } from "./usb";

/** Reboot an Arduino board into its bootloader through its serial port. */
export async function touch1200(port: SerialPortLike, sleep: Sleep = realSleep): Promise<void> {
  await port.open({ baudRate: 1200 });
  // Drop DTR explicitly (closing does too on most platforms); the core
  // reboots once DTR is low and 200 ms have passed since the baud change.
  await port.setSignals({ dataTerminalReady: false }).catch(() => {});
  await sleep(250);
  await port.close();
}

/** Whether a serial port belongs to a board of `profile` running its application. */
export function isRuntimePort(port: SerialPortLike, profile: DeviceProfile): boolean {
  const info = port.getInfo?.() ?? {};
  return profile.runtimeFilters.some((f) => f.usbVendorId === info.usbVendorId && f.usbProductId === info.usbProductId);
}

export interface UsbLike {
  getDevices(): Promise<UsbDeviceLike[]>;
}

/**
 * Wait until a device matching the profile's DFU filters is available to this
 * page (it must have been granted once with `navigator.usb.requestDevice({
 * filters: profile.dfuFilters })`, which needs a user gesture).
 */
export async function waitForDfuDevice(
  usb: UsbLike,
  profile: DeviceProfile,
  { timeoutMs = 10_000, pollMs = 250, sleep = realSleep }: { timeoutMs?: number; pollMs?: number; sleep?: Sleep } = {},
): Promise<UsbDeviceLike> {
  for (let waited = 0; ; waited += pollMs) {
    const devices = await usb.getDevices();
    const d = devices.find((d) => profile.dfuFilters.some((f) => f.vendorId === d.vendorId && f.productId === d.productId));
    if (d) return d;
    if (waited >= timeoutMs) {
      throw new Error(
        `${profile.name} did not appear in DFU mode within ${timeoutMs} ms (if this is the first time, ` +
          "grant access with navigator.usb.requestDevice)",
      );
    }
    await sleep(pollMs);
  }
}
