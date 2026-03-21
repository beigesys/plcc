// SPDX-License-Identifier: MPL-2.0
//! Process image for %I (inputs), %Q (outputs), and %M (markers).

/// FFI-safe layout descriptor passed to compiled PLC programs.
/// Codegen emits code that reads/writes through these pointers.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ProcessImageLayout {
    pub input_ptr: *const u8,
    pub input_size: u32,
    pub output_ptr: *mut u8,
    pub output_size: u32,
    pub marker_ptr: *mut u8,
    pub marker_size: u32,
}

/// Trait for managing the process image buffers.
pub trait ProcessImage {
    /// Returns the current layout descriptor for FFI.
    fn layout(&self) -> ProcessImageLayout;

    /// Read a byte from the input image at the given offset.
    /// Returns `None` if the offset is out of bounds.
    fn read_input(&self, byte_offset: u32) -> Option<u8>;

    /// Read a byte from the output image at the given offset.
    /// Returns `None` if the offset is out of bounds.
    fn read_output(&self, byte_offset: u32) -> Option<u8>;

    /// Write a byte to the output image at the given offset.
    /// Returns `false` if the offset is out of bounds.
    fn write_output(&mut self, byte_offset: u32, value: u8) -> bool;

    /// Read a byte from the marker image at the given offset.
    /// Returns `None` if the offset is out of bounds.
    fn read_marker(&self, byte_offset: u32) -> Option<u8>;

    /// Write a byte to the marker image at the given offset.
    /// Returns `false` if the offset is out of bounds.
    fn write_marker(&mut self, byte_offset: u32, value: u8) -> bool;

    /// Write a byte to the input image (for simulation / fieldbus drivers).
    /// Returns `false` if the offset is out of bounds.
    fn write_input(&mut self, byte_offset: u32, value: u8) -> bool;

    /// Size of the input image in bytes.
    fn input_size(&self) -> u32;

    /// Size of the output image in bytes.
    fn output_size(&self) -> u32;

    /// Size of the marker image in bytes.
    fn marker_size(&self) -> u32;
}

// Safety: ProcessImageLayout contains raw pointers but is only used for FFI
// layout description. The actual safety is managed by the ProcessImage implementor.
unsafe impl Send for ProcessImageLayout {}
unsafe impl Sync for ProcessImageLayout {}
