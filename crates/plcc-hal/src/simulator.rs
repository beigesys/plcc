// SPDX-License-Identifier: MPL-2.0
//! Linux/desktop simulator platform.
//!
//! Uses heap-allocated buffers for I/O image, `std::time::Instant` for clock,
//! and file-backed retain storage. Suitable for development, testing, and
//! simulation on any platform with `std`.

use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::clock::Clock;
use crate::comms::{VarAccessError, VariableAccess};
use crate::diagnostics::{DiagCode, DiagLevel, DiagnosticSink};
use crate::io_driver::{IoDriver, IoDriverError, IoModuleDescriptor};
use crate::platform::{Platform, PlatformError};
use crate::process_image::{ProcessImage, ProcessImageLayout};
use crate::retain::{RetainError, RetainStorage};
use crate::task::{TaskConfig, TaskError, TaskHandle, TaskScheduler};
use crate::watchdog::{Watchdog, WatchdogError};

// ---------------------------------------------------------------------------
// SimClock
// ---------------------------------------------------------------------------

/// Monotonic clock based on `std::time::Instant`.
pub struct SimClock {
    epoch: Instant,
    scan_start: Option<Instant>,
}

impl SimClock {
    fn new() -> Self {
        Self {
            epoch: Instant::now(),
            scan_start: None,
        }
    }
}

impl Clock for SimClock {
    fn now_ns(&self) -> u64 {
        self.epoch.elapsed().as_nanos() as u64
    }

    fn wall_clock_ns(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    }

    fn elapsed_since_last_scan_ns(&self) -> u64 {
        match self.scan_start {
            Some(start) => start.elapsed().as_nanos() as u64,
            None => 0,
        }
    }

    fn begin_scan(&mut self) {
        self.scan_start = Some(Instant::now());
    }
}

// ---------------------------------------------------------------------------
// SimProcessImage
// ---------------------------------------------------------------------------

/// Heap-allocated process image.
pub struct SimProcessImage {
    input: Vec<u8>,
    output: Vec<u8>,
    marker: Vec<u8>,
}

impl SimProcessImage {
    fn new(input_size: usize, output_size: usize, marker_size: usize) -> Self {
        Self {
            input: vec![0u8; input_size],
            output: vec![0u8; output_size],
            marker: vec![0u8; marker_size],
        }
    }
}

impl ProcessImage for SimProcessImage {
    fn layout(&self) -> ProcessImageLayout {
        ProcessImageLayout {
            input_ptr: self.input.as_ptr(),
            input_size: self.input.len() as u32,
            output_ptr: self.output.as_ptr() as *mut u8,
            output_size: self.output.len() as u32,
            marker_ptr: self.marker.as_ptr() as *mut u8,
            marker_size: self.marker.len() as u32,
        }
    }

    fn read_input(&self, byte_offset: u32) -> Option<u8> {
        self.input.get(byte_offset as usize).copied()
    }

    fn read_output(&self, byte_offset: u32) -> Option<u8> {
        self.output.get(byte_offset as usize).copied()
    }

    fn write_output(&mut self, byte_offset: u32, value: u8) -> bool {
        match self.output.get_mut(byte_offset as usize) {
            Some(slot) => {
                *slot = value;
                true
            }
            None => false,
        }
    }

    fn read_marker(&self, byte_offset: u32) -> Option<u8> {
        self.marker.get(byte_offset as usize).copied()
    }

    fn write_marker(&mut self, byte_offset: u32, value: u8) -> bool {
        match self.marker.get_mut(byte_offset as usize) {
            Some(slot) => {
                *slot = value;
                true
            }
            None => false,
        }
    }

    fn write_input(&mut self, byte_offset: u32, value: u8) -> bool {
        match self.input.get_mut(byte_offset as usize) {
            Some(slot) => {
                *slot = value;
                true
            }
            None => false,
        }
    }

    fn input_size(&self) -> u32 {
        self.input.len() as u32
    }

    fn output_size(&self) -> u32 {
        self.output.len() as u32
    }

    fn marker_size(&self) -> u32 {
        self.marker.len() as u32
    }
}

// ---------------------------------------------------------------------------
// SimTaskScheduler
// ---------------------------------------------------------------------------

struct SimTask {
    _config: TaskConfig,
    running: bool,
}

/// Simple single-threaded task scheduler.
pub struct SimTaskScheduler {
    tasks: Vec<Option<SimTask>>,
}

impl SimTaskScheduler {
    fn new() -> Self {
        Self { tasks: Vec::new() }
    }
}

impl TaskScheduler for SimTaskScheduler {
    fn create_task(&mut self, config: TaskConfig) -> Result<TaskHandle, TaskError> {
        let handle = self.tasks.len() as TaskHandle;
        self.tasks.push(Some(SimTask {
            _config: config,
            running: false,
        }));
        Ok(handle)
    }

    fn start_task(&mut self, handle: TaskHandle) -> Result<(), TaskError> {
        match self.tasks.get_mut(handle as usize) {
            Some(Some(task)) => {
                task.running = true;
                Ok(())
            }
            _ => Err(TaskError::InvalidHandle(handle)),
        }
    }

    fn stop_task(&mut self, handle: TaskHandle) -> Result<(), TaskError> {
        match self.tasks.get_mut(handle as usize) {
            Some(Some(task)) => {
                task.running = false;
                Ok(())
            }
            _ => Err(TaskError::InvalidHandle(handle)),
        }
    }

    fn is_running(&self, handle: TaskHandle) -> Result<bool, TaskError> {
        match self.tasks.get(handle as usize) {
            Some(Some(task)) => Ok(task.running),
            _ => Err(TaskError::InvalidHandle(handle)),
        }
    }

    fn destroy_task(&mut self, handle: TaskHandle) -> Result<(), TaskError> {
        match self.tasks.get_mut(handle as usize) {
            Some(slot @ Some(_)) => {
                *slot = None;
                Ok(())
            }
            _ => Err(TaskError::InvalidHandle(handle)),
        }
    }

    fn task_count(&self) -> usize {
        self.tasks.iter().filter(|t| t.is_some()).count()
    }
}

// ---------------------------------------------------------------------------
// SimRetainStorage
// ---------------------------------------------------------------------------

/// File-backed retain storage.
pub struct SimRetainStorage {
    path: Option<PathBuf>,
    capacity: usize,
}

impl SimRetainStorage {
    fn new(path: Option<PathBuf>, capacity: usize) -> Self {
        Self { path, capacity }
    }
}

impl RetainStorage for SimRetainStorage {
    fn capacity(&self) -> usize {
        self.capacity
    }

    fn save(&mut self, data: &[u8]) -> Result<(), RetainError> {
        if data.len() > self.capacity {
            return Err(RetainError::TooLarge {
                size: data.len(),
                capacity: self.capacity,
            });
        }
        match &self.path {
            Some(path) => std::fs::write(path, data).map_err(|e| RetainError::Io(e.to_string())),
            None => Err(RetainError::NotAvailable),
        }
    }

    fn restore(&self) -> Result<Vec<u8>, RetainError> {
        match &self.path {
            Some(path) => {
                if !path.exists() {
                    return Err(RetainError::NoData);
                }
                std::fs::read(path).map_err(|e| RetainError::Io(e.to_string()))
            }
            None => Err(RetainError::NotAvailable),
        }
    }

    fn clear(&mut self) -> Result<(), RetainError> {
        match &self.path {
            Some(path) => {
                if path.exists() {
                    std::fs::remove_file(path).map_err(|e| RetainError::Io(e.to_string()))?;
                }
                Ok(())
            }
            None => Err(RetainError::NotAvailable),
        }
    }
}

// ---------------------------------------------------------------------------
// SimWatchdog
// ---------------------------------------------------------------------------

/// Software watchdog (no-op safety — just tracks state).
pub struct SimWatchdog {
    running: bool,
    tripped: bool,
    timeout_ns: u64,
    last_kick: Option<Instant>,
}

impl SimWatchdog {
    fn new() -> Self {
        Self {
            running: false,
            tripped: false,
            timeout_ns: 0,
            last_kick: None,
        }
    }
}

impl Watchdog for SimWatchdog {
    fn start(&mut self, timeout_ns: u64) -> Result<(), WatchdogError> {
        if self.running {
            return Err(WatchdogError::AlreadyRunning);
        }
        self.timeout_ns = timeout_ns;
        self.running = true;
        self.tripped = false;
        self.last_kick = Some(Instant::now());
        Ok(())
    }

    fn kick(&mut self) -> Result<(), WatchdogError> {
        if !self.running {
            return Err(WatchdogError::NotStarted);
        }
        // Check if we already tripped before this kick
        if let Some(last) = self.last_kick {
            if last.elapsed().as_nanos() as u64 > self.timeout_ns {
                self.tripped = true;
            }
        }
        self.last_kick = Some(Instant::now());
        Ok(())
    }

    fn stop(&mut self) -> Result<(), WatchdogError> {
        if !self.running {
            return Err(WatchdogError::NotStarted);
        }
        self.running = false;
        Ok(())
    }

    fn has_tripped(&self) -> bool {
        if !self.running {
            return self.tripped;
        }
        if self.tripped {
            return true;
        }
        match self.last_kick {
            Some(last) => last.elapsed().as_nanos() as u64 > self.timeout_ns,
            None => false,
        }
    }
}

// ---------------------------------------------------------------------------
// SimDiagnosticSink
// ---------------------------------------------------------------------------

/// Diagnostic message record.
#[derive(Debug, Clone)]
pub struct DiagRecord {
    pub level: DiagLevel,
    pub code: DiagCode,
    pub message: String,
}

/// In-memory diagnostic sink.
pub struct SimDiagnosticSink {
    records: Vec<DiagRecord>,
}

impl SimDiagnosticSink {
    fn new() -> Self {
        Self {
            records: Vec::new(),
        }
    }

    /// Access all recorded diagnostics.
    pub fn records(&self) -> &[DiagRecord] {
        &self.records
    }
}

impl DiagnosticSink for SimDiagnosticSink {
    fn report(&mut self, level: DiagLevel, code: DiagCode, message: &str) {
        self.records.push(DiagRecord {
            level,
            code,
            message: message.to_string(),
        });
    }

    fn error_count(&self) -> usize {
        self.records
            .iter()
            .filter(|r| r.level >= DiagLevel::Error)
            .count()
    }

    fn clear_errors(&mut self) {
        self.records.clear();
    }
}

// ---------------------------------------------------------------------------
// SimIoDriver
// ---------------------------------------------------------------------------

/// No-op I/O driver for the simulator (no real fieldbus).
pub struct SimIoDriver;

impl IoDriver for SimIoDriver {
    fn init(&mut self) -> Result<(), IoDriverError> {
        Ok(())
    }

    fn read_inputs(&mut self, _input_buffer: &mut [u8]) -> Result<(), IoDriverError> {
        // Simulator has no real I/O — inputs are set programmatically
        Ok(())
    }

    fn write_outputs(&mut self, _output_buffer: &[u8]) -> Result<(), IoDriverError> {
        // Simulator has no real I/O — outputs are read programmatically
        Ok(())
    }

    fn is_operational(&self) -> bool {
        true
    }

    fn shutdown(&mut self) -> Result<(), IoDriverError> {
        Ok(())
    }

    fn modules(&self) -> &[IoModuleDescriptor] {
        &[]
    }
}

// ---------------------------------------------------------------------------
// SimVariableAccess
// ---------------------------------------------------------------------------

/// Stub variable access for the simulator.
pub struct SimVariableAccess;

impl VariableAccess for SimVariableAccess {
    fn read_variable(&self, name: &str) -> Result<(Vec<u8>, String), VarAccessError> {
        Err(VarAccessError::NotFound(name.to_string()))
    }

    fn write_variable(&mut self, name: &str, _data: &[u8]) -> Result<(), VarAccessError> {
        Err(VarAccessError::NotFound(name.to_string()))
    }

    fn list_variables(&self) -> Vec<String> {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// LinuxSimulator
// ---------------------------------------------------------------------------

/// Linux/desktop simulator platform.
///
/// Uses heap-allocated buffers for I/O image, `std::time::Instant` for clock,
/// and file-backed retain storage.
pub struct LinuxSimulator {
    clock: SimClock,
    process_image: SimProcessImage,
    task_scheduler: SimTaskScheduler,
    retain_storage: SimRetainStorage,
    watchdog: SimWatchdog,
    diagnostic_sink: SimDiagnosticSink,
    variable_access: SimVariableAccess,
    io_driver: SimIoDriver,
    initialized: bool,
}

/// Builder for constructing a `LinuxSimulator` with custom sizes.
pub struct LinuxSimulatorBuilder {
    input_size: usize,
    output_size: usize,
    marker_size: usize,
    retain_path: Option<PathBuf>,
    retain_capacity: usize,
}

impl Default for LinuxSimulatorBuilder {
    fn default() -> Self {
        Self {
            input_size: 1024,
            output_size: 1024,
            marker_size: 4096,
            retain_path: None,
            retain_capacity: 64 * 1024,
        }
    }
}

impl LinuxSimulatorBuilder {
    /// Create a new builder with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the input image size in bytes.
    pub fn input_size(mut self, size: usize) -> Self {
        self.input_size = size;
        self
    }

    /// Set the output image size in bytes.
    pub fn output_size(mut self, size: usize) -> Self {
        self.output_size = size;
        self
    }

    /// Set the marker image size in bytes.
    pub fn marker_size(mut self, size: usize) -> Self {
        self.marker_size = size;
        self
    }

    /// Set the path for retain storage.
    pub fn retain_path(mut self, path: PathBuf) -> Self {
        self.retain_path = Some(path);
        self
    }

    /// Set the retain storage capacity in bytes.
    pub fn retain_capacity(mut self, capacity: usize) -> Self {
        self.retain_capacity = capacity;
        self
    }

    /// Build the simulator.
    pub fn build(self) -> LinuxSimulator {
        LinuxSimulator {
            clock: SimClock::new(),
            process_image: SimProcessImage::new(
                self.input_size,
                self.output_size,
                self.marker_size,
            ),
            task_scheduler: SimTaskScheduler::new(),
            retain_storage: SimRetainStorage::new(self.retain_path, self.retain_capacity),
            watchdog: SimWatchdog::new(),
            diagnostic_sink: SimDiagnosticSink::new(),
            variable_access: SimVariableAccess,
            io_driver: SimIoDriver,
            initialized: false,
        }
    }
}

impl LinuxSimulator {
    /// Create a new simulator with default buffer sizes.
    ///
    /// Defaults: 1024 bytes input, 1024 bytes output, 4096 bytes markers.
    pub fn new() -> Self {
        LinuxSimulatorBuilder::new().build()
    }

    /// Create a builder for customizing the simulator.
    pub fn builder() -> LinuxSimulatorBuilder {
        LinuxSimulatorBuilder::new()
    }

    /// Access the I/O driver (for testing).
    pub fn io_driver(&self) -> &SimIoDriver {
        &self.io_driver
    }

    /// Mutable access to the I/O driver (for testing).
    pub fn io_driver_mut(&mut self) -> &mut SimIoDriver {
        &mut self.io_driver
    }

    /// Run a single scan cycle.
    ///
    /// The `scan_fn` receives a mutable reference to the process image layout.
    /// This simulates what the compiled PLC code does each cycle.
    pub fn run_scan<F>(&mut self, scan_fn: F)
    where
        F: FnOnce(&mut SimProcessImage),
    {
        self.clock.begin_scan();
        // Read inputs from I/O driver (no-op for simulator)
        let _ = self.io_driver.read_inputs(&mut []);
        // Execute the user's scan logic
        scan_fn(&mut self.process_image);
        // Write outputs to I/O driver (no-op for simulator)
        let _ = self.io_driver.write_outputs(&[]);
    }
}

impl Default for LinuxSimulator {
    fn default() -> Self {
        Self::new()
    }
}

impl Platform for LinuxSimulator {
    type Clock = SimClock;
    type ProcessImage = SimProcessImage;
    type TaskScheduler = SimTaskScheduler;
    type RetainStorage = SimRetainStorage;
    type Watchdog = SimWatchdog;
    type DiagnosticSink = SimDiagnosticSink;
    type VariableAccess = SimVariableAccess;

    fn init(&mut self) -> Result<(), PlatformError> {
        if self.initialized {
            return Err(PlatformError::InitFailed(
                "platform already initialized".to_string(),
            ));
        }
        self.initialized = true;
        Ok(())
    }

    fn shutdown(&mut self) -> Result<(), PlatformError> {
        self.initialized = false;
        Ok(())
    }

    fn clock(&self) -> &SimClock {
        &self.clock
    }

    fn clock_mut(&mut self) -> &mut SimClock {
        &mut self.clock
    }

    fn process_image(&self) -> &SimProcessImage {
        &self.process_image
    }

    fn process_image_mut(&mut self) -> &mut SimProcessImage {
        &mut self.process_image
    }

    fn task_scheduler(&self) -> &SimTaskScheduler {
        &self.task_scheduler
    }

    fn task_scheduler_mut(&mut self) -> &mut SimTaskScheduler {
        &mut self.task_scheduler
    }

    fn retain_storage(&self) -> &SimRetainStorage {
        &self.retain_storage
    }

    fn retain_storage_mut(&mut self) -> &mut SimRetainStorage {
        &mut self.retain_storage
    }

    fn watchdog(&self) -> &SimWatchdog {
        &self.watchdog
    }

    fn watchdog_mut(&mut self) -> &mut SimWatchdog {
        &mut self.watchdog
    }

    fn diagnostic_sink(&self) -> &SimDiagnosticSink {
        &self.diagnostic_sink
    }

    fn diagnostic_sink_mut(&mut self) -> &mut SimDiagnosticSink {
        &mut self.diagnostic_sink
    }

    fn variable_access(&self) -> &SimVariableAccess {
        &self.variable_access
    }

    fn variable_access_mut(&mut self) -> &mut SimVariableAccess {
        &mut self.variable_access
    }
}
