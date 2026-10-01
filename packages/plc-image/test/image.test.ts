// SPDX-License-Identifier: MPL-2.0
//
// The WebAssembly image linker against the native one (`plcc image`, the
// committed chase.img), its errors, and the TypeScript header checks.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { type DeviceLike, ImageError, buildDeviceImage, checkImage, crc32, load, parseImageHeader, programLayout } from "../src/index";

const here = dirname(fileURLToPath(import.meta.url));
const repo = (p: string) => join(here, "../../..", p);
const fixture = (n: string) => new Uint8Array(readFileSync(join(here, "fixtures", n)));
const crateData = (n: string) => new Uint8Array(readFileSync(repo(`crates/plcc-image/tests/data/${n}`)));
// The catalog's Opta manifest, expanded by plcc-device (its golden file).
const opta = (): DeviceLike => JSON.parse(readFileSync(repo("crates/plcc-device/tests/data/arduino-opta.expanded.json"), "utf8"));

describe("buildDeviceImage", () => {
  it("links a plcc object exactly as the native plcc image does", async () => {
    await load();
    const img = await buildDeviceImage(opta(), fixture("chase.o"));
    expect(img.bytes).toEqual(fixture("chase.img"));
    expect(img.address).toBe(0x08180000);
    expect(img.size).toBe(img.bytes.length);
    expect(img.size % 32).toBe(0);
    expect(img.header).toMatchObject({
      targetId: "arduino-opta",
      targetVersion: 2,
      abi: 1,
      slotAddr: 0x08180000,
      slotSize: 0x80000,
      ramAddr: 0x20010000,
      ramSize: 0x10000,
      textAddr: 0x08180080,
      servicesSlot: 0x20010000,
    });
    expect(img.imports.map((i) => i.name)).toEqual(["plcc_monotonic_ns"]);
    expect(img.sections.some((s) => s.kind === "text" && s.name === ".text")).toBe(true);
    expect(img.buildId).toBe(img.header.buildId);
    expect(img.buildId).toMatch(/^[0-9a-f]{32}$/);
  });

  it("puts a given build id in the header, and only changes the header", async () => {
    const a = await buildDeviceImage(opta(), fixture("chase.o"));
    const b = await buildDeviceImage(opta(), fixture("chase.o"), { buildId: "00112233445566778899aabbccddeeff" });
    expect(b.buildId).toBe("00112233445566778899aabbccddeeff");
    expect(b.bytes.subarray(128)).toEqual(a.bytes.subarray(128));
    const c = await buildDeviceImage(opta(), fixture("chase.o"), { buildId: new Uint8Array(16).fill(0xab) });
    expect(c.buildId).toBe("ab".repeat(16));
    await expect(buildDeviceImage(opta(), fixture("chase.o"), { buildId: "xyz" })).rejects.toThrow(/32 hex digits/);
  });

  it("links every relocation type (the crate's assembly test)", async () => {
    const img = await buildDeviceImage(opta(), crateData("relocs.o"));
    const lldText = crateData("relocs.lld.text.bin");
    expect(img.bytes.subarray(128, 128 + lldText.length)).toEqual(lldText);
    expect(img.imports.map((i) => i.name)).toEqual(["plcc_fault", "memcpy", "memset"]);
  });

  it("reports link errors as ImageError with the linker's message", async () => {
    await expect(buildDeviceImage(opta(), crateData("errors-1.o"))).rejects.toThrow(ImageError);
    await expect(buildDeviceImage(opta(), crateData("errors-1.o"))).rejects.toThrow(/`not_a_service`, `printf`: not defined by the program and not a runtime service/);
    await expect(buildDeviceImage(opta(), crateData("errors-9.o"))).rejects.toThrow(/does not define plcc_get_app/);
    await expect(buildDeviceImage(opta(), new TextEncoder().encode("hello"))).rejects.toThrow(/not an ELF object/);
    // The instance survives errors and keeps linking.
    expect((await buildDeviceImage(opta(), fixture("chase.o"))).bytes).toEqual(fixture("chase.img"));
  });

  it("refuses a device without a program slot", async () => {
    const sim = JSON.parse(readFileSync(repo("crates/plcc-device/tests/data/simulator.expanded.json"), "utf8"));
    await expect(buildDeviceImage(sim, fixture("chase.o"))).rejects.toThrow(/has no program slot/);
    const odd = opta();
    odd.flash!.program!.format = 2;
    expect(() => programLayout(odd)).toThrow(/format 2/);
  });

  it("refuses a program that needs services the runtime lacks", async () => {
    const old = opta();
    old.flash!.program!.services = 4;
    await expect(buildDeviceImage(old, crateData("relocs.o"))).rejects.toThrow(/`memset` \(runtime service 5\)/);
  });

  it("links for another layout when the manifest says so", async () => {
    const other = opta();
    other.flash!.program!.address = 0x08100000;
    other.flash!.program!.ram = { start: 0x30000000, size: 0x8000 };
    const img = await buildDeviceImage(other, fixture("chase.o"));
    expect(img.header.slotAddr).toBe(0x08100000);
    expect(img.header.bssAddr).toBe(0x30000008);
    expect(() => checkImage(img.bytes, programLayout(opta()))).toThrow(/another program slot/);
  });
});

describe("header checks", () => {
  const layout = () => programLayout(opta());

  it("crc32 is zlib's", () => {
    expect(crc32(new TextEncoder().encode("123456789"))).toBe(0xcbf43926);
    expect(crc32(new Uint8Array())).toBe(0);
  });

  it("accepts the native image and refuses each kind of damage", () => {
    const good = fixture("chase.img");
    expect(checkImage(good, layout(), 2).targetId).toBe("arduino-opta");
    const reseal = (b: Uint8Array) => new DataView(b.buffer).setUint32(124, crc32(b.subarray(0, 124)), true);
    const damaged = (f: (b: Uint8Array, v: DataView) => void, sealed = true) => {
      const b = good.slice();
      f(b, new DataView(b.buffer));
      if (sealed) reseal(b);
      return () => checkImage(b, layout(), 2);
    };
    expect(damaged((b) => b.fill(0xff), false)).toThrow("empty slot");
    expect(damaged((b) => (b[0] = 0), false)).toThrow("bad magic");
    expect(damaged((b) => (b[9] ^= 1), false)).toThrow("header CRC mismatch");
    expect(damaged((_, v) => v.setUint32(40, 3, true))).toThrow("another version");
    expect(damaged((_, v) => v.setUint32(44, 2, true))).toThrow("ABI mismatch");
    expect(damaged((_, v) => v.setUint32(48, 999, true))).toThrow("needs services");
    expect(damaged((_, v) => v.setUint32(60, 0x24000000, true))).toThrow("another RAM window");
    expect(damaged((_, v) => v.setUint32(100, 0x08180080, true))).toThrow("not Thumb code");
    expect(damaged((b) => (b[b.length - 1] ^= 1), false)).toThrow("body CRC mismatch");
    expect(() => checkImage(good.subarray(0, 64), layout())).toThrow(/too short/);
    expect(() => checkImage(good.subarray(0, good.length - 32), layout())).toThrow(/only/);
  });

  it("parses the header fields", () => {
    const h = parseImageHeader(fixture("chase.img"));
    expect(h.magic).toBe(0x49434c50);
    expect(h.format).toBe(1);
    expect(h.headerSize).toBe(128);
    expect(h.imageSize).toBe(fixture("chase.img").length);
    expect(h.getApp & 1).toBe(1);
  });
});
