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
    /// Compile and JIT-run ST programs, optionally with Modbus TCP for SCADA
    Sim {
        /// Input .st file(s)
        inputs: Vec<PathBuf>,
        /// Number of scan cycles (0 = run forever)
        #[arg(long, default_value = "20")]
        scans: usize,
        /// Interval between scans in milliseconds
        #[arg(long, default_value = "10")]
        interval_ms: u64,
        /// Modbus TCP port for SCADA (e.g. 502)
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

fn parse_inputs(inputs: &[PathBuf]) -> Result<plcc_st::ast::CompilationUnit> {
    let mut all_declarations = Vec::new();
    for input in inputs {
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
    Ok(plcc_st::ast::CompilationUnit {
        declarations: all_declarations,
        span: plcc_st::span::Span::empty(),
    })
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
                println!("OK: {} declaration(s) parsed", unit.declarations.len());
            }

            if errors.is_empty() { Ok(()) } else { std::process::exit(1); }
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
                std::process::exit(1);
            }

            println!("OK: {} declaration(s) checked", unit.declarations.len());
            Ok(())
        }
        Commands::Compile { inputs, output, target } => {
            if inputs.is_empty() {
                eprintln!("Error: at least one input file is required");
                std::process::exit(1);
            }

            let merged = parse_inputs(&inputs)?;
            let context = inkwell::context::Context::create();
            let mut compiler = plcc_codegen::Compiler::new(&context, &inputs[0].display().to_string());

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
                compiler.emit_object(&output, &target).map_err(|e| miette::miette!("{e}"))?;
            }

            println!("Compiled {} file(s) to {}", inputs.len(), output.display());
            Ok(())
        }
        Commands::Sim { inputs, scans, interval_ms, modbus } => {
            if inputs.is_empty() {
                eprintln!("Usage: plcc sim <program.st> [--scans 0] [--modbus 502]");
                std::process::exit(1);
            }

            let merged = parse_inputs(&inputs)?;
            let context = inkwell::context::Context::create();
            let mut compiler = plcc_codegen::Compiler::new(&context, "sim");
            if let Err(e) = compiler.compile(&merged) {
                eprintln!("Codegen error: {e}");
                std::process::exit(1);
            }

            let ir = compiler.emit_ir();
            let scan_fns: Vec<String> = ir.lines()
                .filter_map(|line| {
                    if line.starts_with("define void @") && line.contains("_scan(") {
                        line.trim_start_matches("define void @")
                            .split('(').next()
                            .filter(|n| n.ends_with("_scan"))
                            .map(|n| n.to_string())
                    } else { None }
                })
                .collect();

            if scan_fns.is_empty() {
                eprintln!("No PROGRAM declarations found.");
                std::process::exit(1);
            }

            let scan_name = scan_fns.last().unwrap();
            let init_name = scan_name.replace("_scan", "_init");
            let prog_name = scan_name.trim_end_matches("_scan").to_string();

            let ee = compiler.module()
                .create_jit_execution_engine(inkwell::OptimizationLevel::None)
                .map_err(|e| miette::miette!("JIT error: {e}"))?;

            // Provide plcc_print for JIT (PRINT statement calls this)
            ee.add_global_mapping(
                &compiler.module().get_function("plcc_print").unwrap_or_else(|| {
                    let fn_type = compiler.module().get_context().void_type()
                        .fn_type(&[compiler.module().get_context().ptr_type(inkwell::AddressSpace::default()).into()], false);
                    compiler.module().add_function("plcc_print", fn_type, None)
                }),
                plcc_print_impl as usize,
            );

            let state = std::sync::Arc::new(std::sync::Mutex::new(vec![0u8; 4096]));

            // Init
            {
                let mut s = state.lock().unwrap();
                if let Ok(init_ptr) = ee.get_function_address(&init_name) {
                    let init: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(init_ptr) };
                    init(s.as_mut_ptr());
                }
            }

            let scan_ptr = ee.get_function_address(scan_name)
                .map_err(|_| miette::miette!("Function {scan_name} not found"))?;
            let scan_fn: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(scan_ptr) };

            // Start Modbus TCP server if requested
            if let Some(port) = modbus {
                let mb_state = state.clone();
                std::thread::spawn(move || modbus_tcp_server(port, mb_state));
                eprintln!("Modbus TCP on port {port}");
            }

            let run_forever = scans == 0;
            if run_forever {
                eprintln!("Running {prog_name} (Ctrl+C to stop)...\n");
            } else {
                eprintln!("Running {prog_name} for {scans} scans...\n");
            }

            let interval = std::time::Duration::from_millis(interval_ms);
            let mut cycle: u64 = 0;

            loop {
                {
                    let mut s = state.lock().unwrap();
                    scan_fn(s.as_mut_ptr());
                }

                if cycle % 100 == 0 {
                    let s = state.lock().unwrap();
                    // Show useful status: R10=running R15=raw_lvl R16=clean_lvl R14=cycle
                    let rd = |off: usize| -> i16 {
                        if off * 2 + 1 < s.len() {
                            i16::from_ne_bytes([s[off * 2], s[off * 2 + 1]])
                        } else { 0 }
                    };
                    println!("scan {cycle:>6} | run={} raw={}% clean={}% cycle={}",
                        rd(10), rd(15), rd(16), rd(14));
                }

                cycle += 1;
                if !run_forever && cycle >= scans as u64 { break; }
                if interval_ms > 0 { std::thread::sleep(interval); }
            }

            eprintln!("\n{prog_name} done.");
            Ok(())
        }
    }
}

/// PRINT implementation for JIT — outputs to stderr
extern "C" fn plcc_print_impl(msg: *const u8) {
    if msg.is_null() { return; }
    let cstr = unsafe { std::ffi::CStr::from_ptr(msg as *const std::ffi::c_char) };
    if let Ok(s) = cstr.to_str() {
        eprintln!("[PLC] {s}");
    }
}

/// Modbus TCP server using std::net. Registers map to i16 at byte_offset = reg * 2.
fn modbus_tcp_server(port: u16, state: std::sync::Arc<std::sync::Mutex<Vec<u8>>>) {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind(format!("0.0.0.0:{port}")).unwrap_or_else(|e| {
        eprintln!("Modbus TCP: bind failed: {e}");
        std::process::exit(1);
    });

    for stream in listener.incoming() {
        let mut sock = match stream {
            Ok(s) => s,
            Err(_) => continue,
        };
        let st = state.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 512];
            loop {
                let n = match sock.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                if n < 12 { continue; }

                let tx_id = [buf[0], buf[1]];
                let unit_id = buf[6];
                let fc = buf[7];

                match fc {
                    0x03 => { // Read Holding Registers
                        let start = u16::from_be_bytes([buf[8], buf[9]]) as usize;
                        let count = u16::from_be_bytes([buf[10], buf[11]]) as usize;
                        let mut resp = Vec::with_capacity(9 + count * 2);
                        resp.extend_from_slice(&tx_id);
                        resp.extend_from_slice(&0u16.to_be_bytes());
                        resp.extend_from_slice(&((3 + count * 2) as u16).to_be_bytes());
                        resp.push(unit_id);
                        resp.push(0x03);
                        resp.push((count * 2) as u8);
                        let s = st.lock().unwrap();
                        for i in 0..count {
                            let off = (start + i) * 2;
                            let val = if off + 1 < s.len() {
                                i16::from_ne_bytes([s[off], s[off + 1]])
                            } else { 0 };
                            resp.extend_from_slice(&(val as u16).to_be_bytes());
                        }
                        drop(s);
                        let _ = sock.write_all(&resp);
                    }
                    0x06 => { // Write Single Register
                        let addr = u16::from_be_bytes([buf[8], buf[9]]) as usize;
                        let value = u16::from_be_bytes([buf[10], buf[11]]);
                        let off = addr * 2;
                        { let mut s = st.lock().unwrap();
                          if off + 1 < s.len() {
                              let b = (value as i16).to_ne_bytes();
                              s[off] = b[0]; s[off+1] = b[1];
                          }
                        }
                        let mut resp = Vec::with_capacity(12);
                        resp.extend_from_slice(&tx_id);
                        resp.extend_from_slice(&0u16.to_be_bytes());
                        resp.extend_from_slice(&6u16.to_be_bytes());
                        resp.extend_from_slice(&buf[6..12]);
                        let _ = sock.write_all(&resp);
                    }
                    _ => {
                        let mut resp = Vec::with_capacity(9);
                        resp.extend_from_slice(&tx_id);
                        resp.extend_from_slice(&0u16.to_be_bytes());
                        resp.extend_from_slice(&3u16.to_be_bytes());
                        resp.push(unit_id);
                        resp.push(fc | 0x80);
                        resp.push(0x01);
                        let _ = sock.write_all(&resp);
                    }
                }
            }
        });
    }
}
