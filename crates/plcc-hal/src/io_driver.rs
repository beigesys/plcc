// SPDX-License-Identifier: MPL-2.0
//! Fieldbus I/O driver abstraction.

use thiserror::Error;

/// Supported fieldbus protocols.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldbusKind {
    EtherCAT = 0,
    ModbusTcp = 1,
    ModbusRtu = 2,
    Profinet = 3,
    EtherNetIp = 4,
    CANopen = 5,
    Custom = 255,
}

/// Descriptor for a single I/O module on the fieldbus.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IoModuleDescriptor {
    /// Module index on the bus.
    pub index: u16,
    /// Offset in the input image where this module's inputs start.
    pub input_offset: u32,
    /// Number of input bytes this module contributes.
    pub input_size: u16,
    /// Offset in the output image where this module's outputs start.
    pub output_offset: u32,
    /// Number of output bytes this module contributes.
    pub output_size: u16,
    /// The fieldbus protocol this module uses.
    pub kind: FieldbusKind,
}

/// Errors from I/O drivers.
#[derive(Debug, Error)]
pub enum IoDriverError {
    #[error("bus not initialized")]
    NotInitialized,
    #[error("bus communication timeout")]
    Timeout,
    #[error("module {index} not responding")]
    ModuleOffline { index: u16 },
    #[error("bus error: {0}")]
    BusError(String),
    #[error("I/O driver error: {0}")]
    Other(String),
}

/// Trait for fieldbus I/O drivers.
///
/// Each driver manages one fieldbus network and its modules.
pub trait IoDriver {
    /// Initialize the fieldbus. Called once at startup.
    fn init(&mut self) -> Result<(), IoDriverError>;

    /// Read inputs from all modules into the provided buffer.
    /// The buffer corresponds to the input region of the process image.
    fn read_inputs(&mut self, input_buffer: &mut [u8]) -> Result<(), IoDriverError>;

    /// Write outputs from the provided buffer to all modules.
    /// The buffer corresponds to the output region of the process image.
    fn write_outputs(&mut self, output_buffer: &[u8]) -> Result<(), IoDriverError>;

    /// Check whether the fieldbus is operational.
    fn is_operational(&self) -> bool;

    /// Shut down the fieldbus. Called once at shutdown.
    fn shutdown(&mut self) -> Result<(), IoDriverError>;

    /// Return descriptors for all modules on this bus.
    fn modules(&self) -> &[IoModuleDescriptor];
}
