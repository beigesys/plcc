// SPDX-License-Identifier: MPL-2.0

//! The manifest file format (what a `.toml` file holds, [`Manifest`]) and the
//! expanded form every consumer works with ([`Device`]: `repeat` groups
//! unrolled, addresses canonical). docs/device-manifest.md is the spec.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The manifest format version this crate reads and writes.
pub const SCHEMA_VERSION: u32 = 1;

/// A device manifest as written in its TOML file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "plcc device manifest")]
pub struct Manifest {
    pub device: DeviceInfo,
    pub target: Target,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flash: Option<Flash>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub console: Option<Console>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modbus: Option<Modbus>,
    /// I/O points. An entry with `repeat` stands for several points.
    #[serde(default)]
    pub io: Vec<IoEntry>,
}

/// `[device]`: identity and versions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeviceInfo {
    /// Stable id, lowercase letters, digits and `-` (`arduino-opta`). The
    /// runtime's `info` command reports the same id.
    pub id: String,
    /// Display name.
    pub name: String,
    pub vendor: String,
    #[serde(default)]
    pub description: String,
    /// Version of this manifest; bump it on every change to the file.
    pub version: u32,
    /// Version of the manifest format (this spec): 1.
    pub schema: u32,
    /// Where the manifest was downloaded from (reserved for a registry).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// SHA-256 of the file as published at `source`, hex (reserved for a registry).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// `[target]`: what plcc compiles for and what the runtime provides.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Target {
    /// LLVM target triple (`thumbv7em-none-eabi`, `wasm32-unknown-unknown`).
    pub triple: String,
    /// LLVM CPU name (`cortex-m7`); absent means the triple's generic CPU.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<String>,
    /// LLVM target features, each `+name` or `-name` (`+fp-armv8d16`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    /// ARM float ABI: `soft` (no FPU instructions), `softfp` (FPU
    /// instructions, floats passed in integer registers) or `hard` (floats
    /// in FPU registers; needs an `eabihf` triple).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub float_abi: Option<FloatAbi>,
    pub runtime: Runtime,
    /// Process-image sizes in bytes; every program for the device is built with them.
    pub image: ImageSizes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum FloatAbi {
    Soft,
    Softfp,
    Hard,
}

impl FloatAbi {
    pub fn as_str(self) -> &'static str {
        match self {
            FloatAbi::Soft => "soft",
            FloatAbi::Softfp => "softfp",
            FloatAbi::Hard => "hard",
        }
    }
}

/// `[target.runtime]`: the firmware or host that runs compiled programs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Runtime {
    /// Runtime name (`plcc-arduino`, `plcc-studio-sim`), as its `info` reports it.
    pub kind: String,
    /// Runtime-contract ABI version (docs/process-image.md, `PLCC_ABI_VERSION`).
    pub abi: u32,
}

/// Sizes of the `%I`, `%Q` and `%M` areas in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImageSizes {
    #[serde(rename = "I")]
    pub i: u32,
    #[serde(rename = "Q")]
    pub q: u32,
    #[serde(rename = "M")]
    pub m: u32,
}

/// `[flash]`: how a built application gets onto the device.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Flash {
    pub method: FlashMethod,
    /// USB ids of the bootloader (the device being flashed).
    pub usb: Vec<UsbId>,
    /// DFU alternate setting.
    #[serde(default)]
    pub alt: u8,
    /// The DfuSe alternate's layout name must start with this (`Internal Flash`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    /// Where the application image goes.
    pub address: u32,
    /// Largest application image, bytes.
    pub max_size: u32,
    /// Start the application after the download.
    #[serde(default = "yes")]
    pub leave: bool,
    /// How to get from the running application into the bootloader.
    #[serde(default = "no_reboot")]
    pub reboot: Reboot,
    /// USB ids of the running application (its serial port), for `reboot`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runtime_usb: Vec<UsbId>,
    /// Flash that must never be erased or written (a bootloader).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub protected: Vec<Region>,
}

fn yes() -> bool {
    true
}

fn no_reboot() -> Reboot {
    Reboot::None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum FlashMethod {
    /// USB DFU 1.1 with ST's DfuSe extensions (STM32 system and Arduino bootloaders).
    Dfuse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Reboot {
    /// The user enters the bootloader by hand.
    None,
    /// Open the application's serial port at 1200 baud and close it (Arduino).
    #[serde(rename = "1200-baud-touch")]
    Touch1200,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UsbId {
    pub vid: u16,
    pub pid: u16,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub start: u32,
    pub size: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

impl Region {
    /// One past the last byte (u64, so `start + size` cannot overflow).
    pub fn end(&self) -> u64 {
        self.start as u64 + self.size as u64
    }
}

/// `[console]`: the runtime's line-oriented text console.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Console {
    pub transport: ConsoleTransport,
    pub baud: u32,
    /// Commands the runtime answers (docs/device-manifest.md, "Console protocol").
    pub commands: Vec<ConsoleCommand>,
    /// Format of the `img` reply.
    pub img_format: ImgFormat,
    /// `%M` bytes the `img` reply includes (from byte 0).
    pub img_m_bytes: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ConsoleTransport {
    /// A USB CDC serial port (WebSerial in the browser).
    Webserial,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ConsoleCommand {
    /// `info`: one line of JSON identifying the runtime.
    Info,
    /// `img`: the process image.
    Img,
    /// `mw <n> <value>`: write `%MWn`.
    Mw,
}

impl ConsoleCommand {
    pub fn as_str(self) -> &'static str {
        match self {
            ConsoleCommand::Info => "info",
            ConsoleCommand::Img => "img",
            ConsoleCommand::Mw => "mw",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ImgFormat {
    /// `I: <bytes>  Q: <bytes>  M: <bytes>`, each byte in unpadded upper-case hex.
    HexAreas,
}

/// `[modbus]`: the runtime's Modbus server.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Modbus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtu: Option<ModbusRtu>,
    /// Register tables and the process-image addresses they alias.
    #[serde(default)]
    pub map: Vec<ModbusMap>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModbusRtu {
    /// Physical interface (`RS485`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface: Option<String>,
    /// Server (slave) address, 1..247.
    pub unit: u8,
    pub baud: u32,
    #[serde(default = "eight")]
    pub data_bits: u8,
    pub parity: Parity,
    #[serde(default = "one")]
    pub stop_bits: u8,
}

fn eight() -> u8 {
    8
}

fn one() -> u8 {
    1
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Parity {
    None,
    Even,
    Odd,
}

/// One Modbus table range. Consecutive items map to consecutive addresses:
/// bits for coils and discrete inputs, words for registers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModbusMap {
    pub table: ModbusTable,
    /// First item number (0-based protocol address).
    #[serde(default)]
    pub start: u16,
    pub count: u16,
    /// Process-image address of item `start` (`%MW0`, `%QX0.0`).
    pub address: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ModbusTable {
    /// Read/write bits (function codes 1, 5, 15).
    Coils,
    /// Read-only bits (function code 2).
    Discrete,
    /// Read-only registers (function code 4).
    Input,
    /// Read/write registers (function codes 3, 6, 16).
    Holding,
}

impl ModbusTable {
    pub fn is_bits(self) -> bool {
        matches!(self, ModbusTable::Coils | ModbusTable::Discrete)
    }
    pub fn writable(self) -> bool {
        matches!(self, ModbusTable::Coils | ModbusTable::Holding)
    }
}

/// An `[[io]]` entry: one point, or with `repeat` a numbered group of points
/// whose string fields may contain `{expr}` (integer arithmetic on `n`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IoEntry {
    /// Number of points this entry stands for; `n` runs from `from`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<u32>,
    /// First value of `n` (default 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<i64>,
    /// Stable id, unique in the manifest (`I{n}`).
    pub id: String,
    /// Terminal name printed on the device.
    pub terminal: String,
    pub label: String,
    /// Heading the point is listed under.
    pub group: String,
    pub dir: Dir,
    pub kind: Kind,
    /// IEC elementary type (`BOOL`, `INT`, `UINT`, `REAL`).
    #[serde(rename = "type")]
    pub iec_type: String,
    /// Process-image address (`%IX0.{n-1}`).
    pub address: String,
    /// Raw value range (analog points), `[min, max]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<[i64; 2]>,
    /// Engineering value at the raw `range` ends, `[at_min, at_max]` (linear).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eng: Option<[f64; 2]>,
    /// Engineering units (`V`, `mA`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub units: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    In,
    Out,
    Mem,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Digital,
    Analog,
    Register,
}

/// A validated manifest with every `repeat` group expanded: the form the CLI,
/// the browser and the flashing code use.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Device {
    pub device: DeviceInfo,
    pub target: Target,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flash: Option<Flash>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub console: Option<Console>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modbus: Option<Modbus>,
    pub io: Vec<IoPoint>,
}

/// One I/O point of an expanded manifest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct IoPoint {
    pub id: String,
    pub terminal: String,
    pub label: String,
    pub group: String,
    pub dir: Dir,
    pub kind: Kind,
    #[serde(rename = "type")]
    pub iec_type: String,
    /// Canonical address (`%IX0.3`, `%IW1`).
    pub address: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<[i64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eng: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub units: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}
