// SPDX-License-Identifier: MPL-2.0
//
// The parts of WebUSB (https://wicg.github.io/webusb/) this package uses,
// declared structurally so a real `USBDevice` and a test double both fit.

export interface UsbSetup {
  requestType: "standard" | "class" | "vendor";
  recipient: "device" | "interface" | "endpoint" | "other";
  request: number;
  value: number;
  index: number;
}

export interface UsbInResult {
  data?: DataView;
  status: "ok" | "stall" | "babble";
}

export interface UsbOutResult {
  bytesWritten: number;
  status: "ok" | "stall";
}

export interface UsbAlternateLike {
  alternateSetting: number;
  interfaceClass: number;
  interfaceSubclass: number;
  interfaceProtocol: number;
  interfaceName?: string | null;
}

export interface UsbInterfaceLike {
  interfaceNumber: number;
  alternates: UsbAlternateLike[];
}

export interface UsbConfigurationLike {
  configurationValue: number;
  interfaces: UsbInterfaceLike[];
}

export interface UsbDeviceLike {
  vendorId: number;
  productId: number;
  productName?: string | null;
  serialNumber?: string | null;
  opened: boolean;
  configuration: UsbConfigurationLike | null;
  configurations: UsbConfigurationLike[];
  open(): Promise<void>;
  close(): Promise<void>;
  selectConfiguration(value: number): Promise<void>;
  claimInterface(n: number): Promise<void>;
  releaseInterface(n: number): Promise<void>;
  selectAlternateInterface(n: number, alt: number): Promise<void>;
  controlTransferIn(setup: UsbSetup, length: number): Promise<UsbInResult>;
  controlTransferOut(setup: UsbSetup, data?: BufferSource): Promise<UsbOutResult>;
}

/** The parts of Web Serial (https://wicg.github.io/serial/) used by the 1200-baud touch. */
export interface SerialPortLike {
  open(options: { baudRate: number }): Promise<void>;
  setSignals(signals: { dataTerminalReady?: boolean; requestToSend?: boolean }): Promise<void>;
  close(): Promise<void>;
  getInfo?(): { usbVendorId?: number; usbProductId?: number };
}
