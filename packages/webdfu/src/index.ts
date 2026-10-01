// SPDX-License-Identifier: MPL-2.0
//
// WebUSB DFU 1.1 with ST DfuSe extensions, limited by built-in per-USB-id
// floors and device profiles so a bootloader can never be erased or
// overwritten. Device manifests become profiles through profileFromManifest,
// which only narrows. See docs/studio-wasm.md and docs/device-manifest.md.

export { DfuError, DfuInterface, State, STATUS, describeStatus, stateName, type DfuStatus, type FunctionalDescriptor, type Sleep } from "./dfu";
export { DfuseDevice, SafetyError, type FlashOptions, type OpenOptions, type Phase, type Progress, type WritePlan } from "./dfuse";
export { LayoutError, parseLayout, sectorAt, type MemoryLayout, type Sector } from "./layout";
export { OPTA, OPTA_PROGRAM, PROFILES, type DeviceProfile } from "./profiles";
export { FLOORS, floorFor, usbKey, type Floor } from "./floors";
export { ManifestError, profileFromManifest, programProfileFromManifest, type ManifestFlash } from "./manifest";
export { isRuntimePort, touch1200, waitForDfuDevice, type UsbLike } from "./touch";
export type { SerialPortLike, UsbDeviceLike } from "./usb";
