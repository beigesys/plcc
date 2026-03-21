// SPDX-License-Identifier: MPL-2.0
//! Example: blink simulation using the Linux simulator.
//!
//! Creates a LinuxSimulator, allocates a simple blink program state,
//! and runs scan cycles that toggle an output bit each cycle.

use plcc_hal::platform::Platform;
use plcc_hal::process_image::ProcessImage;
use plcc_hal::simulator::LinuxSimulator;

fn main() {
    let mut sim = LinuxSimulator::builder()
        .input_size(64)
        .output_size(64)
        .marker_size(256)
        .build();

    sim.init().expect("failed to initialize simulator");

    // Simulate a blink program: toggle output byte 0, bit 0 each scan cycle.
    // We use marker byte 0 as our "state" (the blink FB's internal BOOL).
    println!("Running blink simulation for 10 cycles...\n");

    for cycle in 0..10 {
        sim.run_scan(|pi| {
            // Read current state from marker
            let state = pi.read_marker(0).unwrap_or(0);
            // Toggle
            let new_state = if state == 0 { 1 } else { 0 };
            // Write new state back to marker
            pi.write_marker(0, new_state);
            // Write output Q0.0
            pi.write_output(0, new_state);
        });

        let output_val = sim.process_image().read_output(0).unwrap_or(0);
        let state = if output_val != 0 { "ON" } else { "OFF" };
        println!("Cycle {cycle:>2}: Q0.0 = {state}");

        // Sleep to simulate a 100ms scan cycle
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    sim.shutdown().expect("failed to shut down simulator");
    println!("\nSimulation complete.");
}
