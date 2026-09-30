// SPDX-License-Identifier: MPL-2.0
//
// Device manifests are untrusted data: their [flash] section may narrow the
// built-in limits for a bootloader, never widen them.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { DfuseDevice, ManifestError, type ManifestFlash, OPTA, SafetyError, profileFromManifest } from "../src/index";
import { FakeDfuDevice } from "./fake-device";

const here = dirname(fileURLToPath(import.meta.url));
// The catalog's Opta manifest, expanded by plcc-device (the golden file its tests keep current).
const golden = JSON.parse(readFileSync(join(here, "../../../crates/plcc-device/tests/data/arduino-opta.expanded.json"), "utf8"));
const optaFlash = (): ManifestFlash => structuredClone(golden.flash);
const noSleep = async () => {};

function reject(patch: Partial<ManifestFlash>, message: RegExp) {
  expect(() => profileFromManifest({ ...optaFlash(), ...patch }, "Evil")).toThrow(ManifestError);
  expect(() => profileFromManifest({ ...optaFlash(), ...patch }, "Evil")).toThrow(message);
}

describe("profiles from manifests", () => {
  it("turns the catalog's Opta manifest into the built-in Opta profile", () => {
    const p = profileFromManifest(optaFlash(), "Arduino Opta");
    const key = (f: object) => JSON.stringify(f);
    const sorted = <T extends object>(a: T[]) => [...a].sort((x, y) => key(x).localeCompare(key(y)));
    expect({ ...p, dfuFilters: sorted(p.dfuFilters), runtimeFilters: sorted(p.runtimeFilters) }).toEqual({
      ...OPTA,
      dfuFilters: sorted(OPTA.dfuFilters),
      runtimeFilters: sorted(OPTA.runtimeFilters),
    });
  });

  it("accepts a narrower area and keeps everything below it protected", async () => {
    const p = profileFromManifest({ ...optaFlash(), address: 0x08080000, max_size: 0x80000 });
    expect(p).toMatchObject({ address: 0x08080000, maxSize: 0x80000, minAddress: 0x08080000 });
    const fake = new FakeDfuDevice();
    fake.seedBootloader();
    const dev = await DfuseDevice.open(fake, { profile: p, sleep: noSleep });
    expect(() => dev.plan(0x08040000, 16)).toThrow(SafetyError);
    await dev.flash(Uint8Array.from({ length: 64 }, (_, i) => i));
    expect(fake.log.filter((l) => l.startsWith("erase"))).toEqual(["erase 0x08080000"]);
  });

  it("rejects a manifest that moves the application over the bootloader", () => {
    reject({ address: 0x08000000 }, /address 0x08000000 is below 0x08040000/);
    reject({ address: 0x08020000, max_size: 0x20000 }, /is below 0x08040000/);
    // Even with its own protected list emptied.
    reject({ address: 0x08000000, protected: [] }, /is below 0x08040000/);
  });

  it("rejects an oversized image area", () => {
    reject({ max_size: 0x200000 }, /ends at 0x08240000, past 0x08200000/);
    reject({ address: 0x081f0000, max_size: 0x20000 }, /past 0x08200000/);
    reject({ max_size: 0 }, /positive integers/);
  });

  it("rejects another alternate setting or memory", () => {
    reject({ alt: 1 }, /alternate setting 1 is not allowed/);
    reject({ layout: "Ext RAW  Flash" }, /layout "Ext RAW  Flash" is not "Internal Flash"/);
  });

  it("refuses USB ids it has no built-in limits for", () => {
    reject({ usb: [{ vid: 0x0483, pid: 0xdf11 }] }, /no built-in flash limits for USB 0483:df11/);
    // One unknown id among known ones is enough to refuse.
    reject({ usb: [{ vid: 0x2341, pid: 0x0364 }, { vid: 0x1234, pid: 0x5678 }] }, /1234:5678/);
    reject({ usb: [] }, /no bootloader USB ids/);
  });

  it("rejects touching serial ports that are not the board's", () => {
    reject({ runtime_usb: [{ vid: 0x0403, pid: 0x6001 }] }, /runtime USB id 0403:6001/);
    reject({ runtime_usb: [] }, /1200-baud-touch needs runtime_usb/);
  });

  it("rejects other methods and self-contradicting protection", () => {
    reject({ method: "mass-erase" }, /method "mass-erase" is not supported/);
    reject({ address: 0x08040000, protected: [{ start: 0x08100000, size: 0x1000 }] }, /overlaps its own protected region/);
  });
});
