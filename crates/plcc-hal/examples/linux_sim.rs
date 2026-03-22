// SPDX-License-Identifier: MPL-2.0
//! Compile and run any ST program on the Linux simulator.
//!
//! Usage:
//!   cargo run --example linux_sim -p plcc-hal -- program.st [--scans N] [--interval-ms MS]
//!
//! Examples:
//!   cargo run --example linux_sim -p plcc-hal -- tests/fixtures/programs/blink.st
//!   cargo run --example linux_sim -p plcc-hal -- tests/fixtures/programs/state_machine.st --scans 25
//!   cargo run --example linux_sim -p plcc-hal -- tests/fixtures/programs/pid_simple.st --scans 50 --interval-ms 10

#[cfg(feature = "runner")]
fn main() {
    use clap::Parser;
    use inkwell::context::Context;
    use inkwell::OptimizationLevel;
    use std::path::PathBuf;
    use plcc_hal::platform::Platform;
    use plcc_hal::process_image::ProcessImage;
    use plcc_hal::simulator::LinuxSimulator;

    #[derive(Parser)]
    #[command(name = "plcc-sim", about = "Run ST programs on the Linux simulator")]
    struct Args {
        /// Input .st file(s)
        inputs: Vec<PathBuf>,
        /// Number of scan cycles to run
        #[arg(long, default_value = "20")]
        scans: usize,
        /// Interval between scans in milliseconds (0 = no delay)
        #[arg(long, default_value = "100")]
        interval_ms: u64,
        /// Dump LLVM IR instead of running
        #[arg(long)]
        dump_ir: bool,
    }

    let args = Args::parse();

    if args.inputs.is_empty() {
        eprintln!("Usage: linux_sim <program.st> [--scans N] [--interval-ms MS]");
        std::process::exit(1);
    }

    // Parse all input files
    let mut all_declarations = Vec::new();
    for input in &args.inputs {
        let bytes = std::fs::read(input).unwrap_or_else(|e| {
            eprintln!("Failed to read {}: {e}", input.display());
            std::process::exit(1);
        });
        let source = match String::from_utf8(bytes.clone()) {
            Ok(s) => s,
            Err(_) => bytes.iter().map(|&b| b as char).collect(),
        };
        let (unit, errors) = plcc_st::parse(&source);
        if !errors.is_empty() {
            for err in &errors {
                eprintln!("Parse error in {}: {err}", input.display());
            }
            std::process::exit(1);
        }
        all_declarations.extend(unit.declarations);
    }

    let merged = plcc_st::CompilationUnit {
        declarations: all_declarations,
        span: plcc_st::Span::empty(),
    };

    // Compile
    let context = Context::create();
    let mut compiler = plcc_codegen::Compiler::new(&context, "sim");
    if let Err(e) = compiler.compile(&merged) {
        eprintln!("Codegen error: {e}");
        std::process::exit(1);
    }

    if args.dump_ir {
        println!("{}", compiler.emit_ir());
        return;
    }

    // Find all program scan functions
    let ir = compiler.emit_ir();
    let program_names: Vec<String> = ir
        .lines()
        .filter_map(|line| {
            if line.starts_with("define void @") && line.contains("_scan(") {
                let name = line
                    .trim_start_matches("define void @")
                    .split('(')
                    .next()
                    .unwrap_or("")
                    .to_string();
                if name.ends_with("_scan") {
                    Some(name)
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();

    if program_names.is_empty() {
        eprintln!("No PROGRAM declarations found in the source.");
        std::process::exit(1);
    }

    // JIT execute
    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .unwrap_or_else(|e| {
            eprintln!("Failed to create JIT: {e}");
            std::process::exit(1);
        });

    // For each program, allocate state and run
    for scan_name in &program_names {
        let init_name = scan_name.replace("_scan", "_init");
        let display_name = scan_name.trim_end_matches("_scan");

        // Allocate generous state buffer (4KB should cover most programs)
        let state_size = 4096;
        let mut state = vec![0u8; state_size];
        let state_ptr = state.as_mut_ptr();

        // Call init
        if let Ok(init_ptr) = ee.get_function_address(&init_name) {
            let init: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(init_ptr) };
            init(state_ptr);
        }

        let scan_ptr = match ee.get_function_address(scan_name) {
            Ok(p) => p,
            Err(_) => {
                eprintln!("Function {scan_name} not found");
                continue;
            }
        };
        let scan: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(scan_ptr) };

        println!("Running {display_name} for {} scans...\n", args.scans);

        let mut sim = LinuxSimulator::builder()
            .input_size(64)
            .output_size(64)
            .marker_size(256)
            .build();
        sim.init().expect("simulator init failed");

        for cycle in 0..args.scans {
            scan(state_ptr);

            // Print first few state bytes as hex + interpreted values
            let preview_bytes = 20.min(state_size);
            let hex: String = state[..preview_bytes]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(" ");

            // Try to interpret first few fields as common types
            let mut fields = String::new();
            if state_size >= 2 {
                let v = i16::from_ne_bytes([state[0], state[1]]);
                fields.push_str(&format!(" [0]i16={v}"));
            }
            if state_size >= 4 {
                let v = i16::from_ne_bytes([state[2], state[3]]);
                fields.push_str(&format!(" [2]i16={v}"));
            }
            if state_size >= 6 {
                let v = i16::from_ne_bytes([state[4], state[5]]);
                fields.push_str(&format!(" [4]i16={v}"));
            }

            println!("Scan {cycle:>4}: {hex} ...{fields}");

            if args.interval_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(args.interval_ms));
            }
        }

        sim.shutdown().expect("simulator shutdown failed");
        println!("\n{display_name} complete.\n");
    }
}

#[cfg(not(feature = "runner"))]
fn main() {
    eprintln!("This example requires the 'runner' feature. Build with:");
    eprintln!("  cargo run --example linux_sim -p plcc-hal --features runner -- <file.st>");
}
