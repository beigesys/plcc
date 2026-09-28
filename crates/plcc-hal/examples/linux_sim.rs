// SPDX-License-Identifier: MPL-2.0
//! Compile and run any ST program on the Linux simulator through the generic
//! runtime contract: the program's tasks, process image and variables are all
//! discovered from the compiled module — nothing here knows the program.
//!
//! Usage:
//!   cargo run --example linux_sim -p plcc-hal -- program.st [--scans N] [--interval-ms MS]
//!       [--input BYTE=VALUE ...]
//!
//! Examples:
//!   cargo run --example linux_sim -p plcc-hal -- tests/fixtures/programs/blink.st
//!   cargo run --example linux_sim -p plcc-hal -- tests/fixtures/programs/state_machine.st --scans 25
//!   cargo run --example linux_sim -p plcc-hal -- io.st --input 0=1 --scans 5

#[cfg(feature = "runner")]
fn main() {
    use clap::Parser;
    use plcc_hal::jit::{JitModule, JitOptions};
    use plcc_hal::platform::Platform;
    use plcc_hal::process_image::ProcessImage;
    use plcc_hal::scan::ScanCycle;
    use plcc_hal::simulator::LinuxSimulator;
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[derive(Parser)]
    #[command(name = "plcc-sim", about = "Run ST programs on the Linux simulator")]
    struct Args {
        /// Input .st file(s)
        inputs: Vec<PathBuf>,
        /// Number of scan cycles (steps of the scan loop) to run
        #[arg(long, default_value = "20")]
        scans: usize,
        /// Real time between scan cycles in milliseconds (0 = as fast as possible)
        #[arg(long, default_value = "100")]
        interval_ms: u64,
        /// Set an input byte before running: `--input 0=5` writes 5 to %IB0
        #[arg(long, value_name = "BYTE=VALUE")]
        input: Vec<String>,
        /// Do not compile the bundled standard function blocks
        #[arg(long)]
        no_stdlib: bool,
        /// Show at most this many variables per scan
        #[arg(long, default_value = "8")]
        max_vars: usize,
    }

    let args = Args::parse();
    if args.inputs.is_empty() {
        eprintln!("Usage: linux_sim <program.st> [--scans N] [--interval-ms MS]");
        std::process::exit(1);
    }

    let mut sources = Vec::new();
    for input in &args.inputs {
        let bytes = std::fs::read(input).unwrap_or_else(|e| {
            eprintln!("Failed to read {}: {e}", input.display());
            std::process::exit(1);
        });
        let text = String::from_utf8(bytes.clone())
            .unwrap_or_else(|_| bytes.iter().map(|&b| b as char).collect());
        sources.push((input.display().to_string(), text));
    }
    let refs: Vec<(&str, &str)> = sources
        .iter()
        .map(|(n, t)| (n.as_str(), t.as_str()))
        .collect();
    let opts = JitOptions {
        stdlib: !args.no_stdlib,
        // The simulated clock and the scan loop tick at --interval-ms.
        default_task_interval_ns: Some((args.interval_ms.max(1) * 1_000_000) as i64),
        ..JitOptions::default()
    };
    let module = JitModule::compile(&refs, &opts).unwrap_or_else(|e| {
        eprintln!("Compile error: {e}");
        std::process::exit(1);
    });
    let app = module.application().expect("plcc_app");
    let contract = module.contract().expect("contract");
    let [isz, qsz, msz] = contract.image_sizes;

    let mut sim = LinuxSimulator::builder()
        .input_size((isz as usize).max(1))
        .output_size((qsz as usize).max(1))
        .marker_size((msz as usize).max(1))
        .build();
    sim.init().expect("simulator init failed");
    for spec in &args.input {
        let parsed = spec
            .split_once('=')
            .and_then(|(b, v)| Some((b.trim().parse::<u32>().ok()?, v.trim().parse::<u8>().ok()?)));
        match parsed {
            Some((b, v)) if sim.process_image_mut().write_input(b, v) => {}
            _ => eprintln!("ignoring --input {spec}"),
        }
    }

    let mut cycle = ScanCycle::new(app, &mut sim).expect("tasks");
    let kind = cycle.start(&mut sim);
    println!("Image: %I {isz} B, %Q {qsz} B, %M {msz} B   ({kind:?} start)");
    for t in cycle.tasks() {
        let progs: Vec<String> = t.programs.iter().map(|(n, p)| format!("{n}:{p}")).collect();
        println!(
            "Task {} interval={}ms priority={}{} -> {}",
            t.name,
            t.interval_ns as f64 / 1e6,
            t.priority,
            if t.has_single { " SINGLE" } else { "" },
            progs.join(", ")
        );
    }

    // Instance symbol -> live state pointer, from the task table.
    let raw = cycle.app().raw();
    let mut state_of: HashMap<String, *const u8> = HashMap::new();
    for ti in 0..raw.task_count as usize {
        let t = unsafe { &*raw.tasks.add(ti) };
        for pi in 0..t.program_count as usize {
            let p = unsafe { &*t.programs.add(pi) };
            let name = unsafe { std::ffi::CStr::from_ptr(p.name) }.to_string_lossy();
            if let Some(i) = contract.instances.iter().find(|i| i.name == name) {
                state_of.insert(i.symbol.clone(), p.state);
            }
        }
    }
    let shown: Vec<_> = contract
        .variables
        .iter()
        .filter(|v| state_of.contains_key(&v.symbol))
        .take(args.max_vars)
        .collect();
    let show = |v: &plcc_codegen::compiler::contract::VariableInfo| -> String {
        let p = unsafe { state_of[&v.symbol].add(v.offset as usize) };
        let b = unsafe { std::slice::from_raw_parts(p, v.size as usize) };
        let val = match (v.iec_type.as_str(), b.len()) {
            ("BOOL", 1) => (b[0] != 0).to_string().to_uppercase(),
            ("REAL", 4) => f32::from_ne_bytes(b.try_into().unwrap()).to_string(),
            ("LREAL", 8) => f64::from_ne_bytes(b.try_into().unwrap()).to_string(),
            ("TIME" | "LTIME", 8) => {
                format!("{}ms", i64::from_ne_bytes(b.try_into().unwrap()) / 1_000_000)
            }
            ("SINT", 1) => (b[0] as i8).to_string(),
            ("INT", 2) => i16::from_ne_bytes(b.try_into().unwrap()).to_string(),
            ("DINT", 4) => i32::from_ne_bytes(b.try_into().unwrap()).to_string(),
            ("LINT", 8) => i64::from_ne_bytes(b.try_into().unwrap()).to_string(),
            (_, 1) => b[0].to_string(),
            (_, 2) => u16::from_ne_bytes(b.try_into().unwrap()).to_string(),
            (_, 4) => u32::from_ne_bytes(b.try_into().unwrap()).to_string(),
            (_, 8) => u64::from_ne_bytes(b.try_into().unwrap()).to_string(),
            _ => "…".into(),
        };
        format!("{}={val}", v.path)
    };

    let start = std::time::Instant::now();
    for scan in 0..args.scans {
        let now = if args.interval_ms == 0 {
            scan as u64 * 1_000_000
        } else {
            start.elapsed().as_nanos() as u64
        };
        let report = cycle.step_at(&mut sim, now).expect("scan");
        let q: Vec<String> = (0..qsz)
            .map(|i| format!("{:02x}", sim.process_image().read_output(i).unwrap_or(0)))
            .collect();
        let vars: Vec<String> = shown.iter().map(|v| show(v)).collect();
        println!(
            "scan {scan:>4}: ran {:?} %Q=[{}] {}",
            report.ran,
            q.join(" "),
            vars.join(" ")
        );
        if args.interval_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(args.interval_ms));
        }
    }
    cycle.save_retain(&mut sim).ok();
    sim.shutdown().expect("simulator shutdown failed");
}

#[cfg(not(feature = "runner"))]
fn main() {
    eprintln!("This example requires the 'runner' feature. Build with:");
    eprintln!("  cargo run --example linux_sim -p plcc-hal --features runner -- <file.st>");
}
