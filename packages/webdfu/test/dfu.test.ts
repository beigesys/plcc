// SPDX-License-Identifier: MPL-2.0
import { describe, expect, it } from "vitest";
import { DfuError, DfuseDevice, type DeviceProfile, LayoutError, OPTA, type Progress, SafetyError, parseLayout, touch1200, waitForDfuDevice } from "../src/index";
import { FLOORS } from "../src/floors";
import { FakeDfuDevice } from "./fake-device";

const noSleep = async () => {};
const image = (n: number, seed = 1) => Uint8Array.from({ length: n }, (_, i) => (i * 31 + seed) & 0xff);

async function open(fake = new FakeDfuDevice(), profile: DeviceProfile = OPTA) {
  fake.seedBootloader();
  return { fake, dev: await DfuseDevice.open(fake, { profile, sleep: noSleep }) };
}

/** Every bootloader byte the fake seeded is intact and was never erased. */
function bootloaderIntact(fake: FakeDfuDevice) {
  for (let a = 0x08000000; a < 0x08040000; a += 4096) expect(fake.read(a)).toBe(0xb0);
  expect(fake.log.filter((l) => /^(erase|write) 0x080[0-3]/.test(l) || l === "MASS ERASE")).toEqual([]);
}

describe("layout strings", () => {
  it("parses the Opta bootloader's alt 0", () => {
    const l = parseLayout("@Internal Flash  2MB   /0x08000000/01*128Ka,15*128Kg");
    expect(l.name).toBe("Internal Flash  2MB");
    expect(l.sectors).toHaveLength(16);
    expect(l.sectors[0]).toEqual({ start: 0x08000000, size: 131072, readable: true, erasable: false, writable: false, type: "a" });
    expect(l.sectors[1]).toMatchObject({ start: 0x08020000, erasable: true, writable: true, type: "g" });
    expect(l.sectors[15].start + l.sectors[15].size).toBe(0x08200000);
  });

  it("parses several regions and units", () => {
    const l = parseLayout("@Option Bytes  /0x5200201C/01*128 e/0x52002020/2*1Ka");
    expect(l.sectors.map((s) => [s.start, s.size, s.type])).toEqual([
      [0x5200201c, 128, "e"],
      [0x52002020, 1024, "a"],
      [0x52002420, 1024, "a"],
    ]);
  });

  it("rejects malformed layouts", () => {
    for (const bad of ["Internal Flash", "@x/0x08000000", "@x/zz/1*1Kg", "@x/0x0/1*1Kz"]) expect(() => parseLayout(bad)).toThrow(LayoutError);
  });
});

describe("flashing", () => {
  it("erases the touched sectors, writes, and leaves at 0x08040000", async () => {
    const { fake, dev } = await open();
    expect(dev.transferSize).toBe(2048);
    const img = image(300_000);
    const progress: Progress[] = [];
    await dev.flash(img, { onProgress: (p) => progress.push(p) });

    // 300 000 bytes from 0x08040000 span three 128 KiB sectors.
    expect(fake.log.filter((l) => l.startsWith("erase"))).toEqual(["erase 0x08040000", "erase 0x08060000", "erase 0x08080000"]);
    expect(fake.log.filter((l) => l.startsWith("write"))).toHaveLength(Math.ceil(300_000 / 2048));
    for (const i of [0, 1, 2047, 2048, 150_001, 299_999]) expect(fake.read(0x08040000 + i)).toBe(img[i]);
    expect(fake.read(0x08040000 + 300_000)).toBe(0xff);
    expect(fake.leftTo).toBe(0x08040000);
    bootloaderIntact(fake);

    const phases = progress.map((p) => p.phase);
    expect(phases[0]).toBe("erase");
    expect(phases.at(-1)).toBe("manifest");
    const writes = progress.filter((p) => p.phase === "write").map((p) => p.done);
    expect(writes).toEqual([...writes].sort((a, b) => a - b));
    expect(writes.at(-1)).toBe(300_000);
  });

  it("reads the layout from string descriptors when WebUSB has no interface name", async () => {
    const { dev } = await open(new FakeDfuDevice({ interfaceName: false, transferSize: 1024 }));
    expect(dev.layout.sectors).toHaveLength(16);
    expect(dev.transferSize).toBe(1024);
    expect(dev.functional?.dfuVersion).toBe(0x011a);
  });

  it("verifies by reading back", async () => {
    const { fake, dev } = await open();
    await dev.flash(image(5000), { verify: true, leave: false });
    expect(fake.leftTo).toBeNull();
  });

  it("waits out bwPollTimeout while the device is busy", async () => {
    const slept: number[] = [];
    const fake = new FakeDfuDevice({ busyPolls: 3, pollTimeoutMs: 40 });
    const dev = await DfuseDevice.open(fake, { profile: OPTA, sleep: async (ms) => void slept.push(ms) });
    await dev.flash(image(10));
    expect(slept.length).toBeGreaterThan(0);
    expect(slept.every((ms) => ms === 40)).toBe(true);
  });
});

describe("bootloader protection", () => {
  it("refuses any address below 0x08040000 before sending anything", async () => {
    for (const address of [0x08000000, 0x08020000, 0x0803ffff, 0x08030000]) {
      const { fake, dev } = await open();
      const before = fake.log.length;
      await expect(dev.flash(image(16), { address })).rejects.toThrow(SafetyError);
      expect(fake.log.length).toBe(before);
      bootloaderIntact(fake);
    }
    const { dev } = await open();
    expect(() => dev.plan(0x0803fff0, 0x100)).toThrow(/below 0x08040000/);
  });

  it("refuses a read-only ('a') sector even without a minimum address", async () => {
    // A test-only bootloader id whose built-in floor allows the whole flash, so
    // only the DfuSe layout's read-only marking stands between it and sector 0.
    FLOORS["0483:df11"] = { name: "test", minAddress: 0x08000000, end: 0x08200000, alts: [0], layoutName: "Internal Flash", runtime: [] };
    const lenient: DeviceProfile = {
      ...OPTA, name: "generic", dfuFilters: [{ vendorId: 0x0483, productId: 0xdf11 }], minAddress: 0, address: 0x08000000, maxSize: 2 << 20,
    };
    const { fake, dev } = await open(new FakeDfuDevice({ vendorId: 0x0483, productId: 0xdf11 }), lenient).finally(() => delete FLOORS["0483:df11"]);
    expect(() => dev.plan(0x08000000, 4)).toThrow(/type 'a'/);
    expect(() => dev.plan(0x0801fff0, 0x20)).toThrow(/type 'a'/);
    await expect(dev.flash(image(4), { address: 0x0801fffc })).rejects.toThrow(SafetyError);
    bootloaderIntact(fake);
    // The next sector is writable under this profile.
    expect(dev.plan(0x08020000, 4).sectors.map((s) => s.start)).toEqual([0x08020000]);
  });

  it("keeps the Opta floor whatever a hand-made profile says", async () => {
    const wide: DeviceProfile = { ...OPTA, minAddress: 0, address: 0x08000000, maxSize: 2 << 20 };
    const { fake, dev } = await open(new FakeDfuDevice(), wide);
    expect(() => dev.plan(0x08020000, 4)).toThrow(/never writes below 0x08040000 on Arduino Opta/);
    await expect(dev.flash(image(16), { address: 0x08000000 })).rejects.toThrow(SafetyError);
    await expect(dev.leave(0x08020000)).rejects.toThrow(SafetyError);
    bootloaderIntact(fake);
  });

  it("refuses a bootloader it has no built-in limits for", async () => {
    const other: DeviceProfile = { ...OPTA, dfuFilters: [{ vendorId: 0x0483, productId: 0xdf11 }] };
    await expect(DfuseDevice.open(new FakeDfuDevice({ vendorId: 0x0483, productId: 0xdf11 }), { profile: other })).rejects.toThrow(
      /no built-in flash limits for USB 0x0483:0xdf11/,
    );
    await expect(DfuseDevice.open(new FakeDfuDevice(), { profile: { ...OPTA, alt: 1 } })).rejects.toThrow(/alternate setting 1 is not one/);
  });

  it("refuses writes past the flash or the profile's size", async () => {
    const { dev } = await open();
    expect(() => dev.plan(0x081ffff0, 0x20)).toThrow(SafetyError);
    expect(() => dev.plan(0x08040000, OPTA.maxSize + 1)).toThrow(/at most|past 0x08200000/);
    expect(() => dev.plan(0x90000000, 4)).toThrow(SafetyError);
    expect(() => dev.plan(0x08040000, 0)).toThrow(SafetyError);
    // The 10 sectors between the bootloader and the program slot: 0x08040000-0x0817FFFF.
    expect(OPTA.maxSize).toBe(10 * 128 * 1024);
    expect(dev.plan(0x08040000, OPTA.maxSize).sectors).toHaveLength(10);
    expect(() => dev.plan(0x08180000, 4)).toThrow(/past Arduino Opta's application area/);
  });

  it("refuses a device or alternate the profile does not describe", async () => {
    await expect(DfuseDevice.open(new FakeDfuDevice({ productId: 0x0164 }), { profile: OPTA })).rejects.toThrow(/not Arduino Opta in DFU mode/);
    await expect(
      DfuseDevice.open(new FakeDfuDevice({ layout: "@Ext RAW  Flash 16MB   /0x90000000/4096*4Kg" }), { profile: OPTA }),
    ).rejects.toThrow(/not "Internal Flash"/);
    await expect(DfuseDevice.open(new FakeDfuDevice({ layout: "garbage" }), { profile: OPTA })).rejects.toThrow(LayoutError);
  });

  it("never mass-erases and refuses to leave into the bootloader", async () => {
    const { fake, dev } = await open();
    await expect(dev.leave(0x08000000)).rejects.toThrow(SafetyError);
    await dev.flash(image(100));
    expect(fake.log).not.toContain("MASS ERASE");
  });
});

describe("device errors", () => {
  it("reports errERASE clearly and clears the error state", async () => {
    const { fake, dev } = await open(new FakeDfuDevice({ failEraseAt: 0x08060000 }));
    const err = await dev.flash(image(200_000)).catch((e) => e);
    expect(err).toBeInstanceOf(DfuError);
    expect(err.message).toMatch(/erase 0x08060000: the device reported errERASE \(memory erase function failed\)/);
    expect(err.status).toBe(4);
    expect(fake.log).toContain("clrstatus");
    expect(fake.state).toBe(2); // dfuIDLE again
  });

  it("reports a write that does not program (errVERIFY)", async () => {
    const fake = new FakeDfuDevice({ brokenErase: true });
    fake.flash.set(0x08040000 + 3000, 0x00); // a stale 0 the erase will not clear
    const { dev } = await open(fake);
    const err = await dev.flash(image(4000, 7), { leave: false }).catch((e) => e);
    expect(err).toBeInstanceOf(DfuError);
    expect(err.message).toMatch(/write 0x08040800: the device reported errVERIFY/);
    expect(fake.state).toBe(2);
  });

  it("recovers a device left in dfuERROR or dfuDNLOAD_IDLE", async () => {
    for (const initialState of [10, 5]) {
      const { fake, dev } = await open(new FakeDfuDevice({ initialState }));
      await dev.flash(image(10));
      expect(fake.leftTo).toBe(0x08040000);
    }
  });

  it("refuses a device not in its bootloader's idle state", async () => {
    const { dev } = await open(new FakeDfuDevice({ initialState: 0 /* appIDLE */ }));
    await expect(dev.flash(image(10))).rejects.toThrow(/is it in its bootloader/);
  });
});

describe("1200-baud touch", () => {
  it("opens at 1200 baud, drops DTR and closes", async () => {
    const calls: string[] = [];
    await touch1200(
      {
        open: async (o) => void calls.push(`open ${o.baudRate}`),
        setSignals: async (s) => void calls.push(`dtr ${s.dataTerminalReady}`),
        close: async () => void calls.push("close"),
      },
      noSleep,
    );
    expect(calls).toEqual(["open 1200", "dtr false", "close"]);
  });

  it("waits for the DFU device to appear", async () => {
    let polls = 0;
    const fake = new FakeDfuDevice();
    const usb = { getDevices: async () => (++polls < 3 ? [] : [fake]) };
    expect(await waitForDfuDevice(usb, OPTA, { sleep: noSleep })).toBe(fake);
    await expect(waitForDfuDevice({ getDevices: async () => [] }, OPTA, { sleep: noSleep, timeoutMs: 100, pollMs: 50 })).rejects.toThrow(/did not appear/);
  });
});
