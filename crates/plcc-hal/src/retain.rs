// SPDX-License-Identifier: MPL-2.0
//! Persistent (RETAIN) variable storage.

use thiserror::Error;

/// Errors from retain storage.
#[derive(Debug, Error)]
pub enum RetainError {
    #[error("retain storage not available")]
    NotAvailable,
    #[error("data too large: {size} bytes exceeds capacity of {capacity} bytes")]
    TooLarge { size: usize, capacity: usize },
    #[error("no retained data found")]
    NoData,
    #[error("retained data corrupted")]
    Corrupted,
    #[error("I/O error: {0}")]
    Io(String),
}

/// Trait for persistent variable storage.
///
/// RETAIN variables survive power cycles. The implementation must guarantee
/// that data written via `save()` can be read back via `restore()` after
/// a restart.
pub trait RetainStorage {
    /// Maximum capacity in bytes.
    fn capacity(&self) -> usize;

    /// Persist the given data. Overwrites any previously saved data.
    fn save(&mut self, data: &[u8]) -> Result<(), RetainError>;

    /// Restore previously saved data.
    /// Returns the data, or an error if nothing was saved or data is corrupted.
    fn restore(&self) -> Result<Vec<u8>, RetainError>;

    /// Clear all retained data.
    fn clear(&mut self) -> Result<(), RetainError>;
}
