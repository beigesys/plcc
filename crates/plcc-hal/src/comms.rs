// SPDX-License-Identifier: MPL-2.0
//! Communication services for HMI / OPC UA variable access.

use thiserror::Error;

/// Errors from variable access.
#[derive(Debug, Error)]
pub enum VarAccessError {
    #[error("variable not found: {0}")]
    NotFound(String),
    #[error("type mismatch: expected {expected}, got {actual}")]
    TypeMismatch { expected: String, actual: String },
    #[error("read-only variable: {0}")]
    ReadOnly(String),
    #[error("access error: {0}")]
    Other(String),
}

/// Trait for external variable access (HMI, OPC UA, etc.).
///
/// Allows external systems to read and write PLC variables by name.
pub trait VariableAccess {
    /// Read a variable's value as raw bytes. Returns the bytes and a type tag.
    fn read_variable(&self, name: &str) -> Result<(Vec<u8>, String), VarAccessError>;

    /// Write a variable's value from raw bytes.
    fn write_variable(&mut self, name: &str, data: &[u8]) -> Result<(), VarAccessError>;

    /// List all accessible variable names.
    fn list_variables(&self) -> Vec<String>;
}
