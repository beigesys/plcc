// SPDX-License-Identifier: MPL-2.0
//
// The program image header (docs/program-image.md) and the loader's checks,
// in TypeScript: the same sequence as runtimes/arduino-opta/loader/plcc_image.c
// and crates/plcc-image/src/header.rs, so studio can tell before a download
// (or after reading a slot back) whether the runtime would accept an image.

export const IMAGE_MAGIC = 0x49434c50; // "PLCI"
export const IMAGE_FORMAT = 1;
export const HEADER_SIZE = 128;
export const RAM_RESERVED = 8;

/** Where a device's runtime expects programs (the manifest's [flash.program] and identity). */
export interface ProgramLayout {
  targetId: string;
  targetVersion: number;
  abi: number;
  slotAddr: number;
  slotSize: number;
  ramAddr: number;
  ramSize: number;
  /** Entries in the runtime's service table. */
  services: number;
  hardFloat: boolean;
}

export interface ImageHeader {
  magic: number;
  format: number;
  headerSize: number;
  imageSize: number;
  bodyCrc32: number;
  targetId: string;
  targetVersion: number;
  abi: number;
  services: number;
  slotAddr: number;
  slotSize: number;
  ramAddr: number;
  ramSize: number;
  textAddr: number;
  textSize: number;
  dataLoad: number;
  dataAddr: number;
  dataSize: number;
  bssAddr: number;
  bssSize: number;
  servicesSlot: number;
  getApp: number;
  flags: number;
  /** 32 hex digits. */
  buildId: string;
  headerCrc32: number;
}

/** An image the runtime would refuse, or a link that failed. `message` says why. */
export class ImageError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ImageError";
  }
}

let table: Uint32Array | undefined;

/** CRC-32/ISO-HDLC (zlib's crc32). */
export function crc32(bytes: Uint8Array): number {
  if (!table) {
    table = new Uint32Array(256);
    for (let i = 0; i < 256; i++) {
      let c = i;
      for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
      table[i] = c >>> 0;
    }
  }
  let crc = 0xffffffff;
  for (let i = 0; i < bytes.length; i++) crc = table[(crc ^ bytes[i]) & 0xff] ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

const hex = (b: Uint8Array) => Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");

/** Decode the first 128 bytes (no checks beyond the length). */
export function parseImageHeader(bytes: Uint8Array): ImageHeader {
  if (bytes.length < HEADER_SIZE) throw new ImageError(`${bytes.length} bytes is too short for an image header (128)`);
  const v = new DataView(bytes.buffer, bytes.byteOffset, HEADER_SIZE);
  const u = (o: number) => v.getUint32(o, true);
  const id = bytes.subarray(16, 40);
  const end = id.indexOf(0);
  return {
    magic: u(0),
    format: v.getUint16(4, true),
    headerSize: v.getUint16(6, true),
    imageSize: u(8),
    bodyCrc32: u(12),
    targetId: new TextDecoder().decode(id.subarray(0, end < 0 ? 24 : end)),
    targetVersion: u(40),
    abi: u(44),
    services: u(48),
    slotAddr: u(52),
    slotSize: u(56),
    ramAddr: u(60),
    ramSize: u(64),
    textAddr: u(68),
    textSize: u(72),
    dataLoad: u(76),
    dataAddr: u(80),
    dataSize: u(84),
    bssAddr: u(88),
    bssSize: u(92),
    servicesSlot: u(96),
    getApp: u(100),
    flags: u(104),
    buildId: hex(bytes.subarray(108, 124)),
    headerCrc32: u(124),
  };
}

const within = (start: number, len: number, lo: number, hi: number) => start >= lo && start + len <= hi;

/**
 * The loader's checks on `bytes` (the slot from its first byte): throws
 * ImageError with the loader's reason, or returns the header. The runtime
 * accepts manifest versions `minVersion..layout.targetVersion`.
 */
export function checkImage(bytes: Uint8Array, layout: ProgramLayout, minVersion = 1): ImageHeader {
  const fail = (m: string): never => {
    throw new ImageError(m);
  };
  const h = parseImageHeader(bytes);
  if (h.magic === 0xffffffff) fail("empty slot");
  if (h.magic !== IMAGE_MAGIC) fail("no program image (bad magic)");
  if (h.format !== IMAGE_FORMAT) fail("unknown image format");
  if (h.headerSize !== HEADER_SIZE) fail("unknown header size");
  if (crc32(bytes.subarray(0, 124)) !== h.headerCrc32) fail("header CRC mismatch");
  if (bytes.subarray(16, 40).indexOf(0) < 0) fail("target id not terminated");
  if (h.targetId !== layout.targetId) fail("linked for another device");
  if (h.targetVersion < minVersion || h.targetVersion > layout.targetVersion) fail("linked for another version of this device's manifest");
  if (h.abi !== layout.abi) fail("runtime ABI mismatch");
  if (h.services > layout.services) fail("needs services this runtime does not have");
  if (h.slotAddr !== layout.slotAddr || h.slotSize !== layout.slotSize) fail("linked for another program slot");
  if (h.ramAddr !== layout.ramAddr || h.ramSize !== layout.ramSize) fail("linked for another RAM window");
  if (h.flags !== 0) fail("unknown flags");
  const slot = layout.slotAddr;
  const slotEnd = slot + h.imageSize;
  if (h.imageSize < HEADER_SIZE || h.imageSize > layout.slotSize) fail("bad image size");
  if (h.textAddr !== slot + HEADER_SIZE) fail("text does not follow the header");
  if (!within(h.textAddr, h.textSize, slot + HEADER_SIZE, slotEnd)) fail("text outside the image");
  if (!within(h.dataLoad, h.dataSize, h.textAddr + h.textSize, slotEnd)) fail(".data image outside the image");
  const ram = layout.ramAddr;
  const ramEnd = ram + layout.ramSize;
  if (h.servicesSlot !== ram) fail("services slot is not the window's first word");
  if (!within(h.dataAddr, h.dataSize, ram + RAM_RESERVED, ramEnd)) fail(".data outside the RAM window");
  if (!within(h.bssAddr, h.bssSize, h.dataAddr + h.dataSize, ramEnd)) fail(".bss outside the RAM window");
  if ((h.getApp & 1) === 0 || !within(h.getApp - 1, 2, h.textAddr, h.textAddr + h.textSize)) fail("plcc_get_app is not Thumb code inside the image");
  if (bytes.length < h.imageSize) fail(`image_size ${h.imageSize} but only ${bytes.length} bytes`);
  if (crc32(bytes.subarray(HEADER_SIZE, h.imageSize)) !== h.bodyCrc32) fail("body CRC mismatch");
  return h;
}
