// SPDX-License-Identifier: MPL-2.0
//
// ST DfuSe extensions to DFU 1.1 (ST AN3156, "USB DFU protocol used in the
// STM32 bootloader", §6: DFU_DNLOAD with wBlockNum = 0 carries a command —
// 0x21 Set Address Pointer, 0x41 Erase (page, or mass erase without an
// address); wBlockNum ≥ 2 writes wTransferSize bytes at
// pointer + (wBlockNum − 2) × wTransferSize; a zero-length DFU_DNLOAD leaves
// DFU mode and starts the application at the pointer). Written from the
// application note and the DFU 1.1 specification; the flash sequence matches
// what dfu-util's `--dfuse-address=<addr>:leave` does.
//
// Safety: every erase and write is checked against the alternate setting's
// memory layout, the built-in limits for the bootloader's USB id (floors.ts)
// and the device profile before anything is sent. A sector the layout marks
// read-only or not erasable/writable, any address below the floor's or the
// profile's minimum, and anything past the layout, the floor or `maxSize` is
// refused, whatever the profile says. A USB id without a floor is refused.
// There is no mass erase.

import { DfuError, DfuInterface, type FunctionalDescriptor, type Sleep, State, getString, readConfigDescriptors, realSleep } from "./dfu";
import { type Floor, floorFor } from "./floors";
import { type MemoryLayout, type Sector, hex, parseLayout, sectorAt } from "./layout";
import type { DeviceProfile } from "./profiles";
import type { UsbDeviceLike } from "./usb";

const CMD_SET_ADDRESS = 0x21;
const CMD_ERASE = 0x41;

/** A write the safety rules refuse. Nothing was sent to the device. */
export class SafetyError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "SafetyError";
  }
}

export type Phase = "erase" | "write" | "verify" | "manifest";

export interface Progress {
  phase: Phase;
  done: number;
  total: number;
}

export interface FlashOptions {
  /** Default: the profile's address. */
  address?: number;
  /** Default: the profile's `leave`. */
  leave?: boolean;
  /** Read the image back with DFU_UPLOAD and compare. */
  verify?: boolean;
  onProgress?: (p: Progress) => void;
}

export interface OpenOptions {
  profile: DeviceProfile;
  sleep?: Sleep;
  /** Override the functional descriptor's wTransferSize. */
  transferSize?: number;
}

export interface WritePlan {
  address: number;
  length: number;
  /** Sectors to erase, in order. */
  sectors: Sector[];
}

/** One claimed DfuSe interface, ready to flash within a profile's limits. */
export class DfuseDevice {
  private constructor(
    readonly device: UsbDeviceLike,
    readonly dfu: DfuInterface,
    readonly profile: DeviceProfile,
    readonly layout: MemoryLayout,
    readonly layoutText: string,
    readonly transferSize: number,
    readonly functional: FunctionalDescriptor | null,
    /** The built-in limits for this bootloader, which no profile can widen. */
    readonly floor: Floor,
  ) {}

  /**
   * Open `device` (a USBDevice in DFU mode), claim the profile's DFU
   * alternate setting and read its memory layout. Refuses a device the
   * profile does not describe.
   */
  static async open(device: UsbDeviceLike, opts: OpenOptions): Promise<DfuseDevice> {
    const { profile } = opts;
    if (!profile.dfuFilters.some((f) => f.vendorId === device.vendorId && f.productId === device.productId)) {
      throw new SafetyError(
        `USB ${hex4(device.vendorId)}:${hex4(device.productId)} is not ${profile.name} in DFU mode ` +
          `(expected ${profile.dfuFilters.map((f) => `${hex4(f.vendorId)}:${hex4(f.productId)}`).join(" or ")})`,
      );
    }
    const floor = floorFor(device.vendorId, device.productId);
    if (!floor) {
      throw new SafetyError(
        `webdfu has no built-in flash limits for USB ${hex4(device.vendorId)}:${hex4(device.productId)}; ` +
          "it will not flash a bootloader until they are added in code (src/floors.ts)",
      );
    }
    if (!floor.alts.includes(profile.alt)) {
      throw new SafetyError(`alternate setting ${profile.alt} is not one webdfu writes on ${floor.name} (${floor.alts.join(", ")})`);
    }
    if (!device.opened) await device.open();
    if (!device.configuration) await device.selectConfiguration(device.configurations[0]?.configurationValue ?? 1);
    const config = device.configuration!;
    const intf = config.interfaces.find((i) =>
      i.alternates.some((a) => a.alternateSetting === profile.alt && a.interfaceClass === 0xfe && a.interfaceSubclass === 0x01),
    );
    if (!intf) throw new DfuError(`no DFU interface with alternate setting ${profile.alt}`);
    await device.claimInterface(intf.interfaceNumber);
    await device.selectAlternateInterface(intf.interfaceNumber, profile.alt);

    const descriptors = await readConfigDescriptors(device).catch(() => null);
    let name = intf.alternates.find((a) => a.alternateSetting === profile.alt)?.interfaceName ?? null;
    if (!name) {
      const idx = descriptors?.names.get(`${intf.interfaceNumber}:${profile.alt}`);
      if (idx) name = await getString(device, idx);
    }
    if (!name) throw new SafetyError("the DFU alternate setting has no name, so its memory layout is unknown: refusing to write");
    const layout = parseLayout(name);
    for (const want of [profile.layoutName, floor.layoutName]) {
      if (!layout.name.startsWith(want)) {
        throw new SafetyError(`alternate ${profile.alt} is "${layout.name}", not "${want}": refusing to write`);
      }
    }
    const functional = descriptors?.functional ?? null;
    const transferSize = opts.transferSize ?? functional?.transferSize ?? 2048;
    if (transferSize <= 0 || transferSize > 65535) throw new DfuError(`bad transfer size ${transferSize}`);
    const dfu = new DfuInterface(device, intf.interfaceNumber, opts.sleep ?? realSleep);
    return new DfuseDevice(device, dfu, profile, layout, name, transferSize, functional, floor);
  }

  /**
   * Check a write of `length` bytes at `address` against the profile and the
   * memory layout, and list the sectors it erases. Throws SafetyError.
   */
  plan(address: number, length: number): WritePlan {
    const p = this.profile;
    if (!Number.isInteger(address) || !Number.isInteger(length) || address < 0 || length <= 0) {
      throw new SafetyError(`bad write: ${length} bytes at ${address}`);
    }
    const end = address + length;
    if (end > 0x1_0000_0000) throw new SafetyError(`the write ends past the 32-bit address space (${hex(address)} + ${length})`);
    const f = this.floor;
    if (address < f.minAddress) {
      throw new SafetyError(`refusing to write at ${hex(address)}: webdfu never writes below ${hex(f.minAddress)} on ${f.name} (the bootloader)`);
    }
    if (end > f.end) throw new SafetyError(`the write ends at ${hex(end)}, past ${hex(f.end)}, the end of ${f.name}'s application flash`);
    if (address < p.minAddress) {
      throw new SafetyError(
        `refusing to write at ${hex(address)}: ${p.name} protects everything below ${hex(p.minAddress)} (the bootloader)`,
      );
    }
    if (length > p.maxSize) throw new SafetyError(`the image is ${length} bytes; ${p.name} takes at most ${p.maxSize}`);
    if (end > p.address + p.maxSize) {
      throw new SafetyError(`the write ends at ${hex(end)}, past ${p.name}'s application area (${hex(p.address + p.maxSize)})`);
    }
    const sectors: Sector[] = [];
    for (let a = address; a < end; ) {
      const s = sectorAt(this.layout, a);
      if (!s) throw new SafetyError(`${hex(a)} is outside the device's memory layout "${this.layoutText}"`);
      this.checkSector(s);
      sectors.push(s);
      a = s.start + s.size;
    }
    return { address, length, sectors };
  }

  private checkSector(s: Sector): void {
    if (s.type === "a" || !s.writable || !s.erasable) {
      throw new SafetyError(
        `refusing to touch the sector at ${hex(s.start)} (${s.size / 1024} KiB, type '${s.type}'): the layout marks it ` +
          `${s.writable ? "" : "not writable"}${!s.writable && !s.erasable ? " and " : ""}${s.erasable ? "" : "not erasable"}`,
      );
    }
    const min = Math.max(this.profile.minAddress, this.floor.minAddress);
    if (s.start < min) {
      throw new SafetyError(`refusing to touch the sector at ${hex(s.start)}: below ${hex(min)}`);
    }
    if (s.start + s.size > this.floor.end) {
      throw new SafetyError(`refusing to touch the sector at ${hex(s.start)}: past ${hex(this.floor.end)}`);
    }
  }

  /** DfuSe command: DFU_DNLOAD block 0 with [cmd, address LE], then poll. */
  private async command(cmd: number, address: number, what: string): Promise<void> {
    const b = new Uint8Array(5);
    b[0] = cmd;
    new DataView(b.buffer).setUint32(1, address >>> 0, true);
    await this.dfu.download(0, b);
    await this.dfu.pollUntilIdle(what);
  }

  private async setAddress(address: number): Promise<void> {
    await this.command(CMD_SET_ADDRESS, address, `set address ${hex(address)}`);
  }

  private async eraseSector(s: Sector): Promise<void> {
    this.checkSector(s); // again, right before the command
    await this.command(CMD_ERASE, s.start, `erase ${hex(s.start)}`);
  }

  /** Erase, write, optionally verify and leave. */
  async flash(image: Uint8Array, opts: FlashOptions = {}): Promise<void> {
    const address = opts.address ?? this.profile.address;
    const plan = this.plan(address, image.length);
    const report = opts.onProgress ?? (() => {});
    await this.dfu.toIdle();

    const eraseTotal = plan.sectors.length;
    report({ phase: "erase", done: 0, total: eraseTotal });
    for (let i = 0; i < plan.sectors.length; i++) {
      await this.eraseSector(plan.sectors[i]);
      report({ phase: "erase", done: i + 1, total: eraseTotal });
    }

    report({ phase: "write", done: 0, total: image.length });
    for (let off = 0; off < image.length; off += this.transferSize) {
      const chunk = image.subarray(off, Math.min(image.length, off + this.transferSize));
      const at = address + off;
      this.plan(at, chunk.length); // every chunk re-checked
      await this.setAddress(at);
      await this.dfu.download(2, chunk);
      await this.dfu.pollUntilIdle(`write ${hex(at)}`);
      report({ phase: "write", done: off + chunk.length, total: image.length });
    }

    if (opts.verify) await this.verify(address, image, report);

    if (opts.leave ?? this.profile.leave) {
      report({ phase: "manifest", done: 0, total: 1 });
      await this.leave(address);
      report({ phase: "manifest", done: 1, total: 1 });
    } else {
      await this.dfu.abort();
    }
  }

  /** Read `image.length` bytes back at `address` (DFU_UPLOAD) and compare. */
  private async verify(address: number, image: Uint8Array, report: (p: Progress) => void): Promise<void> {
    report({ phase: "verify", done: 0, total: image.length });
    await this.dfu.toIdle();
    for (let off = 0; off < image.length; off += this.transferSize) {
      const n = Math.min(this.transferSize, image.length - off);
      await this.setAddress(address + off);
      await this.dfu.abort(); // back to dfuIDLE before an upload (AN3156 §6.4)
      const r = await this.device.controlTransferIn(
        { requestType: "class", recipient: "interface", request: 2, value: 2, index: this.dfu.interfaceNumber },
        n,
      );
      if (r.status !== "ok" || !r.data) throw new DfuError(`DFU_UPLOAD at ${hex(address + off)} failed: ${r.status}`);
      const got = new Uint8Array(r.data.buffer, r.data.byteOffset, r.data.byteLength);
      for (let i = 0; i < n; i++) {
        if (got[i] !== image[off + i]) {
          throw new DfuError(`verify failed at ${hex(address + off + i)}: wrote 0x${image[off + i].toString(16)}, read 0x${(got[i] ?? 0).toString(16)}`);
        }
      }
      await this.dfu.abort();
      report({ phase: "verify", done: off + n, total: image.length });
    }
  }

  /**
   * Leave DFU mode and start the application at `address`: set the address
   * pointer, a zero-length DFU_DNLOAD, then DFU_GETSTATUS to trigger
   * manifestation (AN3156 §6.3.4). The device resets, so the status request
   * may fail; that is expected.
   */
  async leave(address = this.profile.address): Promise<void> {
    if (address < this.profile.minAddress || address < this.floor.minAddress) {
      throw new SafetyError(`refusing to start the application at ${hex(address)}`);
    }
    await this.setAddress(address);
    await this.dfu.download(2, new Uint8Array(0));
    try {
      const s = await this.dfu.getStatus();
      if (s.state === State.dfuERROR || s.status !== 0) {
        throw new DfuError(`leaving DFU mode: status ${s.status} in state ${s.state}`, s.status, s.state);
      }
    } catch (e) {
      if (e instanceof DfuError && e.status !== undefined) throw e;
      // The device reset and dropped off the bus: success.
    }
    await this.device.close().catch(() => {});
  }
}

function hex4(n: number): string {
  return `0x${n.toString(16).padStart(4, "0")}`;
}
