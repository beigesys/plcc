// SPDX-License-Identifier: MPL-2.0

/// Trait for all function blocks — defines the lifecycle.
pub trait FunctionBlock {
    /// Initialize the FB state to default values.
    fn init(&mut self);
    /// Execute one scan cycle.
    fn scan(&mut self);
    /// Reset the FB to initial state.
    fn reset(&mut self);
}
