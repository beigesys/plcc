// SPDX-License-Identifier: MPL-2.0
//! A compiled plcc module, seen through its runtime contract.
//!
//! Every module plcc compiles exports one descriptor, `plcc_app` (also returned by
//! `plcc_get_app()`), that points at everything a runtime needs: the process image,
//! the task table, `plcc_init` / `plcc_run_task`, and the RETAIN regions. The types
//! here mirror it field for field (`#[repr(C)]`, see `docs/process-image.md` and the
//! header `plcc compile --emit-header` writes). [`Application`] wraps a descriptor
//! however it was obtained — a statically linked object, `dlopen`, or a JIT.

use core::ffi::{CStr, c_char};
use thiserror::Error;

use crate::fault::{Fault, catch_fault};
use crate::process_image::ProcessImageLayout;

/// The descriptor layout version this crate understands.
pub const ABI_VERSION: u32 = 1;

/// `plcc_program_instance_t`.
#[repr(C)]
#[derive(Debug)]
pub struct RawProgramInstance {
    pub name: *const c_char,
    pub program_type: *const c_char,
    // `C-unwind`: the C calling convention, but a fault handler may unwind out
    // of the compiled code (see `fault.rs`).
    pub init: extern "C-unwind" fn(*mut u8),
    pub scan: extern "C-unwind" fn(*mut u8),
    pub state: *mut u8,
    pub state_size: u64,
}

/// `plcc_task_t`.
#[repr(C)]
#[derive(Debug)]
pub struct RawTask {
    pub name: *const c_char,
    /// Cycle time in nanoseconds; 0 for an event-only or free-running task.
    pub interval_ns: i64,
    /// IEC 61131-3: 0 is the highest priority.
    pub priority: u32,
    pub program_count: u32,
    /// Current value of the task's SINGLE input, or `None`.
    pub single: Option<extern "C" fn() -> u8>,
    pub programs: *const RawProgramInstance,
}

/// `plcc_retain_region_t`.
#[repr(C)]
#[derive(Debug)]
pub struct RawRetainRegion {
    pub name: *const c_char,
    pub data: *mut u8,
    pub size: u64,
}

/// `plcc_app_t`.
#[repr(C)]
#[derive(Debug)]
pub struct RawApp {
    pub abi_version: u32,
    pub task_count: u32,
    pub tasks: *const RawTask,
    pub image: *const ProcessImageLayout,
    pub init: extern "C-unwind" fn(),
    pub run_task: extern "C-unwind" fn(u32),
    pub retain: *const RawRetainRegion,
    pub retain_count: u32,
    pub retain_signature: u32,
}

/// Errors from loading a compiled module or restoring its retain data.
#[derive(Debug, Error)]
pub enum AppError {
    #[error("null plcc_app descriptor")]
    Null,
    #[error("plcc_app ABI version {found}, this runtime supports {ABI_VERSION}")]
    AbiVersion { found: u32 },
    #[error("retain data does not match this program (signature {found:#010x}, expected {expected:#010x})")]
    RetainMismatch { found: u32, expected: u32 },
    #[error("retain data is malformed")]
    RetainMalformed,
}

/// A task as described by the compiled module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskInfo {
    pub name: String,
    pub interval_ns: u64,
    pub priority: u32,
    pub has_single: bool,
    /// `(instance name, PROGRAM type)` in execution order.
    pub programs: Vec<(String, String)>,
}

fn string(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    // Safety: the compiler emits NUL-terminated constant strings.
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

/// Magic at the start of a retain blob produced by [`Application::retain_snapshot`].
const RETAIN_MAGIC: &[u8; 4] = b"PLCR";

/// A loaded, compiled plcc module.
///
/// The module's code and data must outlive this value; the constructors are
/// `unsafe` for that reason.
pub struct Application {
    raw: &'static RawApp,
}

// The descriptor is immutable; the state it points at is only touched through
// `&mut self` methods or by the task functions the caller chooses to run.
unsafe impl Send for Application {}

impl Application {
    /// Wrap a `plcc_app` descriptor.
    ///
    /// # Safety
    /// `raw` must point at the `plcc_app` of a loaded plcc module that stays loaded
    /// for as long as the returned value (and anything derived from it) is used.
    pub unsafe fn from_raw(raw: *const RawApp) -> Result<Self, AppError> {
        let raw = unsafe { raw.as_ref() }.ok_or(AppError::Null)?;
        if raw.abi_version != ABI_VERSION {
            return Err(AppError::AbiVersion {
                found: raw.abi_version,
            });
        }
        Ok(Self { raw })
    }

    /// Wrap the descriptor returned by a module's `plcc_get_app`.
    ///
    /// # Safety
    /// As [`Self::from_raw`]; `get_app` must be that module's `plcc_get_app`.
    pub unsafe fn from_get_app(get_app: extern "C" fn() -> *const RawApp) -> Result<Self, AppError> {
        unsafe { Self::from_raw(get_app()) }
    }

    pub fn raw(&self) -> &RawApp {
        self.raw
    }

    fn raw_tasks(&self) -> &[RawTask] {
        if self.raw.task_count == 0 {
            return &[];
        }
        unsafe { core::slice::from_raw_parts(self.raw.tasks, self.raw.task_count as usize) }
    }

    fn raw_retain(&self) -> &[RawRetainRegion] {
        if self.raw.retain_count == 0 {
            return &[];
        }
        unsafe { core::slice::from_raw_parts(self.raw.retain, self.raw.retain_count as usize) }
    }

    /// Number of tasks.
    pub fn task_count(&self) -> usize {
        self.raw.task_count as usize
    }

    /// Describe every task.
    pub fn tasks(&self) -> Vec<TaskInfo> {
        self.raw_tasks()
            .iter()
            .map(|t| {
                let programs = if t.program_count == 0 {
                    &[][..]
                } else {
                    unsafe { core::slice::from_raw_parts(t.programs, t.program_count as usize) }
                };
                TaskInfo {
                    name: string(t.name),
                    interval_ns: t.interval_ns.max(0) as u64,
                    priority: t.priority,
                    has_single: t.single.is_some(),
                    programs: programs
                        .iter()
                        .map(|p| (string(p.name), string(p.program_type)))
                        .collect(),
                }
            })
            .collect()
    }

    /// The module's process image descriptor.
    pub fn image(&self) -> ProcessImageLayout {
        unsafe { *self.raw.image }
    }

    fn area(ptr: *const u8, size: u32) -> &'static mut [u8] {
        if size == 0 || ptr.is_null() {
            return &mut [];
        }
        unsafe { core::slice::from_raw_parts_mut(ptr as *mut u8, size as usize) }
    }

    /// The module's `%I` area (written by the runtime when latching inputs).
    pub fn inputs_mut(&mut self) -> &mut [u8] {
        let i = self.image();
        Self::area(i.input_ptr, i.input_size)
    }

    /// The module's `%Q` area (read by the runtime when flushing outputs).
    pub fn outputs(&self) -> &[u8] {
        let i = self.image();
        Self::area(i.output_ptr, i.output_size)
    }

    /// The module's `%M` area.
    pub fn memory_mut(&mut self) -> &mut [u8] {
        let i = self.image();
        Self::area(i.marker_ptr, i.marker_size)
    }

    /// `plcc_init()`: initialize every program instance. Call once before any task.
    pub fn init(&mut self) {
        (self.raw.init)()
    }

    /// `plcc_run_task(task)`: run every program instance of one task once.
    pub fn run_task(&mut self, task: usize) {
        (self.raw.run_task)(task as u32)
    }

    /// [`Self::init`], returning a runtime fault (e.g. a division by zero in an
    /// initial value) instead of unwinding — when the module's `plcc_fault` is
    /// [`unwinding_fault_handler`](crate::fault::unwinding_fault_handler), as in
    /// the JIT.
    pub fn try_init(&mut self) -> Result<(), Fault> {
        let init = self.raw.init;
        catch_fault(|| init())
    }

    /// [`Self::run_task`], returning a runtime fault instead of unwinding (see
    /// [`Self::try_init`]). After a fault the task stopped part-way through its
    /// scan; the program's variables are as the fault left them.
    pub fn try_run_task(&mut self, task: usize) -> Result<(), Fault> {
        let run = self.raw.run_task;
        catch_fault(|| run(task as u32))
    }

    /// Current value of a task's SINGLE input, if it has one.
    pub fn single(&self, task: usize) -> Option<bool> {
        let f = self.raw_tasks().get(task)?.single?;
        Some(f() != 0)
    }

    /// Layout signature of the RETAIN regions (changes when they change).
    pub fn retain_signature(&self) -> u32 {
        self.raw.retain_signature
    }

    /// `(name, size)` of every RETAIN region.
    pub fn retain_regions(&self) -> Vec<(String, usize)> {
        self.raw_retain()
            .iter()
            .map(|r| (string(r.name), r.size as usize))
            .collect()
    }

    /// Serialize every RETAIN variable: `"PLCR"`, signature, then each region's
    /// bytes in table order (all integers little-endian).
    pub fn retain_snapshot(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(RETAIN_MAGIC);
        out.extend_from_slice(&self.raw.retain_signature.to_le_bytes());
        for r in self.raw_retain() {
            out.extend_from_slice(&(r.size as u32).to_le_bytes());
            out.extend_from_slice(unsafe { core::slice::from_raw_parts(r.data, r.size as usize) });
        }
        out
    }

    /// Restore a [`Self::retain_snapshot`]. Call after [`Self::init`] (a warm
    /// start). A snapshot from a program with a different RETAIN layout is rejected
    /// and nothing is written.
    pub fn restore_retain(&mut self, data: &[u8]) -> Result<(), AppError> {
        if data.len() < 8 || &data[..4] != RETAIN_MAGIC {
            return Err(AppError::RetainMalformed);
        }
        let found = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        if found != self.raw.retain_signature {
            return Err(AppError::RetainMismatch {
                found,
                expected: self.raw.retain_signature,
            });
        }
        // Validate the whole blob before writing anything.
        let mut pos = 8;
        let mut spans = Vec::new();
        for r in self.raw_retain() {
            let size = data
                .get(pos..pos + 4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
                .ok_or(AppError::RetainMalformed)?;
            if size != r.size as usize || data.len() < pos + 4 + size {
                return Err(AppError::RetainMalformed);
            }
            spans.push((pos + 4, size));
            pos += 4 + size;
        }
        for (r, (start, size)) in self.raw_retain().iter().zip(spans) {
            unsafe { core::ptr::copy_nonoverlapping(data[start..].as_ptr(), r.data, size) };
        }
        Ok(())
    }
}
