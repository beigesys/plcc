// SPDX-License-Identifier: MPL-2.0

use clap::{Parser, Subcommand};
use miette::{IntoDiagnostic, NamedSource, Result};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "plcc", about = "IEC 61131-3 Structured Text compiler")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Parse a Structured Text file and optionally dump the AST
    Parse {
        /// Input .st file
        input: PathBuf,
        /// Dump AST as JSON
        #[arg(long)]
        dump_ast: bool,
    },
    /// Parse and type-check a Structured Text file
    Check {
        /// Input .st file
        input: PathBuf,
    },
    /// Compile one or more Structured Text files
    Compile {
        /// Input .st file(s)
        inputs: Vec<PathBuf>,
        /// Output file
        #[arg(short, long)]
        output: PathBuf,
        /// Target triple (e.g. x86_64-unknown-linux-gnu, wasm32-unknown-unknown)
        #[arg(long, default_value = "x86_64-unknown-linux-gnu")]
        target: String,
    },
    /// Simulate: compile and run ST programs with JIT execution
    Sim {
        /// Input .st file(s)
        inputs: Vec<PathBuf>,
        /// Number of scan cycles (0 = run forever)
        #[arg(long, default_value = "20")]
        scans: usize,
        /// Interval between scans in milliseconds (0 = no delay)
        #[arg(long, default_value = "0")]
        interval_ms: u64,
        /// Start Modbus TCP server on this port (e.g. 502 for FUXA/SCADA)
        #[arg(long)]
        modbus: Option<u16>,
    },
}

/// Read source file, falling back to Latin-1 if not valid UTF-8.
fn read_source(path: &std::path::Path) -> Result<String> {
    let bytes = std::fs::read(path).into_diagnostic()?;
    match String::from_utf8(bytes.clone()) {
        Ok(s) => Ok(s),
        Err(_) => Ok(bytes.iter().map(|&b| b as char).collect()),
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Parse { input, dump_ast } => {
            let source = read_source(&input)?;
            let (unit, errors) = plcc_st::parse(&source);

            if !errors.is_empty() {
                let file_name = input.display().to_string();
                for err in &errors {
                    let report = miette::Report::new(err.clone())
                        .with_source_code(NamedSource::new(&file_name, source.clone()));
                    eprintln!("{:?}", report);
                }
                eprintln!("{} error(s)", errors.len());
            }

            if dump_ast {
                let json = serde_json::to_string_pretty(&unit).into_diagnostic()?;
                println!("{json}");
            } else if errors.is_empty() {
                println!(
                    "OK: {} declaration(s) parsed",
                    unit.declarations.len()
                );
            }

            if errors.is_empty() {
                Ok(())
            } else {
                std::process::exit(1);
            }
        }
        Commands::Check { input } => {
            let source = read_source(&input)?;
            let (unit, parse_errors) = plcc_st::parse(&source);
            let file_name = input.display().to_string();

            if !parse_errors.is_empty() {
                for err in &parse_errors {
                    let report = miette::Report::new(err.clone())
                        .with_source_code(NamedSource::new(&file_name, source.clone()));
                    eprintln!("{:?}", report);
                }
                std::process::exit(1);
            }

            let (_symbols, check_errors) = plcc_hir::check(&unit);

            if !check_errors.is_empty() {
                for err in &check_errors {
                    let report = miette::Report::new(err.clone())
                        .with_source_code(NamedSource::new(&file_name, source.clone()));
                    eprintln!("{:?}", report);
                }
                eprintln!("{} type error(s)", check_errors.len());
                std::process::exit(1);
            }

            println!("OK: {} declaration(s) checked", unit.declarations.len());
            Ok(())
        }
        Commands::Compile {
            inputs,
            output,
            target,
        } => {
            if inputs.is_empty() {
                eprintln!("Error: at least one input file is required");
                std::process::exit(1);
            }

            // Parse all input files and merge declarations
            let mut all_declarations = Vec::new();
            let mut had_errors = false;

            for input in &inputs {
                let source = read_source(input)?;
                let (unit, parse_errors) = plcc_st::parse(&source);

                if !parse_errors.is_empty() {
                    let file_name = input.display().to_string();
                    for err in &parse_errors {
                        let report = miette::Report::new(err.clone())
                            .with_source_code(NamedSource::new(&file_name, source.clone()));
                        eprintln!("{:?}", report);
                    }
                    had_errors = true;
                }

                all_declarations.extend(unit.declarations);
            }

            if had_errors {
                std::process::exit(1);
            }

            let merged = plcc_st::ast::CompilationUnit {
                declarations: all_declarations,
                span: plcc_st::span::Span::empty(),
            };

            let module_name = inputs[0].display().to_string();
            let context = inkwell::context::Context::create();
            let mut compiler = plcc_codegen::Compiler::new(&context, &module_name);

            if let Err(e) = compiler.compile(&merged) {
                eprintln!("Codegen error: {e}");
                std::process::exit(1);
            }

            let out_str = output.display().to_string();
            if out_str.ends_with(".ll") {
                std::fs::write(&output, compiler.emit_ir()).into_diagnostic()?;
            } else if out_str.ends_with(".bc") {
                compiler.emit_bitcode(&output);
            } else {
                compiler
                    .emit_object(&output, &target)
                    .map_err(|e| miette::miette!("{e}"))?;
            }

            println!("Compiled {} file(s) to {}", inputs.len(), output.display());
            Ok(())
        }
        Commands::Sim {
            inputs,
            scans,
            interval_ms,
            modbus,
        } => {
            if inputs.is_empty() {
                eprintln!("Usage: plcc sim <program.st> [--scans N] [--interval-ms MS] [--modbus PORT]");
                std::process::exit(1);
            }

            // Parse all inputs
            let mut all_declarations = Vec::new();
            for input in &inputs {
                let source = read_source(input)?;
                let (unit, errors) = plcc_st::parse(&source);
                if !errors.is_empty() {
                    let file_name = input.display().to_string();
                    for err in &errors {
                        let report = miette::Report::new(err.clone())
                            .with_source_code(NamedSource::new(&file_name, source.clone()));
                        eprintln!("{:?}", report);
                    }
                    std::process::exit(1);
                }
                all_declarations.extend(unit.declarations);
            }

            let merged = plcc_st::ast::CompilationUnit {
                declarations: all_declarations,
                span: plcc_st::span::Span::empty(),
            };

            // Compile
            let context = inkwell::context::Context::create();
            let mut compiler = plcc_codegen::Compiler::new(&context, "sim");
            if let Err(e) = compiler.compile(&merged) {
                eprintln!("Codegen error: {e}");
                std::process::exit(1);
            }

            // Find the LAST program scan function (the main program, not FBs)
            let ir = compiler.emit_ir();
            let scan_fns: Vec<String> = ir
                .lines()
                .filter_map(|line| {
                    if line.starts_with("define void @") && line.contains("_scan(") {
                        line.trim_start_matches("define void @")
                            .split('(')
                            .next()
                            .filter(|n| n.ends_with("_scan"))
                            .map(|n| n.to_string())
                    } else {
                        None
                    }
                })
                .collect();

            if scan_fns.is_empty() {
                eprintln!("No PROGRAM declarations found.");
                std::process::exit(1);
            }

            // Use the last scan function (main program)
            let scan_name = scan_fns.last().unwrap();
            let init_name = scan_name.replace("_scan", "_init");
            let prog_name = scan_name.trim_end_matches("_scan").to_string();

            // JIT execute
            let ee = compiler
                .module()
                .create_jit_execution_engine(inkwell::OptimizationLevel::None)
                .map_err(|e| miette::miette!("JIT error: {e}"))?;

            let state_size = 4096;
            let state = std::sync::Arc::new(std::sync::Mutex::new(vec![0u8; state_size]));

            // Init
            {
                let mut s = state.lock().unwrap();
                if let Ok(init_ptr) = ee.get_function_address(&init_name) {
                    let init: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(init_ptr) };
                    init(s.as_mut_ptr());
                }
            }

            let scan_ptr = ee
                .get_function_address(scan_name)
                .map_err(|_| miette::miette!("Function {scan_name} not found"))?;
            let scan_fn: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(scan_ptr) };

            // Start Modbus TCP server if requested
            if let Some(port) = modbus {
                let mb_state = state.clone();
                std::thread::spawn(move || {
                    let rt = tokio::runtime::Runtime::new().unwrap();
                    rt.block_on(async move {
                        run_modbus_tcp_server(port, mb_state).await;
                    });
                });
                eprintln!("Modbus TCP server on port {port}");
                eprintln!("Connect FUXA SCADA to localhost:{port}\n");
            }

            let run_forever = scans == 0;
            if run_forever {
                eprintln!("Running {prog_name} continuously (Ctrl+C to stop)...\n");
            } else {
                eprintln!("Running {prog_name} for {scans} scans...\n");
            }

            let scan_interval = if interval_ms > 0 {
                std::time::Duration::from_millis(interval_ms)
            } else {
                std::time::Duration::from_millis(10) // default 10ms = 100Hz
            };

            let mut cycle: u64 = 0;
            loop {
                {
                    let mut s = state.lock().unwrap();
                    scan_fn(s.as_mut_ptr());
                }

                if cycle % 100 == 0 {
                    let s = state.lock().unwrap();
                    let v0 = i16::from_ne_bytes([s[0], s[1]]);
                    let v1 = i16::from_ne_bytes([s[2], s[3]]);
                    let v2 = i16::from_ne_bytes([s[4], s[5]]);
                    println!("scan {cycle:>6} | mode={v0} cycle={v1} [{v2}, ...]");
                }

                cycle += 1;
                if !run_forever && cycle >= scans as u64 {
                    break;
                }

                std::thread::sleep(scan_interval);
            }

            eprintln!("\n{prog_name} done.");
            Ok(())
        }
    }
}

/// Modbus TCP server — serves PLC state as Modbus holding registers.
/// MBAP header (7 bytes) + Modbus PDU. Registers map to i16 values
/// at 2-byte offsets in the state buffer.
async fn run_modbus_tcp_server(port: u16, state: std::sync::Arc<std::sync::Mutex<Vec<u8>>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let addr = format!("0.0.0.0:{port}");
    let listener = TcpListener::bind(&addr).await.unwrap_or_else(|e| {
        eprintln!("Modbus TCP: failed to bind {addr}: {e}");
        std::process::exit(1);
    });
    eprintln!("Modbus TCP listening on {addr}");

    loop {
        let (mut socket, peer) = match listener.accept().await {
            Ok(s) => s,
            Err(_) => continue,
        };
        eprintln!("Modbus TCP: client connected from {peer}");
        let st = state.clone();

        tokio::spawn(async move {
            let mut buf = [0u8; 512];
            loop {
                let n = match socket.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(_) => break,
                };
                if n < 12 { continue; } // MBAP(7) + unit(1) + FC(1) + data(min 3)

                // MBAP header
                let tx_id = u16::from_be_bytes([buf[0], buf[1]]);
                let _proto = u16::from_be_bytes([buf[2], buf[3]]); // should be 0
                let _length = u16::from_be_bytes([buf[4], buf[5]]);
                let unit_id = buf[6];
                let fc = buf[7];

                match fc {
                    0x03 => {
                        // Read Holding Registers
                        let start = u16::from_be_bytes([buf[8], buf[9]]) as usize;
                        let count = u16::from_be_bytes([buf[10], buf[11]]) as usize;
                        let byte_count = (count * 2) as u8;

                        let resp = {
                            let mut r = Vec::with_capacity(9 + count * 2);
                            r.extend_from_slice(&tx_id.to_be_bytes());
                            r.extend_from_slice(&0u16.to_be_bytes());
                            r.extend_from_slice(&((3 + count * 2) as u16).to_be_bytes());
                            r.push(unit_id);
                            r.push(0x03);
                            r.push(byte_count);
                            let s = st.lock().unwrap();
                            for i in 0..count {
                                let offset = (start + i) * 2;
                                if offset + 1 < s.len() {
                                    let val = i16::from_ne_bytes([s[offset], s[offset + 1]]);
                                    r.extend_from_slice(&(val as u16).to_be_bytes());
                                } else {
                                    r.extend_from_slice(&0u16.to_be_bytes());
                                }
                            }
                            r
                        };
                        let _ = socket.write_all(&resp).await;
                    }
                    0x06 => {
                        // Write Single Register
                        let addr = u16::from_be_bytes([buf[8], buf[9]]) as usize;
                        let value = u16::from_be_bytes([buf[10], buf[11]]);
                        let offset = addr * 2;

                        {
                            let mut s = st.lock().unwrap();
                            if offset + 1 < s.len() {
                                let bytes = (value as i16).to_ne_bytes();
                                s[offset] = bytes[0];
                                s[offset + 1] = bytes[1];
                            }
                        }

                        // Echo request as response
                        let mut resp = Vec::with_capacity(12);
                        resp.extend_from_slice(&tx_id.to_be_bytes());
                        resp.extend_from_slice(&0u16.to_be_bytes());
                        resp.extend_from_slice(&6u16.to_be_bytes());
                        resp.extend_from_slice(&buf[6..12]);
                        let _ = socket.write_all(&resp).await;
                    }
                    0x10 => {
                        // Write Multiple Registers
                        let start = u16::from_be_bytes([buf[8], buf[9]]) as usize;
                        let count = u16::from_be_bytes([buf[10], buf[11]]) as usize;
                        let _byte_count = buf[12];

                        {
                            let mut s = st.lock().unwrap();
                            for i in 0..count {
                                let offset = (start + i) * 2;
                                let val = u16::from_be_bytes([buf[13 + i*2], buf[14 + i*2]]);
                                if offset + 1 < s.len() {
                                    let bytes = (val as i16).to_ne_bytes();
                                    s[offset] = bytes[0];
                                    s[offset + 1] = bytes[1];
                                }
                            }
                        }

                        // Response: echo address and count
                        let mut resp = Vec::with_capacity(12);
                        resp.extend_from_slice(&tx_id.to_be_bytes());
                        resp.extend_from_slice(&0u16.to_be_bytes());
                        resp.extend_from_slice(&6u16.to_be_bytes());
                        resp.push(unit_id);
                        resp.push(0x10);
                        resp.extend_from_slice(&buf[8..12]);
                        let _ = socket.write_all(&resp).await;
                    }
                    _ => {
                        // Exception: illegal function
                        let mut resp = Vec::with_capacity(9);
                        resp.extend_from_slice(&tx_id.to_be_bytes());
                        resp.extend_from_slice(&0u16.to_be_bytes());
                        resp.extend_from_slice(&3u16.to_be_bytes());
                        resp.push(unit_id);
                        resp.push(fc | 0x80);
                        resp.push(0x01);
                        let _ = socket.write_all(&resp).await;
                    }
                }
            }
            eprintln!("Modbus TCP: client {peer} disconnected");
        });
    }
}
