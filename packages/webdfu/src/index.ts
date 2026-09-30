// SPDX-License-Identifier: MPL-2.0
//
// WebUSB DFU 1.1 with ST DfuSe extensions, limited by device profiles so a
// bootloader can never be erased or overwritten. See docs/studio-wasm.md.

export { DfuError, DfuInterface, State, STATUS, describeStatus, stateName, type DfuStatus, type FunctionalDescriptor, type Sleep } from "./dfu";
export { DfuseDevice, SafetyError, type FlashOptions, type OpenOptions, type Phase, type Progress, type WritePlan } from "./dfuse";
export { LayoutError, parseLayout, sectorAt, type MemoryLayout, type Sector } from "./layout";
export { OPTA, PROFILES, type DeviceProfile } from "./profiles";
export { isRuntimePort, touch1200, waitForDfuDevice, type UsbLike } from "./touch";
export type { SerialPortLike, UsbDeviceLike } from "./usb";
