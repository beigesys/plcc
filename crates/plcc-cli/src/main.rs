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
    /// Compile a Structured Text file
    Compile {
        /// Input .st file
        input: PathBuf,
        /// Output file
        #[arg(short, long)]
        output: PathBuf,
        /// Target triple (e.g. x86_64-unknown-linux-gnu, wasm32-unknown-unknown)
        #[arg(long, default_value = "x86_64-unknown-linux-gnu")]
        target: String,
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
            input,
            output,
            target,
        } => {
            let source = read_source(&input)?;
            let (unit, parse_errors) = plcc_st::parse(&source);

            if !parse_errors.is_empty() {
                let file_name = input.display().to_string();
                for err in &parse_errors {
                    let report = miette::Report::new(err.clone())
                        .with_source_code(NamedSource::new(&file_name, source.clone()));
                    eprintln!("{:?}", report);
                }
                std::process::exit(1);
            }

            let context = inkwell::context::Context::create();
            let mut compiler = plcc_codegen::Compiler::new(&context, &input.display().to_string());

            if let Err(e) = compiler.compile(&unit) {
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

            println!("Compiled to {}", output.display());
            Ok(())
        }
    }
}
