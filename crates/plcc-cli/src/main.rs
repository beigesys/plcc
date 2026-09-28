// SPDX-License-Identifier: MPL-2.0

use clap::{Parser, Subcommand};
use miette::{IntoDiagnostic, NamedSource, Result};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "plcc",
    about = "IEC 61131-3 compiler: Structured Text, and LD/FBD/ST in PLCopen XML"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Parse a Structured Text or PLCopen XML file and optionally dump the AST
    /// (for XML: the AST its LD/FBD/ST bodies lower to)
    Parse {
        /// Input .st file, or PLCopen .xml
        input: PathBuf,
        /// Dump AST as JSON
        #[arg(long)]
        dump_ast: bool,
    },
    /// Parse and type-check Structured Text / PLCopen XML files — exactly the
    /// check `compile` runs before code generation
    Check {
        /// Input .st and/or PLCopen .xml file(s)
        inputs: Vec<PathBuf>,
        /// Standard function block library to check against (as `compile` does)
        #[arg(long, value_enum, default_value_t = StdlibOpt::BundledSt)]
        stdlib: StdlibOpt,
    },
    /// Compile one or more Structured Text / PLCopen XML files
    Compile {
        /// Input .st and/or PLCopen .xml file(s)
        inputs: Vec<PathBuf>,
        /// Output file
        #[arg(short, long)]
        output: PathBuf,
        /// Target triple (e.g. x86_64-unknown-linux-gnu, wasm32-unknown-unknown)
        #[arg(long, default_value = "x86_64-unknown-linux-gnu")]
        target: String,
        /// Standard function block library to compile alongside the program
        #[arg(long, value_enum, default_value_t = StdlibOpt::BundledSt)]
        stdlib: StdlibOpt,
        /// Write a C header describing the runtime contract (process image, task
        /// table, entry points, state structs with layout asserts, AT bindings)
        #[arg(long, value_name = "FILE")]
        emit_header: Option<PathBuf>,
        /// Write a JSON symbol table (AT bindings, tasks, instances, and every
        /// program variable with its offset) for HMI / Modbus mapping tools
        #[arg(long, value_name = "FILE")]
        emit_symbols: Option<PathBuf>,
        /// Fix a process-image area's size in bytes instead of deriving it from the
        /// highest address used, e.g. `--image-size I=64 --image-size Q=64`
        #[arg(long, value_name = "AREA=BYTES")]
        image_size: Vec<String>,
        /// Interval of the implicit task when the source has no CONFIGURATION
        #[arg(long, value_name = "TIME", default_value = "T#20ms")]
        task_interval: String,
        /// Skip the type checker (`plcc check`) that otherwise runs before code
        /// generation and stops the build on a type error
        #[arg(long)]
        no_typecheck: bool,
    },
    /// Compile and JIT-run ST programs, optionally with Modbus TCP for SCADA
    Sim {
        /// Input .st and/or PLCopen .xml file(s)
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
        /// Standard function block library to compile alongside the program
        #[arg(long, value_enum, default_value_t = StdlibOpt::BundledSt)]
        stdlib: StdlibOpt,
        /// Skip the type checker that otherwise runs before code generation
        #[arg(long)]
        no_typecheck: bool,
    },
}

/// `--stdlib` values.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
enum StdlibOpt {
    /// Compile the bundled IEC 61131-3 standard function blocks (written in ST)
    /// into the module: SR, RS, R_TRIG, F_TRIG, CTU, CTD, CTUD, TON, TOF, TP.
    BundledSt,
    /// Compile no standard library. Every FB the program instantiates must be
    /// defined in the input files.
    None,
}

/// Read source file, falling back to Latin-1 if not valid UTF-8.
fn read_source(path: &std::path::Path) -> Result<String> {
    let bytes = std::fs::read(path).into_diagnostic()?;
    match String::from_utf8(bytes.clone()) {
        Ok(s) => Ok(s),
        Err(_) => Ok(bytes.iter().map(|&b| b as char).collect()),
    }
}

/// Parse one input: PLCopen XML (a `.xml` file, or any file whose root element
/// is `<project>`) is lowered to the ST AST by `plcc-plcopen`; anything else is
/// Structured Text. Diagnostics carry spans into `source` and no source code.
fn parse_file(
    path: &std::path::Path,
    source: &str,
) -> (plcc_st::ast::CompilationUnit, Vec<miette::Report>) {
    let is_xml = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("xml"))
        || plcc_plcopen::is_plcopen(source);
    if is_xml {
        let (unit, errors) = plcc_plcopen::parse(source);
        (unit, errors.into_iter().map(miette::Report::new).collect())
    } else {
        let (unit, errors) = plcc_st::parse(source);
        (unit, errors.into_iter().map(miette::Report::new).collect())
    }
}

/// Name of a top-level declaration, uppercased, when it has one.
fn declaration_name(decl: &plcc_st::ast::Declaration) -> Option<String> {
    use plcc_st::ast::Declaration as D;
    let name = match decl {
        D::Program(d) => &d.name.name,
        D::Function(d) => &d.name.name,
        D::FunctionBlock(d) => &d.name.name,
        D::Class(d) => &d.name.name,
        D::Interface(d) => &d.name.name,
        D::TypeDecl(d) => &d.name.name,
        D::GlobalVarDecl(_) | D::Configuration(_) => return None,
    };
    Some(name.to_uppercase())
}

/// Where a declaration of the merged unit came from, for diagnostics: spans are
/// offsets into that one file's source.
#[derive(Clone)]
struct Origin {
    name: String,
    source: std::rc::Rc<String>,
    /// Part of the bundled standard library rather than the user's input.
    prelude: bool,
}

/// The merged compilation unit of every input (plus the prelude), and the origin of
/// each of its declarations, index for index.
struct Parsed {
    unit: plcc_st::ast::CompilationUnit,
    origins: Vec<Origin>,
}

fn parse_inputs(inputs: &[PathBuf], stdlib: StdlibOpt) -> Result<Parsed> {
    let mut all_declarations = Vec::new();
    let mut origins = Vec::new();
    for input in inputs {
        let source = read_source(input)?;
        let (unit, errors) = parse_file(input, &source);
        if !errors.is_empty() {
            let file_name = input.display().to_string();
            for report in errors {
                let report = report.with_source_code(NamedSource::new(&file_name, source.clone()));
                eprintln!("{:?}", report);
            }
            std::process::exit(1);
        }
        let origin = Origin {
            name: input.display().to_string(),
            source: std::rc::Rc::new(source),
            prelude: false,
        };
        origins.extend(std::iter::repeat_n(origin, unit.declarations.len()));
        all_declarations.extend(unit.declarations);
    }

    if stdlib == StdlibOpt::BundledSt {
        // Collect the names the user defined *first*, so the prelude can step aside.
        //
        // Collision policy: the user definition wins, silently, and the colliding
        // prelude declaration is dropped. Two reasons. (1) Keeping both is not an
        // option — codegen would emit two `ton_scan` functions and LLVM would rename
        // one, so the program would call whichever won a name lookup. (2) A hard
        // error would break the common case of a codebase that already vendored its
        // own TON/CTU (OSCAT-derived projects routinely do), and would make adding a
        // block to the prelude a breaking change for every such project. `--stdlib
        // none` remains available for anyone who wants no prelude at all.
        let user_names: std::collections::HashSet<String> = all_declarations
            .iter()
            .filter_map(declaration_name)
            .collect();

        let mut prelude = Vec::new();
        let mut prelude_origins = Vec::new();
        for unit_src in plcc_stdlib::UNITS {
            let (unit, errors) = plcc_st::parse(unit_src.source);
            if !errors.is_empty() {
                // A parse error in the bundled library is a compiler bug, not a user
                // error, so say so rather than blaming the user's file.
                for err in &errors {
                    let report = miette::Report::new(err.clone()).with_source_code(
                        NamedSource::new(unit_src.name, unit_src.source.to_string()),
                    );
                    eprintln!("{:?}", report);
                }
                eprintln!(
                    "internal error: the bundled ST standard library failed to parse \
                     ({}). Re-run with --stdlib none to work around it.",
                    unit_src.name
                );
                std::process::exit(1);
            }
            let origin = Origin {
                name: unit_src.name.to_string(),
                source: std::rc::Rc::new(unit_src.source.to_string()),
                prelude: true,
            };
            for decl in unit.declarations {
                let superseded = declaration_name(&decl).is_some_and(|n| user_names.contains(&n));
                if !superseded {
                    prelude.push(decl);
                    prelude_origins.push(origin.clone());
                }
            }
        }
        // Prelude first: user code may instantiate a prelude FB, never the reverse.
        prelude.extend(all_declarations);
        all_declarations = prelude;
        prelude_origins.extend(origins);
        origins = prelude_origins;
    }

    Ok(Parsed {
        unit: plcc_st::ast::CompilationUnit {
            declarations: all_declarations,
            span: plcc_st::span::Span::empty(),
        },
        origins,
    })
}

/// Run the HIR type checker over the merged unit and print its diagnostics against
/// the file each came from. Returns whether any is an error (warnings do not stop
/// a build). `plcc check` and `plcc compile` both call this, so they always agree.
///
/// CODESYS will not build a project with type errors; neither does `compile`,
/// unless `--no-typecheck` is given.
fn run_typecheck(parsed: &Parsed) -> bool {
    let (_symbols, diagnostics) = plcc_hir::TypeChecker::new().check_located(&parsed.unit);
    let mut errors = 0;
    let mut warnings = 0;
    for (index, diag) in diagnostics {
        let Some(origin) = parsed.origins.get(index) else {
            continue;
        };
        if diag.is_warning() {
            // The bundled library is ours; its warnings are not the user's problem.
            if origin.prelude {
                continue;
            }
            warnings += 1;
        } else {
            errors += 1;
        }
        let report = miette::Report::new(diag)
            .with_source_code(NamedSource::new(&origin.name, origin.source.as_str().to_owned()));
        eprintln!("{report:?}");
    }
    if errors > 0 || warnings > 0 {
        eprintln!("type check: {errors} error(s), {warnings} warning(s)");
    }
    errors > 0
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Parse { input, dump_ast } => {
            let source = read_source(&input)?;
            let (unit, errors) = parse_file(&input, &source);
            let error_count = errors.len();

            if !errors.is_empty() {
                let file_name = input.display().to_string();
                for report in errors {
                    let report =
                        report.with_source_code(NamedSource::new(&file_name, source.clone()));
                    eprintln!("{:?}", report);
                }
                eprintln!("{error_count} error(s)");
            }

            if dump_ast {
                let json = serde_json::to_string_pretty(&unit).into_diagnostic()?;
                println!("{json}");
            } else if error_count == 0 {
                println!("OK: {} declaration(s) parsed", unit.declarations.len());
            }

            if error_count == 0 {
                Ok(())
            } else {
                std::process::exit(1);
            }
        }
        Commands::Check { inputs, stdlib } => {
            if inputs.is_empty() {
                eprintln!("Error: at least one input file is required");
                std::process::exit(1);
            }
            let parsed = parse_inputs(&inputs, stdlib)?;
            if run_typecheck(&parsed) {
                std::process::exit(1);
            }
            let user_decls = parsed.origins.iter().filter(|o| !o.prelude).count();
            println!("OK: {user_decls} declaration(s) checked");
            Ok(())
        }
        Commands::Compile {
            inputs,
            output,
            target,
            stdlib,
            emit_header,
            emit_symbols,
            image_size,
            task_interval,
            no_typecheck,
        } => {
            if inputs.is_empty() {
                eprintln!("Error: at least one input file is required");
                std::process::exit(1);
            }

            let parsed = parse_inputs(&inputs, stdlib)?;
            if !no_typecheck && run_typecheck(&parsed) {
                eprintln!("not compiled: fix the type errors, or pass --no-typecheck");
                std::process::exit(1);
            }
            let merged = parsed.unit;
            let context = inkwell::context::Context::create();
            let mut compiler =
                plcc_codegen::Compiler::new(&context, &inputs[0].display().to_string());
            for spec in &image_size {
                let (area, bytes) = parse_image_size(spec)?;
                compiler.set_image_size(area, bytes);
            }
            let interval = plcc_codegen::compiler::contract::parse_duration_ns(&task_interval)
                .ok_or_else(|| miette::miette!("--task-interval: `{task_interval}` is not a TIME"))?;
            compiler.set_task_options(plcc_codegen::TaskOptions {
                default_interval_ns: interval,
            });

            if let Err(e) = compiler.compile(&merged) {
                report_codegen_error(&e, &inputs);
                std::process::exit(1);
            }

            if emit_header.is_some() || emit_symbols.is_some() {
                let contract = compiler
                    .runtime_contract(&target)
                    .map_err(|e| miette::miette!("{e}"))?;
                if let Some(path) = &emit_header {
                    let guard = format!(
                        "PLCC_{}_H",
                        path.file_stem().and_then(|s| s.to_str()).unwrap_or("program")
                    );
                    std::fs::write(path, plcc_codegen::header::c_header(&contract, &guard))
                        .into_diagnostic()?;
                }
                if let Some(path) = &emit_symbols {
                    std::fs::write(path, plcc_codegen::header::symbols_json(&contract))
                        .into_diagnostic()?;
                }
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
            stdlib,
            no_typecheck,
        } => {
            if inputs.is_empty() {
                eprintln!("Usage: plcc sim <program.st> [--scans 0] [--modbus 502]");
                std::process::exit(1);
            }

            let parsed = parse_inputs(&inputs, stdlib)?;
            if !no_typecheck && run_typecheck(&parsed) {
                eprintln!("not run: fix the type errors, or pass --no-typecheck");
                std::process::exit(1);
            }
            let merged = parsed.unit;
            let context = inkwell::context::Context::create();
            let mut compiler = plcc_codegen::Compiler::new(&context, "sim");
            if let Err(e) = compiler.compile(&merged) {
                eprintln!("Codegen error: {e}");
                std::process::exit(1);
            }

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

            let scan_name = scan_fns.last().unwrap();
            let init_name = scan_name.replace("_scan", "_init");
            let prog_name = scan_name.trim_end_matches("_scan").to_string();

            let ee = compiler
                .module()
                .create_jit_execution_engine(inkwell::OptimizationLevel::None)
                .map_err(|e| miette::miette!("JIT error: {e}"))?;

            // Provide plcc_print for JIT (PRINT statement calls this)
            ee.add_global_mapping(
                &compiler
                    .module()
                    .get_function("plcc_print")
                    .unwrap_or_else(|| {
                        let fn_type = compiler.module().get_context().void_type().fn_type(
                            &[compiler
                                .module()
                                .get_context()
                                .ptr_type(inkwell::AddressSpace::default())
                                .into()],
                            false,
                        );
                        compiler.module().add_function("plcc_print", fn_type, None)
                    }),
                // Cast through a pointer, not straight to usize: a bare `fn item
                // as usize` is a zero-sized item type, and rustc's
                // function_casts_as_integer lint flags it as ambiguous.
                plcc_print_impl as *const () as usize,
            );

            // Provide plcc_monotonic_ns for JIT (MONOTONIC_NS() and the bundled ST
            // timer FBs call this). Same shape as plcc_print: declare it if the module
            // did not already import it, then map the symbol onto the host clock.
            ee.add_global_mapping(
                &compiler
                    .module()
                    .get_function("plcc_monotonic_ns")
                    .unwrap_or_else(|| {
                        let fn_type = compiler
                            .module()
                            .get_context()
                            .i64_type()
                            .fn_type(&[], false);
                        compiler
                            .module()
                            .add_function("plcc_monotonic_ns", fn_type, None)
                    }),
                plcc_runtime::host_clock::plcc_monotonic_ns as *const () as usize,
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

            let scan_ptr = ee
                .get_function_address(scan_name)
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
                        } else {
                            0
                        }
                    };
                    println!(
                        "scan {cycle:>6} | run={} raw={}% clean={}% cycle={}",
                        rd(10),
                        rd(15),
                        rd(16),
                        rd(14)
                    );
                }

                cycle += 1;
                if !run_forever && cycle >= scans as u64 {
                    break;
                }
                if interval_ms > 0 {
                    std::thread::sleep(interval);
                }
            }

            eprintln!("\n{prog_name} done.");
            Ok(())
        }
    }
}

/// `I=64` / `q=16` / `M=256`.
fn parse_image_size(spec: &str) -> Result<(plcc_codegen::direct_address::Area, u32)> {
    use plcc_codegen::direct_address::Area;
    let (a, n) = spec
        .split_once('=')
        .ok_or_else(|| miette::miette!("--image-size expects AREA=BYTES, got `{spec}`"))?;
    let area = match a.trim().to_ascii_uppercase().as_str() {
        "I" => Area::Input,
        "Q" => Area::Output,
        "M" => Area::Memory,
        other => return Err(miette::miette!("--image-size: unknown area `{other}` (I, Q or M)")),
    };
    let bytes = n
        .trim()
        .parse()
        .map_err(|_| miette::miette!("--image-size: `{n}` is not a byte count"))?;
    Ok((area, bytes))
}

/// Print a codegen error, with its source excerpt when it carries a span and the
/// span can be attributed to a file (a single input).
fn report_codegen_error(e: &plcc_codegen::compiler::CodegenError, inputs: &[PathBuf]) {
    if let (Some(span), [input]) = (e.span(), inputs)
        && let Ok(source) = read_source(input)
        && span.end <= source.len()
    {
        let report = miette::miette!(
            labels = vec![miette::LabeledSpan::at(span.start..span.end, "here")],
            "{e}"
        )
        .with_source_code(NamedSource::new(input.display().to_string(), source));
        eprintln!("{report:?}");
        return;
    }
    match e.span() {
        Some(span) => eprintln!("Codegen error: {e} (source offset {})", span.start),
        None => eprintln!("Codegen error: {e}"),
    }
}

/// PRINT implementation for JIT — outputs to stderr
extern "C" fn plcc_print_impl(msg: *const u8) {
    if msg.is_null() {
        return;
    }
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
                if n < 12 {
                    continue;
                }

                let tx_id = [buf[0], buf[1]];
                let unit_id = buf[6];
                let fc = buf[7];

                // Coil address N maps to state INT at byte offset (N-1)*2
                // Value is bool: INT != 0 → true
                match fc {
                    0x01 => {
                        // Read Coils (FC01)
                        let start = u16::from_be_bytes([buf[8], buf[9]]) as usize;
                        let count = u16::from_be_bytes([buf[10], buf[11]]) as usize;
                        let byte_count = (count + 7) / 8;
                        let mut resp = Vec::with_capacity(9 + byte_count);
                        resp.extend_from_slice(&tx_id);
                        resp.extend_from_slice(&0u16.to_be_bytes());
                        resp.extend_from_slice(&((3 + byte_count) as u16).to_be_bytes());
                        resp.push(unit_id);
                        resp.push(0x01);
                        resp.push(byte_count as u8);
                        let s = st.lock().unwrap();
                        for byte_idx in 0..byte_count {
                            let mut byte_val: u8 = 0;
                            for bit in 0..8 {
                                let coil = start + byte_idx * 8 + bit;
                                if coil < start + count {
                                    let off = coil * 2; // each coil = one INT16 in state
                                    if off + 1 < s.len() {
                                        let v = i16::from_ne_bytes([s[off], s[off + 1]]);
                                        if v != 0 {
                                            byte_val |= 1 << bit;
                                        }
                                    }
                                }
                            }
                            resp.push(byte_val);
                        }
                        drop(s);
                        let _ = sock.write_all(&resp);
                    }
                    0x05 => {
                        // Write Single Coil (FC05)
                        let addr = u16::from_be_bytes([buf[8], buf[9]]) as usize;
                        let raw_val = u16::from_be_bytes([buf[10], buf[11]]);
                        let value: i16 = if raw_val == 0xFF00 { 1 } else { 0 };
                        let off = addr * 2;
                        {
                            let mut s = st.lock().unwrap();
                            if off + 1 < s.len() {
                                let b = value.to_ne_bytes();
                                s[off] = b[0];
                                s[off + 1] = b[1];
                            }
                        }
                        let mut resp = Vec::with_capacity(12);
                        resp.extend_from_slice(&tx_id);
                        resp.extend_from_slice(&0u16.to_be_bytes());
                        resp.extend_from_slice(&6u16.to_be_bytes());
                        resp.extend_from_slice(&buf[6..12]);
                        let _ = sock.write_all(&resp);
                    }
                    0x03 => {
                        // Read Holding Registers (FC03)
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
                            } else {
                                0
                            };
                            resp.extend_from_slice(&(val as u16).to_be_bytes());
                        }
                        drop(s);
                        let _ = sock.write_all(&resp);
                    }
                    0x06 => {
                        // Write Single Register (FC06)
                        let addr = u16::from_be_bytes([buf[8], buf[9]]) as usize;
                        let value = u16::from_be_bytes([buf[10], buf[11]]);
                        let off = addr * 2;
                        {
                            let mut s = st.lock().unwrap();
                            if off + 1 < s.len() {
                                let b = (value as i16).to_ne_bytes();
                                s[off] = b[0];
                                s[off + 1] = b[1];
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
