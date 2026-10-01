// SPDX-License-Identifier: MPL-2.0
//
// USB Device Firmware Upgrade, version 1.1 (USB-IF, "Universal Serial Bus
// Device Class Specification for Device Firmware Upgrade", 2004): class
// requests (§3, table 3.2), the state machine (§6.1.2, fig. A.1), and the
// DFU_GETSTATUS payload (§6.1.2). Written from the specification.

import type { UsbDeviceLike, UsbSetup } from "./usb";

/** bRequest values (DFU 1.1 table 3.2). */
export const Request = {
  DETACH: 0,
  DNLOAD: 1,
  UPLOAD: 2,
  GETSTATUS: 3,
  CLRSTATUS: 4,
  GETSTATE: 5,
  ABORT: 6,
} as const;

/** bState values (DFU 1.1 §6.1.2). */
export const State = {
  appIDLE: 0,
  appDETACH: 1,
  dfuIDLE: 2,
  dfuDNLOAD_SYNC: 3,
  dfuDNBUSY: 4,
  dfuDNLOAD_IDLE: 5,
  dfuMANIFEST_SYNC: 6,
  dfuMANIFEST: 7,
  dfuMANIFEST_WAIT_RESET: 8,
  dfuUPLOAD_IDLE: 9,
  dfuERROR: 10,
} as const;

const STATE_NAMES = Object.fromEntries(Object.entries(State).map(([k, v]) => [v, k])) as Record<number, string>;
export const stateName = (s: number) => STATE_NAMES[s] ?? `state ${s}`;

/** bStatus values and their meaning (DFU 1.1 §6.1.2, table). */
export const STATUS: Record<number, [string, string]> = {
  0x00: ["OK", "no error"],
  0x01: ["errTARGET", "file is not targeted for use by this device"],
  0x02: ["errFILE", "file is for this device but fails a verification test"],
  0x03: ["errWRITE", "device is unable to write memory"],
  0x04: ["errERASE", "memory erase function failed"],
  0x05: ["errCHECK_ERASED", "memory erase check failed"],
  0x06: ["errPROG", "program memory function failed"],
  0x07: ["errVERIFY", "programmed memory failed verification"],
  0x08: ["errADDRESS", "cannot program memory due to received address that is out of range"],
  0x09: ["errNOTDONE", "received DFU_DNLOAD with wLength = 0, but the firmware is incomplete"],
  0x0a: ["errFIRMWARE", "device's firmware is corrupt; it cannot return to run-time operations"],
  0x0b: ["errVENDOR", "vendor-specific error"],
  0x0c: ["errUSBR", "device detected an unexpected USB reset"],
  0x0d: ["errPOR", "device detected an unexpected power on reset"],
  0x0e: ["errUNKNOWN", "something went wrong, but the device does not know what"],
  0x0f: ["errSTALLEDPKT", "device stalled an unexpected request"],
};

export interface DfuStatus {
  status: number;
  pollTimeoutMs: number;
  state: number;
  iString: number;
}

/** A failure the device reported, or a transfer that failed. */
export class DfuError extends Error {
  readonly status?: number;
  readonly state?: number;

  constructor(message: string, status?: number, state?: number) {
    super(message);
    this.name = "DfuError";
    this.status = status;
    this.state = state;
  }
}

export function describeStatus(s: DfuStatus): string {
  const [name, text] = STATUS[s.status] ?? [`status 0x${s.status.toString(16)}`, "unknown status"];
  return `${name} (${text}) in ${stateName(s.state)}`;
}

/** The DFU functional descriptor (DFU 1.1 §4.1.3, table 4.2). */
export interface FunctionalDescriptor {
  canDownload: boolean;
  canUpload: boolean;
  manifestationTolerant: boolean;
  willDetach: boolean;
  detachTimeoutMs: number;
  transferSize: number;
  /** 0x0110 for DFU 1.1, 0x011a for ST DfuSe. */
  dfuVersion: number;
}

export type Sleep = (ms: number) => Promise<void>;
export const realSleep: Sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** Class requests on one DFU interface. */
export class DfuInterface {
  readonly device: UsbDeviceLike;
  readonly interfaceNumber: number;
  readonly sleep: Sleep;

  constructor(device: UsbDeviceLike, interfaceNumber: number, sleep: Sleep = realSleep) {
    this.device = device;
    this.interfaceNumber = interfaceNumber;
    this.sleep = sleep;
  }

  private setup(request: number, value = 0): UsbSetup {
    return { requestType: "class", recipient: "interface", request, value, index: this.interfaceNumber };
  }

  private async out(request: number, value: number, data?: Uint8Array): Promise<number> {
    const r = await this.device.controlTransferOut(this.setup(request, value), data as BufferSource | undefined);
    if (r.status !== "ok") throw new DfuError(`${requestName(request)} failed: ${r.status}`);
    return r.bytesWritten;
  }

  private async in(request: number, length: number): Promise<DataView> {
    const r = await this.device.controlTransferIn(this.setup(request), length);
    if (r.status !== "ok" || !r.data) throw new DfuError(`${requestName(request)} failed: ${r.status}`);
    return r.data;
  }

  /** DFU_DNLOAD block `block` (wValue) with `data`. */
  download(block: number, data: Uint8Array): Promise<number> {
    return this.out(Request.DNLOAD, block, data);
  }

  async getStatus(): Promise<DfuStatus> {
    const d = await this.in(Request.GETSTATUS, 6);
    if (d.byteLength < 6) throw new DfuError(`DFU_GETSTATUS returned ${d.byteLength} bytes, not 6`);
    return {
      status: d.getUint8(0),
      pollTimeoutMs: d.getUint8(1) | (d.getUint8(2) << 8) | (d.getUint8(3) << 16),
      state: d.getUint8(4),
      iString: d.getUint8(5),
    };
  }

  async getState(): Promise<number> {
    return (await this.in(Request.GETSTATE, 1)).getUint8(0);
  }

  async clearStatus(): Promise<void> {
    await this.out(Request.CLRSTATUS, 0);
  }

  async abort(): Promise<void> {
    await this.out(Request.ABORT, 0);
  }

  /**
   * Poll DFU_GETSTATUS, honouring bwPollTimeout, until the state is no longer
   * `busy` (dfuDNBUSY, or dfuMANIFEST). Returns the final status. A device in
   * dfuERROR is cleared (DFU_CLRSTATUS) and the error thrown.
   */
  async pollUntilIdle(what: string, timeoutMs = 30_000): Promise<DfuStatus> {
    let waited = 0;
    for (;;) {
      const s = await this.getStatus();
      if (s.state === State.dfuERROR || s.status !== 0) {
        await this.clearStatus().catch(() => {});
        throw new DfuError(`${what}: the device reported ${describeStatus(s)}`, s.status, s.state);
      }
      if (s.state !== State.dfuDNBUSY && s.state !== State.dfuMANIFEST) return s;
      if (waited > timeoutMs) throw new DfuError(`${what}: still ${stateName(s.state)} after ${timeoutMs} ms`);
      await this.sleep(s.pollTimeoutMs);
      waited += Math.max(1, s.pollTimeoutMs);
    }
  }

  /** Bring the interface to dfuIDLE from wherever a previous session left it. */
  async toIdle(): Promise<void> {
    let s = await this.getStatus();
    if (s.state === State.dfuERROR) {
      await this.clearStatus();
      s = await this.getStatus();
    }
    if (s.state === State.dfuDNLOAD_IDLE || s.state === State.dfuUPLOAD_IDLE) {
      await this.abort();
      s = await this.getStatus();
    }
    if (s.state !== State.dfuIDLE) {
      throw new DfuError(`the device is in ${stateName(s.state)}, not dfuIDLE: is it in its bootloader?`, s.status, s.state);
    }
  }
}

function requestName(r: number): string {
  return `DFU_${Object.entries(Request).find(([, v]) => v === r)?.[0] ?? r}`;
}

/** Standard GET_DESCRIPTOR (USB 2.0 §9.4.3). */
export async function getDescriptor(device: UsbDeviceLike, type: number, index: number, length: number, langId = 0): Promise<DataView> {
  const r = await device.controlTransferIn(
    { requestType: "standard", recipient: "device", request: 0x06, value: (type << 8) | index, index: langId },
    length,
  );
  if (r.status !== "ok" || !r.data) throw new DfuError(`GET_DESCRIPTOR(${type}, ${index}) failed: ${r.status}`);
  return r.data;
}

/** A string descriptor as text (UTF-16LE), in the device's first language. */
export async function getString(device: UsbDeviceLike, index: number): Promise<string> {
  const langs = await getDescriptor(device, 3, 0, 255);
  const lang = langs.byteLength >= 4 ? langs.getUint16(2, true) : 0x0409;
  const d = await getDescriptor(device, 3, index, 255, lang);
  let s = "";
  for (let i = 2; i + 1 < Math.min(d.byteLength, d.getUint8(0)); i += 2) s += String.fromCharCode(d.getUint16(i, true));
  return s;
}

export interface InterfaceDescriptors {
  /** (interface, alternate) → iInterface string index. */
  names: Map<string, number>;
  functional: FunctionalDescriptor | null;
}

/** Parse the active configuration descriptor: iInterface indices and the DFU functional descriptor. */
export async function readConfigDescriptors(device: UsbDeviceLike, configIndex = 0): Promise<InterfaceDescriptors> {
  const head = await getDescriptor(device, 2, configIndex, 9);
  const total = head.getUint16(2, true);
  const d = await getDescriptor(device, 2, configIndex, total);
  const names = new Map<string, number>();
  let functional: FunctionalDescriptor | null = null;
  for (let i = 0; i + 1 < d.byteLength; ) {
    const len = d.getUint8(i);
    const type = d.getUint8(i + 1);
    if (len < 2) break;
    if (type === 4 && len >= 9) names.set(`${d.getUint8(i + 2)}:${d.getUint8(i + 3)}`, d.getUint8(i + 8));
    if (type === 0x21 && len >= 7) {
      const attrs = d.getUint8(i + 2);
      functional = {
        canDownload: (attrs & 1) !== 0,
        canUpload: (attrs & 2) !== 0,
        manifestationTolerant: (attrs & 4) !== 0,
        willDetach: (attrs & 8) !== 0,
        detachTimeoutMs: d.getUint16(i + 3, true),
        transferSize: d.getUint16(i + 5, true),
        dfuVersion: len >= 9 ? d.getUint16(i + 7, true) : 0x0100,
      };
    }
    i += len;
  }
  return { names, functional };
}
