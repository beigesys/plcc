// SPDX-License-Identifier: MPL-2.0
//
// A fake USBDevice: an STM32-style DfuSe bootloader with flash that only
// erases to 0xFF and programs 1 → 0, a read-only sector, the DFU 1.1 state
// machine (sync/busy/idle/error/manifest), descriptors, and fault injection.

import type { UsbDeviceLike, UsbInResult, UsbOutResult, UsbSetup } from "../src/usb";

const S = { dfuIDLE: 2, dfuDNLOAD_SYNC: 3, dfuDNBUSY: 4, dfuDNLOAD_IDLE: 5, dfuMANIFEST_SYNC: 6, dfuMANIFEST: 7, dfuUPLOAD_IDLE: 9, dfuERROR: 10 };
const ERR = { OK: 0, errTARGET: 1, errWRITE: 3, errERASE: 4, errPROG: 6, errVERIFY: 7, errADDRESS: 8, errSTALLEDPKT: 0x0f };

export const OPTA_LAYOUT = "@Internal Flash  2MB   /0x08000000/01*128Ka,15*128Kg";

export interface FakeOptions {
  vendorId?: number;
  productId?: number;
  layout?: string;
  /** Expose the layout as `interfaceName` (WebUSB); false: only via string descriptors. */
  interfaceName?: boolean;
  transferSize?: number;
  initialState?: number;
  /** Erasing this sector address fails with errERASE. */
  failEraseAt?: number;
  /** Erase reports success but leaves the flash as it was. */
  brokenErase?: boolean;
  /** Polls reporting dfuDNBUSY before an operation completes. */
  busyPolls?: number;
  pollTimeoutMs?: number;
}

function str(s: string): Uint8Array {
  const b = new Uint8Array(2 + 2 * s.length);
  b[0] = b.length;
  b[1] = 3;
  for (let i = 0; i < s.length; i++) {
    b[2 + 2 * i] = s.charCodeAt(i) & 0xff;
    b[3 + 2 * i] = s.charCodeAt(i) >> 8;
  }
  return b;
}

export class FakeDfuDevice implements UsbDeviceLike {
  vendorId: number;
  productId: number;
  productName = "Fake Opta DFU";
  opened = false;
  configuration: UsbDeviceLike["configuration"] = null;
  configurations: UsbDeviceLike["configurations"];
  /** Flash contents by address (only sectors of the layout exist). */
  readonly flash = new Map<number, number>();
  readonly log: string[] = [];
  state: number;
  status = ERR.OK;
  pointer = 0;
  gone = false;
  leftTo: number | null = null;
  private pending: (() => number) | null = null;
  private busyLeft = 0;
  private readonly sectors: { start: number; size: number; type: string }[] = [];
  readonly transferSize: number;

  readonly opts: FakeOptions;

  constructor(opts: FakeOptions = {}) {
    this.opts = opts;
    this.vendorId = opts.vendorId ?? 0x2341;
    this.productId = opts.productId ?? 0x0364;
    this.transferSize = opts.transferSize ?? 2048;
    this.state = opts.initialState ?? S.dfuIDLE;
    const layout = opts.layout ?? OPTA_LAYOUT;
    // Only the simple one-region form is needed here.
    const [, , base, groups] = /^@([^/]*)\/(0x[0-9a-fA-F]+)\/(.*)$/.exec(layout) ?? [, , "0", ""];
    let addr = parseInt(base, 16);
    for (const g of groups ? groups.split(",") : []) {
      const [, n, size, unit, type] = /(\d+)\*(\d+)([ KM]?)([a-g])/.exec(g)!;
      const bytes = parseInt(size, 10) * (unit === "K" ? 1024 : unit === "M" ? 1 << 20 : 1);
      for (let i = 0; i < parseInt(n, 10); i++, addr += bytes) this.sectors.push({ start: addr, size: bytes, type });
    }
    this.configurations = [
      {
        configurationValue: 1,
        interfaces: [
          {
            interfaceNumber: 0,
            alternates: [
              { alternateSetting: 0, interfaceClass: 0xfe, interfaceSubclass: 1, interfaceProtocol: 2, interfaceName: opts.interfaceName === false ? null : layout },
              { alternateSetting: 1, interfaceClass: 0xfe, interfaceSubclass: 1, interfaceProtocol: 2, interfaceName: "@Ext RAW  Flash 16MB   /0x90000000/4096*4Kg" },
            ],
          },
        ],
      },
    ];
    this.strings = [new Uint8Array([4, 3, 0x09, 0x04]), str("Arduino"), str("Opta"), str("serial"), str(layout)];
  }

  private readonly strings: Uint8Array[];

  /** Pretend the bootloader lives in the first 256 KiB. */
  seedBootloader(byte = 0xb0): void {
    for (let a = 0x08000000; a < 0x08040000; a += 4096) this.flash.set(a, byte);
  }

  /** Flash byte (0xFF where never programmed). */
  read(addr: number): number {
    return this.flash.get(addr) ?? 0xff;
  }

  private sectorAt(addr: number) {
    return this.sectors.find((s) => addr >= s.start && addr < s.start + s.size);
  }

  async open() {
    if (this.gone) throw new DOMException("device gone", "NotFoundError");
    this.opened = true;
  }
  async close() {
    this.opened = false;
  }
  async selectConfiguration(v: number) {
    this.configuration = this.configurations.find((c) => c.configurationValue === v) ?? null;
  }
  async claimInterface() {}
  async releaseInterface() {}
  async selectAlternateInterface(_n: number, alt: number) {
    this.log.push(`alt ${alt}`);
  }

  private configDescriptor(): Uint8Array {
    const intf = (alt: number, iInterface: number) => [9, 4, 0, alt, 0, 0xfe, 1, 2, iInterface];
    const func = [9, 0x21, 0x0b, 0xff, 0x00, this.transferSize & 0xff, this.transferSize >> 8, 0x1a, 0x01];
    const body = [...intf(0, 4), ...intf(1, 0), ...func];
    const total = 9 + body.length;
    return new Uint8Array([9, 2, total & 0xff, total >> 8, 1, 1, 0, 0x80, 50, ...body]);
  }

  private fail(status: number): number {
    this.status = status;
    this.state = S.dfuERROR;
    return 0;
  }

  async controlTransferIn(setup: UsbSetup, length: number): Promise<UsbInResult> {
    if (this.gone) throw new DOMException("device gone", "NetworkError");
    const reply = (b: Uint8Array): UsbInResult => ({ status: "ok", data: new DataView(b.slice(0, length).buffer) });
    if (setup.requestType === "standard" && setup.request === 6) {
      const type = setup.value >> 8;
      const index = setup.value & 0xff;
      if (type === 2) return reply(this.configDescriptor());
      if (type === 3 && this.strings[index]) return reply(this.strings[index]);
      return { status: "stall" };
    }
    if (setup.requestType !== "class" || setup.index !== 0) return { status: "stall" };
    switch (setup.request) {
      case 3: {
        // GETSTATUS: runs a pending operation (the "sync" → busy → idle dance).
        if (this.state === S.dfuDNLOAD_SYNC && this.pending) {
          if (this.busyLeft > 0) {
            this.busyLeft--;
            return reply(new Uint8Array([0, this.opts.pollTimeoutMs ?? 5, 0, 0, S.dfuDNBUSY, 0]));
          }
          const op = this.pending;
          this.pending = null;
          this.state = S.dfuDNLOAD_IDLE;
          op();
        } else if (this.state === S.dfuMANIFEST_SYNC) {
          // The device leaves DFU mode and drops off the bus.
          this.leftTo = this.pointer;
          this.log.push(`leave ${hex(this.pointer)}`);
          this.gone = true;
          return reply(new Uint8Array([0, 0, 0, 0, S.dfuMANIFEST, 0]));
        }
        return reply(new Uint8Array([this.status, 0, 0, 0, this.state, 0]));
      }
      case 5:
        return reply(new Uint8Array([this.state]));
      case 2: {
        // UPLOAD (DfuSe: block ≥ 2 reads at pointer + (block-2)*length)
        if (this.state !== S.dfuIDLE && this.state !== S.dfuUPLOAD_IDLE) {
          this.fail(ERR.errSTALLEDPKT);
          return { status: "stall" };
        }
        this.state = S.dfuUPLOAD_IDLE;
        const base = this.pointer + (setup.value - 2) * length;
        const out = new Uint8Array(length);
        for (let i = 0; i < length; i++) out[i] = this.read(base + i);
        return reply(out);
      }
    }
    return { status: "stall" };
  }

  async controlTransferOut(setup: UsbSetup, data?: BufferSource): Promise<UsbOutResult> {
    if (this.gone) throw new DOMException("device gone", "NetworkError");
    if (setup.requestType !== "class" || setup.index !== 0) return { status: "stall", bytesWritten: 0 };
    const bytes = data ? new Uint8Array(data instanceof ArrayBuffer ? data : (data as ArrayBufferView).buffer, (data as ArrayBufferView).byteOffset ?? 0, (data as ArrayBufferView).byteLength ?? (data as ArrayBuffer).byteLength) : new Uint8Array(0);
    switch (setup.request) {
      case 4: // CLRSTATUS
        this.log.push("clrstatus");
        this.status = ERR.OK;
        this.state = S.dfuIDLE;
        return { status: "ok", bytesWritten: 0 };
      case 6: // ABORT
        this.state = S.dfuIDLE;
        return { status: "ok", bytesWritten: 0 };
      case 1: {
        // DNLOAD
        if (this.state !== S.dfuIDLE && this.state !== S.dfuDNLOAD_IDLE) {
          this.fail(ERR.errSTALLEDPKT);
          return { status: "stall", bytesWritten: 0 };
        }
        const block = setup.value;
        if (bytes.length === 0) {
          this.state = S.dfuMANIFEST_SYNC;
          return { status: "ok", bytesWritten: 0 };
        }
        this.busyLeft = this.opts.busyPolls ?? 1;
        this.state = S.dfuDNLOAD_SYNC;
        if (block === 0) {
          const cmd = bytes[0];
          const addr = bytes.length >= 5 ? new DataView(bytes.buffer, bytes.byteOffset).getUint32(1, true) : null;
          this.pending = () => {
            if (cmd === 0x21 && addr !== null) {
              this.pointer = addr;
              return 0;
            }
            if (cmd === 0x41 && addr === null) {
              this.log.push("MASS ERASE");
              return this.fail(ERR.errTARGET);
            }
            if (cmd === 0x41 && addr !== null) {
              const s = this.sectorAt(addr);
              if (!s || s.type === "a" || addr === this.opts.failEraseAt) return this.fail(s ? ERR.errERASE : ERR.errADDRESS);
              this.log.push(`erase ${hex(s.start)}`);
              if (!this.opts.brokenErase) for (let a = s.start; a < s.start + s.size; a++) this.flash.delete(a);
              return 0;
            }
            return this.fail(ERR.errSTALLEDPKT);
          };
        } else if (block >= 2) {
          const at = this.pointer + (block - 2) * this.transferSize;
          this.pending = () => {
            if (bytes.length > this.transferSize) return this.fail(ERR.errADDRESS);
            for (let i = 0; i < bytes.length; i++) {
              const s = this.sectorAt(at + i);
              if (!s || s.type === "a") return this.fail(ERR.errTARGET);
              const v = this.read(at + i) & bytes[i]; // flash programs 1 → 0 only
              if (v !== bytes[i]) return this.fail(ERR.errVERIFY);
              this.flash.set(at + i, v);
            }
            this.log.push(`write ${hex(at)} ${bytes.length}`);
            return 0;
          };
        } else {
          return { status: "stall", bytesWritten: 0 };
        }
        return { status: "ok", bytesWritten: bytes.length };
      }
    }
    return { status: "stall", bytesWritten: 0 };
  }
}

function hex(n: number) {
  return `0x${n.toString(16).padStart(8, "0")}`;
}
