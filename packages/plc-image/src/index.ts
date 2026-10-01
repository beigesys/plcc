// SPDX-License-Identifier: MPL-2.0
//
// Program images in the browser (docs/program-image.md): crates/plcc-image,
// compiled to WebAssembly with a plain C ABI, links the Cortex-M object plcc
// compiled for a device into an image for the device's program slot. The
// image goes to the device with @plcc/webdfu (programProfileFromManifest).
//
//   import { buildDeviceImage } from "@plcc/plc-image";
//   const img = await buildDeviceImage(device, objectBytes);   // device: an expanded manifest
//   await dfu.flash(img.bytes, { address: img.address });

import { ImageError, type ImageHeader, type ProgramLayout, checkImage } from "./header";

export { ImageError, IMAGE_FORMAT, IMAGE_MAGIC, HEADER_SIZE, checkImage, crc32, parseImageHeader, type ImageHeader, type ProgramLayout } from "./header";

/** The parts of an expanded device manifest (plcc_device::Device as JSON) this package reads. */
export interface DeviceLike {
  device: { id: string; version: number; name?: string };
  target: { runtime: { abi: number }; float_abi?: string };
  flash?: {
    program?: { format: number; address: number; max_size: number; ram: { start: number; size: number }; services: number };
  };
}

export interface DeviceImage {
  /** The whole image; write it at `address`. */
  bytes: Uint8Array;
  /** The program slot's flash address. */
  address: number;
  size: number;
  /** 32 hex digits, as in the header. */
  buildId: string;
  header: ImageHeader;
  /** Runtime services the program calls, with their veneer addresses. */
  imports: { index: number; name: string; veneer: number }[];
  /** Placed sections: kind is text, rodata, data or bss. */
  sections: { name: string; kind: string; addr: number; size: number }[];
  layout: ProgramLayout;
}

export interface BuildOptions {
  /** Header build id: 16 bytes or 32 hex digits (default: from the object's SHA-256). */
  buildId?: Uint8Array | string;
}

/** The program layout of a device, from its manifest. Throws ImageError if it has no program slot. */
export function programLayout(device: DeviceLike): ProgramLayout {
  const p = device.flash?.program;
  const name = device.device?.name ?? device.device?.id ?? "device";
  if (!p) throw new ImageError(`${name} has no program slot ([flash.program]): its runtime does not load program images`);
  if (p.format !== 1) throw new ImageError(`${name}: program image format ${p.format} is not one this package writes (1)`);
  return {
    targetId: device.device.id,
    targetVersion: device.device.version,
    abi: device.target.runtime.abi,
    slotAddr: p.address,
    slotSize: p.max_size,
    ramAddr: p.ram.start,
    ramSize: p.ram.size,
    services: p.services,
    hardFloat: device.target.float_abi === "hard",
  };
}

interface Exports {
  memory: WebAssembly.Memory;
  plcc_image_alloc(len: number): number;
  plcc_image_free(ptr: number, len: number): void;
  plcc_image_link(
    obj: number,
    objLen: number,
    id: number,
    idLen: number,
    targetVersion: number,
    abi: number,
    slotAddr: number,
    slotSize: number,
    ramAddr: number,
    ramSize: number,
    services: number,
    hardFloat: number,
    buildId: number,
  ): number;
  plcc_image_output_ptr(): number;
  plcc_image_output_len(): number;
  plcc_image_report_ptr(): number;
  plcc_image_report_len(): number;
}

let ready: Promise<Exports> | undefined;

type Source = BufferSource | WebAssembly.Module | URL | string | Response | Promise<Response>;

async function instantiate(source: Source | undefined): Promise<Exports> {
  let src: Source = source ?? new URL("../pkg/plcc_image.wasm", import.meta.url);
  if (typeof src === "string") src = new URL(src, import.meta.url);
  let module: WebAssembly.Module;
  if (src instanceof WebAssembly.Module) {
    module = src;
  } else if (src instanceof URL) {
    if (src.protocol === "file:") {
      // Node (tests, tools): no fetch for file URLs.
      const fs = await import("node:fs/promises");
      module = await WebAssembly.compile(await fs.readFile(src));
    } else {
      module = await WebAssembly.compile(await (await fetch(src)).arrayBuffer());
    }
  } else if (src instanceof Response || (typeof (src as Promise<Response>).then === "function")) {
    module = await WebAssembly.compile(await (await (src as Promise<Response>)).arrayBuffer());
  } else {
    module = await WebAssembly.compile(src as BufferSource);
  }
  // The module imports nothing (std on wasm32-unknown-unknown needs no host).
  const { exports } = await WebAssembly.instantiate(module, {});
  return exports as unknown as Exports;
}

/**
 * Load the WebAssembly linker. Called on first use; call it early to start the
 * download. `source`: the .wasm (bytes, URL, Response or a compiled module);
 * default `../pkg/plcc_image.wasm` next to this file.
 */
export function load(source?: Source): Promise<void> {
  ready ??= instantiate(source).catch((e) => {
    ready = undefined;
    throw e;
  });
  return ready.then(() => undefined);
}

function buildIdBytes(id: Uint8Array | string): Uint8Array {
  if (typeof id === "string") {
    if (!/^[0-9a-fA-F]{32}$/.test(id)) throw new ImageError(`build id "${id}" is not 32 hex digits`);
    return Uint8Array.from({ length: 16 }, (_, i) => parseInt(id.slice(2 * i, 2 * i + 2), 16));
  }
  if (id.length !== 16) throw new ImageError(`a build id is 16 bytes, not ${id.length}`);
  return id;
}

/**
 * Link `object` (the ELF object plcc compiled for `device`, e.g. with
 * `--device arduino-opta`) into a program image for the device's program
 * slot. Throws ImageError with the linker's message (an import that is not a
 * runtime service, a program too big for the slot, ...). The result passes
 * the runtime's checks (checkImage) or this throws.
 */
export async function buildDeviceImage(device: DeviceLike, object: Uint8Array, options: BuildOptions = {}): Promise<DeviceImage> {
  const layout = programLayout(device);
  if (!ready) void load();
  const w = await ready!;
  const id = new TextEncoder().encode(layout.targetId);
  const bid = options.buildId === undefined ? undefined : buildIdBytes(options.buildId);
  const objPtr = w.plcc_image_alloc(object.length);
  const idPtr = w.plcc_image_alloc(id.length);
  const bidPtr = bid ? w.plcc_image_alloc(16) : 0;
  try {
    new Uint8Array(w.memory.buffer, objPtr, object.length).set(object);
    new Uint8Array(w.memory.buffer, idPtr, id.length).set(id);
    if (bid) new Uint8Array(w.memory.buffer, bidPtr, 16).set(bid);
    const rc = w.plcc_image_link(
      objPtr,
      object.length,
      idPtr,
      id.length,
      layout.targetVersion,
      layout.abi,
      layout.slotAddr,
      layout.slotSize,
      layout.ramAddr,
      layout.ramSize,
      layout.services,
      layout.hardFloat ? 1 : 0,
      bidPtr,
    );
    // Read the results after the call: the memory may have grown.
    const report = JSON.parse(new TextDecoder().decode(new Uint8Array(w.memory.buffer, w.plcc_image_report_ptr(), w.plcc_image_report_len())));
    if (rc !== 0 || !report.ok) throw new ImageError(report.error ?? "the image linker failed");
    const bytes = new Uint8Array(w.memory.buffer, w.plcc_image_output_ptr(), w.plcc_image_output_len()).slice();
    // Never hand out an image the runtime would refuse.
    const header = checkImage(bytes, layout, layout.targetVersion);
    return {
      bytes,
      address: report.address,
      size: report.size,
      buildId: report.buildId,
      header,
      imports: report.imports,
      sections: report.sections,
      layout,
    };
  } finally {
    w.plcc_image_free(objPtr, object.length);
    w.plcc_image_free(idPtr, id.length);
    if (bid) w.plcc_image_free(bidPtr, 16);
  }
}
