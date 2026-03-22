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
        /// Number of scan cycles
        #[arg(long, default_value = "20")]
        scans: usize,
        /// Interval between scans in milliseconds (0 = no delay)
        #[arg(long, default_value = "0")]
        interval_ms: u64,
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
        } => {
            if inputs.is_empty() {
                eprintln!("Usage: plcc sim <program.st> [--scans N] [--interval-ms MS]");
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

            // Find program scan functions
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

            // JIT execute
            let ee = compiler
                .module()
                .create_jit_execution_engine(inkwell::OptimizationLevel::None)
                .map_err(|e| miette::miette!("JIT error: {e}"))?;

            for scan_name in &scan_fns {
                let init_name = scan_name.replace("_scan", "_init");
                let prog_name = scan_name.trim_end_matches("_scan");

                let state_size = 4096;
                let mut state = vec![0u8; state_size];
                let state_ptr = state.as_mut_ptr();

                // Init
                if let Ok(init_ptr) = ee.get_function_address(&init_name) {
                    let init: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(init_ptr) };
                    init(state_ptr);
                }

                let scan_ptr = ee
                    .get_function_address(scan_name)
                    .map_err(|_| miette::miette!("Function {scan_name} not found"))?;
                let scan_fn: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(scan_ptr) };

                eprintln!("Running {prog_name} for {scans} scans...\n");

                for cycle in 0..scans {
                    scan_fn(state_ptr);

                    // Print state snapshot
                    let n = 16.min(state_size);
                    let hex: String = state[..n]
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<Vec<_>>()
                        .join(" ");
                    let v0 = if state_size >= 2 {
                        i16::from_ne_bytes([state[0], state[1]])
                    } else {
                        0
                    };
                    let v1 = if state_size >= 4 {
                        i16::from_ne_bytes([state[2], state[3]])
                    } else {
                        0
                    };
                    let v2 = if state_size >= 6 {
                        i16::from_ne_bytes([state[4], state[5]])
                    } else {
                        0
                    };
                    println!("scan {cycle:>4} | {hex} | [{v0}, {v1}, {v2}, ...]");

                    if interval_ms > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(interval_ms));
                    }
                }

                eprintln!("\n{prog_name} done.");
            }
            Ok(())
        }
    }
}
