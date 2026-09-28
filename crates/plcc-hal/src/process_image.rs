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

    /// Copy the input image into `dst` (the program's `%I` area) — the *latch*
    /// at the start of a scan cycle. Copies `min(dst.len(), input_size)` bytes and
    /// returns that count. Override with a bulk copy where one is available.
    fn read_inputs(&self, dst: &mut [u8]) -> usize {
        let n = dst.len().min(self.input_size() as usize);
        for (i, b) in dst[..n].iter_mut().enumerate() {
            *b = self.read_input(i as u32).unwrap_or(0);
        }
        n
    }

    /// Copy `src` (the program's `%Q` area) into the output image — the *flush*
    /// at the end of a scan cycle. Returns the number of bytes copied.
    fn write_outputs(&mut self, src: &[u8]) -> usize {
        let n = src.len().min(self.output_size() as usize);
        for (i, b) in src[..n].iter().enumerate() {
            self.write_output(i as u32, *b);
        }
        n
    }
}

// Safety: ProcessImageLayout contains raw pointers but is only used for FFI
// layout description. The actual safety is managed by the ProcessImage implementor.
unsafe impl Send for ProcessImageLayout {}
unsafe impl Sync for ProcessImageLayout {}
