// SPDX-License-Identifier: MPL-2.0
//! Integration tests for the Linux simulator.

use plcc_hal::clock::Clock;
use plcc_hal::diagnostics::{DiagCode, DiagLevel, DiagnosticSink};
use plcc_hal::platform::Platform;
use plcc_hal::process_image::ProcessImage;
use plcc_hal::retain::RetainStorage;
use plcc_hal::simulator::LinuxSimulator;

#[test]
fn process_image_write_read_output() {
    let mut sim = LinuxSimulator::builder()
        .input_size(16)
        .output_size(16)
        .marker_size(16)
        .build();

    // Write to output, read back
    assert!(sim.process_image_mut().write_output(0, 0xAB));
    assert_eq!(sim.process_image().read_output(0), Some(0xAB));

    // Out of bounds
    assert!(!sim.process_image_mut().write_output(16, 0xFF));
    assert_eq!(sim.process_image().read_output(16), None);
}

#[test]
fn process_image_write_read_input() {
    let mut sim = LinuxSimulator::builder()
        .input_size(8)
        .output_size(8)
        .marker_size(8)
        .build();

    assert!(sim.process_image_mut().write_input(3, 0x42));
    assert_eq!(sim.process_image().read_input(3), Some(0x42));
    assert_eq!(sim.process_image().read_input(0), Some(0x00));
}

#[test]
fn process_image_write_read_marker() {
    let mut sim = LinuxSimulator::builder()
        .input_size(4)
        .output_size(4)
        .marker_size(32)
        .build();

    assert!(sim.process_image_mut().write_marker(31, 0xFF));
    assert_eq!(sim.process_image().read_marker(31), Some(0xFF));
    assert!(!sim.process_image_mut().write_marker(32, 0x01));
}

#[test]
fn process_image_layout_pointers() {
    let sim = LinuxSimulator::builder()
        .input_size(128)
        .output_size(64)
        .marker_size(256)
        .build();

    let layout = sim.process_image().layout();
    assert_eq!(layout.input_size, 128);
    assert_eq!(layout.output_size, 64);
    assert_eq!(layout.marker_size, 256);
    assert!(!layout.input_ptr.is_null());
    assert!(!layout.output_ptr.is_null());
    assert!(!layout.marker_ptr.is_null());
}

#[test]
fn clock_elapsed_time_increases() {
    let mut sim = LinuxSimulator::new();

    let t0 = sim.clock().now_ns();
    // Small busy-wait to ensure time passes
    std::thread::sleep(std::time::Duration::from_millis(5));
    let t1 = sim.clock().now_ns();
    assert!(t1 > t0, "monotonic clock must increase over time");

    // begin_scan then check elapsed
    sim.clock_mut().begin_scan();
    std::thread::sleep(std::time::Duration::from_millis(5));
    let elapsed = sim.clock().elapsed_since_last_scan_ns();
    assert!(
        elapsed >= 1_000_000,
        "elapsed should be at least 1ms, got {elapsed}ns"
    );
}

#[test]
fn clock_elapsed_zero_before_begin_scan() {
    let sim = LinuxSimulator::new();
    assert_eq!(sim.clock().elapsed_since_last_scan_ns(), 0);
}

#[test]
fn retain_storage_save_restore_roundtrip() {
    let dir = std::env::temp_dir().join("plcc_test_retain");
    let _ = std::fs::create_dir_all(&dir);
    let retain_path = dir.join("test_retain.bin");

    // Clean up from previous runs
    let _ = std::fs::remove_file(&retain_path);

    let mut sim = LinuxSimulator::builder()
        .retain_path(retain_path.clone())
        .retain_capacity(1024)
        .build();

    let data = b"hello retain world";
    sim.retain_storage_mut()
        .save(data)
        .expect("save should succeed");

    let restored = sim
        .retain_storage()
        .restore()
        .expect("restore should succeed");
    assert_eq!(restored, data);

    // Clean up
    sim.retain_storage_mut()
        .clear()
        .expect("clear should succeed");
    assert!(
        sim.retain_storage().restore().is_err(),
        "restore after clear should fail"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn retain_storage_rejects_oversized_data() {
    let dir = std::env::temp_dir().join("plcc_test_retain_size");
    let _ = std::fs::create_dir_all(&dir);
    let retain_path = dir.join("test_retain_size.bin");

    let mut sim = LinuxSimulator::builder()
        .retain_path(retain_path)
        .retain_capacity(16)
        .build();

    let big_data = vec![0xAA; 32];
    let result = sim.retain_storage_mut().save(&big_data);
    assert!(
        result.is_err(),
        "save should reject data exceeding capacity"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn diagnostic_sink_report_and_count() {
    let mut sim = LinuxSimulator::new();

    assert_eq!(sim.diagnostic_sink().error_count(), 0);

    sim.diagnostic_sink_mut()
        .report(DiagLevel::Info, DiagCode::None, "informational");
    assert_eq!(sim.diagnostic_sink().error_count(), 0);

    sim.diagnostic_sink_mut().report(
        DiagLevel::Warning,
        DiagCode::ScanOverrun,
        "scan took too long",
    );
    assert_eq!(sim.diagnostic_sink().error_count(), 0);

    sim.diagnostic_sink_mut().report(
        DiagLevel::Error,
        DiagCode::DivisionByZero,
        "div/0 at line 42",
    );
    assert_eq!(sim.diagnostic_sink().error_count(), 1);

    sim.diagnostic_sink_mut()
        .report(DiagLevel::Fatal, DiagCode::StackOverflow, "stack overflow");
    assert_eq!(sim.diagnostic_sink().error_count(), 2);

    sim.diagnostic_sink_mut().clear_errors();
    assert_eq!(sim.diagnostic_sink().error_count(), 0);
}

#[test]
fn full_scan_cycle() {
    let mut sim = LinuxSimulator::builder()
        .input_size(8)
        .output_size(8)
        .marker_size(8)
        .build();

    sim.init().expect("init failed");

    // Simulate: input byte 0 = 0x55, scan copies input[0] to output[0]
    sim.process_image_mut().write_input(0, 0x55);

    sim.run_scan(|pi| {
        let input_val = pi.read_input(0).unwrap_or(0);
        pi.write_output(0, input_val);
    });

    assert_eq!(
        sim.process_image().read_output(0),
        Some(0x55),
        "output should reflect input after scan"
    );

    sim.shutdown().expect("shutdown failed");
}

#[test]
fn platform_double_init_fails() {
    let mut sim = LinuxSimulator::new();
    sim.init().expect("first init should succeed");
    assert!(sim.init().is_err(), "second init should fail");
    sim.shutdown().expect("shutdown should succeed");
}
