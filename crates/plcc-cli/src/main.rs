// SPDX-License-Identifier: MPL-2.0

mod convert;
mod device;
mod image;

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
        /// Input .st file, PLCopen .xml, TwinCAT object (.TcPOU, ...), .plcproj or directory
        input: PathBuf,
        /// Dump AST as JSON
        #[arg(long)]
        dump_ast: bool,
        /// Rockwell L5X: print the Structured Text the project lowers to
        #[arg(long)]
        dump_st: bool,
    },
    /// Parse and type-check Structured Text / PLCopen XML files — exactly the
    /// check `compile` runs before code generation
    Check {
        /// Input .st, PLCopen .xml, TwinCAT objects (.TcPOU/.TcDUT/.TcGVL/.TcIO/.TcTTO), .plcproj files or directories
        inputs: Vec<PathBuf>,
        /// Standard function block library to check against (as `compile` does)
        #[arg(long, value_enum, default_value_t = StdlibOpt::BundledSt)]
        stdlib: StdlibOpt,
        /// Rockwell L5X: TOML file binding Logix tags (module tags such as
        /// `Local:1:I.Data.0`, aliases, base tags) to process-image addresses
        /// (`%IX0.0`); see docs/l5x.md
        #[arg(long, value_name = "FILE")]
        io_map: Option<PathBuf>,
    },
    /// Compile one or more Structured Text / PLCopen XML files
    Compile {
        /// Input .st, PLCopen .xml, TwinCAT objects (.TcPOU/.TcDUT/.TcGVL/.TcIO/.TcTTO), .plcproj files or directories
        inputs: Vec<PathBuf>,
        /// Output file
        #[arg(short, long)]
        output: PathBuf,
        /// Device manifest (a .toml file, or a catalog id such as arduino-opta):
        /// sets the target triple, CPU, features, float ABI and process-image
        /// sizes; explicit flags override it (docs/device-manifest.md)
        #[arg(long, value_name = "FILE|ID")]
        device: Option<String>,
        /// Target triple (e.g. x86_64-unknown-linux-gnu, wasm32-unknown-unknown);
        /// default: the device's, else x86_64-unknown-linux-gnu
        #[arg(long)]
        target: Option<String>,
        /// LLVM CPU to generate code for (e.g. cortex-m7); default: the triple's generic CPU
        #[arg(long, value_name = "CPU")]
        cpu: Option<String>,
        /// LLVM target features, comma-separated or repeated (e.g. +fp-armv8d16)
        #[arg(long, value_name = "+FEATURE", value_delimiter = ',', allow_hyphen_values = true)]
        features: Vec<String>,
        /// ARM float ABI: soft (no FPU instructions), softfp (FPU instructions,
        /// floats passed in integer registers; `eabi` triples) or hard (floats in
        /// FPU registers; `eabihf` triples)
        #[arg(long, value_name = "ABI")]
        float_abi: Option<String>,
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
        /// Optimization level: 0 (default) runs no IR optimization, 1-3 run
        /// LLVM's `default<O1>`..`default<O3>` pipeline before emitting
        #[arg(short = 'O', long = "opt-level", default_value_t = 0,
              value_parser = clap::value_parser!(u8).range(0..=3))]
        opt_level: u8,
        /// Rockwell L5X: TOML file binding Logix tags (module tags such as
        /// `Local:1:I.Data.0`, aliases, base tags) to process-image addresses
        /// (`%IX0.0`); see docs/l5x.md
        #[arg(long, value_name = "FILE")]
        io_map: Option<PathBuf>,
    },
    /// Convert a program to another notation: canonical Structured Text, IEC
    /// ladder (PLCopen XML), Rockwell ladder (L5X) or the ladder model as JSON;
    /// dialect differences are reported as warnings (docs/ladder-translation.md)
    Convert {
        /// Input .st, PLCopen .xml, .L5X, ladder .json, TwinCAT objects,
        /// .plcproj files or directories (ladder outputs take one input)
        inputs: Vec<PathBuf>,
        /// Output notation
        #[arg(long, value_enum)]
        to: convert::Format,
        /// Output file (default: standard output)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Ladder dialect of `--to ladder-json` / `--to st` output (translates
        /// between IEC and Logix ladder); plcopen is always IEC, l5x Logix
        #[arg(long, value_enum)]
        dialect: Option<convert::DialectOpt>,
        /// With L5X inputs and `--to st`: append the Logix prelude the
        /// generated ST calls, so the output compiles on its own
        #[arg(long)]
        prelude: bool,
    },
    /// Device manifests: validate them, list the catalog (docs/device-manifest.md)
    Device {
        #[command(subcommand)]
        command: DeviceCommand,
    },
    /// Link a compiled object into a program image for a device's program
    /// slot (docs/program-image.md), or check an image with --info
    Image {
        /// The object from `plcc compile --device <id>` (or, with --info, an image)
        input: PathBuf,
        /// Device manifest (file or catalog id) with a [flash.program] slot
        #[arg(long)]
        device: String,
        /// Output image
        #[arg(short, long, required_unless_present = "info")]
        output: Option<PathBuf>,
        /// Build id for the header: 32 hex digits (default: from the object's SHA-256)
        #[arg(long, value_name = "HEX")]
        build_id: Option<String>,
        /// Print the image map (sections, veneers, entry point)
        #[arg(long)]
        map: bool,
        /// Print the link report as JSON
        #[arg(long)]
        json: bool,
        /// INPUT is an image: print its header and check it as the device's runtime would
        #[arg(long)]
        info: bool,
    },
    /// Compile and JIT-run ST programs, optionally with Modbus TCP for SCADA
    Sim {
        /// Input .st, PLCopen .xml, TwinCAT objects (.TcPOU/.TcDUT/.TcGVL/.TcIO/.TcTTO), .plcproj files or directories
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
        /// Rockwell L5X: TOML file binding Logix tags (module tags such as
        /// `Local:1:I.Data.0`, aliases, base tags) to process-image addresses
        /// (`%IX0.0`); see docs/l5x.md
        #[arg(long, value_name = "FILE")]
        io_map: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum DeviceCommand {
    /// Validate device manifests and report problems with file, line and column
    Check {
        files: Vec<PathBuf>,
        /// Print the expanded manifest(s) as JSON (`repeat` groups unrolled)
        #[arg(long)]
        json: bool,
    },
    /// List the device catalog (`devices/`, or the built-in copies)
    List,
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

/// Parse one input: a TwinCAT object (`.TcPOU`, `.TcDUT`, `.TcGVL`, `.TcIO`,
/// or any file whose root element is `<TcPlcObject>`) goes through
/// `plcc-twincat`; a Rockwell `.L5X` export (or any file whose root is
/// `<RSLogix5000Content>`) through `plcc-l5x`; PLCopen XML (a `.xml` file, or
/// any file whose root element is `<project>`) is lowered to the ST AST by
/// `plcc-plcopen`; anything else is Structured Text. Diagnostics carry spans
/// into `source` and no source code. The flag is whether any of them is an
/// error (L5X lowering also reports warnings).
fn parse_file(
    path: &std::path::Path,
    source: &str,
    l5x: &plcc_l5x::Options,
) -> (plcc_st::ast::CompilationUnit, Vec<miette::Report>, bool) {
    parse_file_with(path, source, l5x, false)
}

/// [`parse_file`], optionally with ladder rungs annotated by comments
/// (`plcc convert --to st`).
fn parse_file_with(
    path: &std::path::Path,
    source: &str,
    l5x: &plcc_l5x::Options,
    annotate: bool,
) -> (plcc_st::ast::CompilationUnit, Vec<miette::Report>, bool) {
    if plcc_twincat::is_object_file(path) || plcc_twincat::is_twincat_object(source) {
        let (unit, errors) = plcc_twincat::parse(source);
        let reports: Vec<miette::Report> = errors.into_iter().map(miette::Report::new).collect();
        let failed = !reports.is_empty();
        return (unit, reports, failed);
    }
    if is_l5x_input(path, source) {
        let (unit, diags) = if annotate {
            plcc_l5x::parse_annotated(source, l5x)
        } else {
            plcc_l5x::parse_with(source, l5x)
        };
        let failed = diags.iter().any(|d| !d.is_warning());
        return (
            unit,
            diags.into_iter().map(miette::Report::new).collect(),
            failed,
        );
    }
    let is_xml = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("xml"))
        || plcc_plcopen::is_plcopen(source);
    let (unit, reports): (_, Vec<miette::Report>) = if is_xml {
        let opts = plcc_plcopen::Options {
            annotate_rungs: annotate,
        };
        let (unit, errors) = plcc_plcopen::parse_with(source, &opts);
        (unit, errors.into_iter().map(miette::Report::new).collect())
    } else {
        let (unit, errors) = plcc_st::parse(source);
        (unit, errors.into_iter().map(miette::Report::new).collect())
    };
    let failed = !reports.is_empty();
    (unit, reports, failed)
}

/// Diagnostics shown in full per file; the rest are only counted. Rendering a
/// diagnostic scans its file up to the span, so thousands of them against a
/// multi-megabyte export would take minutes.
const MAX_SHOWN_REPORTS: usize = 200;

fn print_reports(file_name: &str, source: &std::sync::Arc<String>, reports: Vec<miette::Report>) {
    let total = reports.len();
    for report in reports.into_iter().take(MAX_SHOWN_REPORTS) {
        let report = report.with_source_code(NamedSource::new(file_name, source.clone()));
        eprintln!("{report:?}");
    }
    if total > MAX_SHOWN_REPORTS {
        eprintln!(
            "{file_name}: {} more diagnostic(s) not shown",
            total - MAX_SHOWN_REPORTS
        );
    }
}

fn is_l5x_input(path: &std::path::Path, source: &str) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("l5x"))
        || plcc_l5x::is_l5x(source)
}

/// `--io-map FILE` → L5X options.
fn l5x_options(io_map: &Option<PathBuf>) -> Result<plcc_l5x::Options> {
    let mut opts = plcc_l5x::Options::default();
    if let Some(p) = io_map {
        let text = read_source(p)?;
        opts.io_map =
            plcc_l5x::IoMap::parse(&text).map_err(|e| miette::miette!("{}: {e}", p.display()))?;
    }
    Ok(opts)
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
    /// Lowered from a Rockwell L5X project (or the Logix prelude): Logix
    /// semantics, e.g. an out-of-range subscript is a fault (docs/l5x.md).
    logix: bool,
}

/// The merged compilation unit of every input (plus the prelude), and the origin of
/// each of its declarations, index for index.
struct Parsed {
    unit: plcc_st::ast::CompilationUnit,
    origins: Vec<Origin>,
    /// Libraries the TwinCAT projects reference that neither plcc nor another
    /// input provides; named when the type check fails.
    missing_libraries: Vec<String>,
}

/// The source files the command-line inputs stand for, and the TwinCAT tasks
/// among them. A `.plcproj` is the object files it compiles; a directory is the
/// one `.plcproj` below it, or else every TwinCAT object file below it; a
/// `.TcTTO` contributes its task (see [`twincat_configuration`]).
///
/// The first TwinCAT project is the application. Every later one is a library
/// it uses (TwinCAT references libraries by name; plcc needs their sources):
/// its tasks are ignored, and a declaration whose name the application (or an
/// earlier library) already declares — a library's own test `MAIN`, say — is
/// left out.
struct Inputs {
    /// Each file, and whether it belongs to a library project.
    files: Vec<(PathBuf, bool)>,
    tasks: Vec<(PathBuf, plcc_twincat::Task)>,
    /// Set while expanding a project after the first.
    library: bool,
    projects: usize,
    /// (library references, project name) of every TwinCAT project.
    references: Vec<(Vec<String>, String)>,
}

fn expand_inputs(inputs: &[PathBuf]) -> Result<Inputs> {
    let mut out = Inputs {
        files: Vec::new(),
        tasks: Vec::new(),
        library: false,
        projects: 0,
        references: Vec::new(),
    };
    for input in inputs {
        out.library = false;
        let project = if input.is_dir() {
            match plcc_twincat::directory_input(input) {
                Ok(plcc_twincat::DirectoryInput::Project(p)) => p,
                Ok(plcc_twincat::DirectoryInput::Files(files)) => {
                    if files.is_empty() {
                        miette::bail!("{}: no TwinCAT project or object files", input.display());
                    }
                    add_input_files(&mut out, files)?;
                    continue;
                }
                Err(e) => miette::bail!("{e}"),
            }
        } else if plcc_twincat::is_project_file(input) {
            input.clone()
        } else {
            add_input_files(&mut out, vec![input.clone()])?;
            continue;
        };
        out.projects += 1;
        out.library = out.projects > 1;
        let source = read_source(&project)?;
        let stem = project
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        out.references
            .push((plcc_twincat::project_libraries(&source), stem));
        match plcc_twincat::project_files(&project, &source) {
            Ok(files) => add_input_files(&mut out, files)?,
            Err(e) => {
                let report = miette::Report::new(e).with_source_code(NamedSource::new(
                    project.display().to_string(),
                    source,
                ));
                eprintln!("{report:?}");
                std::process::exit(1);
            }
        }
    }
    Ok(out)
}

fn add_input_files(out: &mut Inputs, files: Vec<PathBuf>) -> Result<()> {
    for file in files {
        if !plcc_twincat::is_task_file(&file) {
            out.files.push((file, out.library));
            continue;
        }
        if out.library {
            continue;
        }
        let source = read_source(&file)?;
        match plcc_twincat::parse_task(&source) {
            Ok(Some(task)) => out.tasks.push((file, task)),
            Ok(None) => {}
            Err(e) => {
                let report = miette::Report::new(e)
                    .with_source_code(NamedSource::new(file.display().to_string(), source));
                eprintln!("{report:?}");
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

/// Print a file's parse diagnostics; returns whether any is an error (warnings,
/// such as an ignored SFC transition in a TwinCAT POU, do not stop a build).
fn print_parse_diagnostics(path: &std::path::Path, source: &str, reports: Vec<miette::Report>) -> bool {
    let failed = reports
        .iter()
        .any(|r| r.severity() != Some(miette::Severity::Warning));
    if !reports.is_empty() {
        // One shared copy: a large project can have thousands of diagnostics.
        let shared = std::sync::Arc::new(source.to_string());
        print_reports(&path.display().to_string(), &shared, reports);
    }
    failed
}

/// TwinCAT's task configuration (`.TcTTO` files), as the ST CONFIGURATION it
/// stands for — added to the build unless the sources declare their own.
fn twincat_configuration(
    inputs: &Inputs,
    declarations: &[plcc_st::ast::Declaration],
) -> Option<(String, plcc_st::ast::CompilationUnit)> {
    let has_config = declarations
        .iter()
        .any(|d| matches!(d, plcc_st::ast::Declaration::Configuration(_)));
    if has_config {
        return None;
    }
    let tasks: Vec<plcc_twincat::Task> = inputs.tasks.iter().map(|(_, t)| t.clone()).collect();
    let text = plcc_twincat::configuration_source(&tasks)?;
    let (unit, errors) = plcc_st::parse(&text);
    if !errors.is_empty() {
        // Task or program names that are not ST identifiers; say so and fall
        // back to the implicit task.
        eprintln!("warning: the TwinCAT task configuration could not be used; programs run in the default task");
        return None;
    }
    Some((text, unit))
}

fn parse_inputs(inputs: &[PathBuf], stdlib: StdlibOpt, l5x: &plcc_l5x::Options) -> Result<Parsed> {
    let inputs = expand_inputs(inputs)?;
    let mut all_declarations = Vec::new();
    let mut origins = Vec::new();
    let mut failed = false;
    let mut any_l5x = false;
    let mut defined: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut shadowed = Vec::new();
    for (input, library) in &inputs.files {
        let source = read_source(input)?;
        let logix = is_l5x_input(input, &source);
        any_l5x |= logix;
        let (unit, errors, file_failed) = parse_file(input, &source, l5x);
        let printed_failed = print_parse_diagnostics(input, &source, errors);
        // L5X lowering reports warnings too; only its errors fail the build.
        failed |= if logix { file_failed } else { printed_failed };
        let origin = Origin {
            name: input.display().to_string(),
            source: std::rc::Rc::new(source),
            prelude: false,
            logix,
        };
        for decl in unit.declarations {
            let name = declaration_name(&decl);
            if *library && name.as_ref().is_some_and(|n| defined.contains(n)) {
                shadowed.push(name.unwrap_or_default());
                continue;
            }
            defined.extend(name);
            origins.push(origin.clone());
            all_declarations.push(decl);
        }
    }
    if failed {
        std::process::exit(1);
    }
    if !shadowed.is_empty() {
        eprintln!(
            "note: {} declaration(s) of library projects left out: the application already \
             declares {}",
            shadowed.len(),
            shadowed.join(", ")
        );
    }
    if let Some((text, unit)) = twincat_configuration(&inputs, &all_declarations) {
        let origin = Origin {
            name: "<TwinCAT task configuration>".to_string(),
            source: std::rc::Rc::new(text),
            prelude: false,
            logix: false,
        };
        origins.extend(std::iter::repeat_n(origin, unit.declarations.len()));
        all_declarations.extend(unit.declarations);
    }

    // Rockwell projects need the Logix instruction prelude (TIMER, COUNTER,
    // the instruction helpers), whatever `--stdlib` says.
    if any_l5x {
        let src = plcc_l5x::prelude();
        let (unit, errors) = plcc_st::parse(&src);
        if !errors.is_empty() {
            for err in &errors {
                let report = miette::Report::new(err.clone())
                    .with_source_code(NamedSource::new(plcc_l5x::PRELUDE_NAME, src.clone()));
                eprintln!("{:?}", report);
            }
            eprintln!("internal error: the bundled Logix prelude failed to parse");
            std::process::exit(1);
        }
        let origin = Origin {
            name: plcc_l5x::PRELUDE_NAME.to_string(),
            source: std::rc::Rc::new(src),
            prelude: true,
            logix: true,
        };
        let mut decls = unit.declarations;
        let mut os: Vec<Origin> = std::iter::repeat_n(origin, decls.len()).collect();
        decls.extend(all_declarations);
        os.extend(origins);
        all_declarations = decls;
        origins = os;
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
                logix: false,
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

    let given: Vec<String> = inputs
        .references
        .iter()
        .map(|(_, name)| name.to_uppercase())
        .collect();
    let mut missing_libraries: Vec<String> = Vec::new();
    for lib in inputs.references.iter().flat_map(|(libs, _)| libs) {
        if !plcc_hir::libraries::provided(lib)
            && !given.contains(&lib.to_uppercase())
            && !missing_libraries.iter().any(|m| m.eq_ignore_ascii_case(lib))
        {
            missing_libraries.push(lib.clone());
        }
    }
    Ok(Parsed {
        unit: plcc_st::ast::CompilationUnit {
            declarations: all_declarations,
            span: plcc_st::span::Span::empty(),
        },
        origins,
        missing_libraries,
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
        let report = miette::Report::new(diag).with_source_code(NamedSource::new(
            &origin.name,
            origin.source.as_str().to_owned(),
        ));
        eprintln!("{report:?}");
    }
    if errors > 0 || warnings > 0 {
        eprintln!("type check: {errors} error(s), {warnings} warning(s)");
    }
    if errors > 0 && !parsed.missing_libraries.is_empty() {
        eprintln!(
            "note: the TwinCAT project references libraries plcc does not provide: {} \
             (pass the .plcproj of an open-source library after the application's)",
            parsed.missing_libraries.join(", ")
        );
    }
    errors > 0
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Convert {
            inputs,
            to,
            output,
            dialect,
            prelude,
        } => convert::run(convert::Args {
            inputs,
            to,
            output,
            prelude,
            dialect,
        }),
        Commands::Parse {
            input,
            dump_ast,
            dump_st,
        } => {
            if dump_st {
                let source = read_source(&input)?;
                let (st, diags) = plcc_l5x::to_st(&source, &plcc_l5x::Options::default());
                println!("{st}");
                let file_name = input.display().to_string();
                let shared = std::sync::Arc::new(source.clone());
                print_reports(&file_name, &shared, diags.into_iter().map(miette::Report::new).collect());
                return Ok(());
            }
            // A TwinCAT project or directory parses every file it stands for.
            let inputs = expand_inputs(std::slice::from_ref(&input))?;
            let mut unit = plcc_st::ast::CompilationUnit {
                declarations: Vec::new(),
                span: plcc_st::span::Span::empty(),
            };
            let mut error_count = 0;
            for (file, _) in &inputs.files {
                let source = read_source(file)?;
                let logix = is_l5x_input(file, &source);
                let (file_unit, errors, failed) =
                    parse_file(file, &source, &plcc_l5x::Options::default());
                error_count += if logix {
                    if failed { errors.len() } else { 0 }
                } else {
                    errors
                        .iter()
                        .filter(|r| r.severity() != Some(miette::Severity::Warning))
                        .count()
                };
                print_parse_diagnostics(file, &source, errors);
                unit.declarations.extend(file_unit.declarations);
            }
            if error_count > 0 {
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
        Commands::Check {
            inputs,
            stdlib,
            io_map,
        } => {
            if inputs.is_empty() {
                eprintln!("Error: at least one input file is required");
                std::process::exit(1);
            }
            let parsed = parse_inputs(&inputs, stdlib, &l5x_options(&io_map)?)?;
            if run_typecheck(&parsed) {
                std::process::exit(1);
            }
            let user_decls = parsed.origins.iter().filter(|o| !o.prelude).count();
            println!("OK: {user_decls} declaration(s) checked");
            Ok(())
        }
        Commands::Device { command } => match command {
            DeviceCommand::Check { files, json } => device::check(&files, json),
            DeviceCommand::List => device::list(),
        },
        Commands::Image { input, device, output, build_id, map, json, info } => {
            image::run(&input, &device, output.as_deref(), build_id.as_deref(), map, json, info)
        }
        Commands::Compile {
            inputs,
            output,
            device,
            target,
            cpu,
            features,
            float_abi,
            stdlib,
            emit_header,
            emit_symbols,
            image_size,
            task_interval,
            no_typecheck,
            opt_level,
            io_map,
        } => {
            if inputs.is_empty() {
                eprintln!("Error: at least one input file is required");
                std::process::exit(1);
            }

            // The device manifest supplies what the flags leave out.
            let device = device.as_deref().map(device::resolve).transpose()?;
            let dev = device.as_ref().map(|d| &d.device);
            if let Some(d) = dev {
                let abi = plcc_codegen::compiler::contract::ABI_VERSION;
                if d.target.runtime.abi != abi {
                    miette::bail!(
                        "{}: the {} runtime implements runtime ABI {}, this plcc compiles for ABI {abi}",
                        device.as_ref().map_or("", |d| d.name.as_str()),
                        d.target.runtime.kind,
                        d.target.runtime.abi
                    );
                }
            }
            let target = target
                .or_else(|| dev.map(|d| d.target.triple.clone()))
                .unwrap_or_else(|| "x86_64-unknown-linux-gnu".to_string());
            let cpu = cpu.or_else(|| dev.and_then(|d| d.target.cpu.clone()));
            let features = if features.is_empty() {
                dev.map(|d| d.target.features.clone()).unwrap_or_default()
            } else {
                features
            };
            let float_abi = float_abi.or_else(|| {
                dev.and_then(|d| d.target.float_abi.map(|a| a.as_str().to_string()))
            });
            let mut image_size = image_size;
            if let Some(d) = dev {
                let given: Vec<String> = image_size
                    .iter()
                    .filter_map(|s| s.split_once('=').map(|(a, _)| a.trim().to_ascii_uppercase()))
                    .collect();
                let img = d.target.image;
                for (area, bytes) in [("I", img.i), ("Q", img.q), ("M", img.m)] {
                    if !given.iter().any(|g| g == area) {
                        image_size.push(format!("{area}={bytes}"));
                    }
                }
            }

            let parsed = parse_inputs(&inputs, stdlib, &l5x_options(&io_map)?)?;
            if !no_typecheck && run_typecheck(&parsed) {
                eprintln!("not compiled: fix the type errors, or pass --no-typecheck");
                std::process::exit(1);
            }
            let machine = machine_options(&target, cpu.as_deref(), &features, float_abi.as_deref())?;
            let context = inkwell::context::Context::create();
            let mut compiler =
                plcc_codegen::Compiler::new(&context, &inputs[0].display().to_string());
            compiler.set_machine_options(machine);
            for spec in &image_size {
                let (area, bytes) = parse_image_size(spec)?;
                compiler.set_image_size(area, bytes);
            }
            register_sources(&mut compiler, &parsed);
            let interval = plcc_codegen::compiler::contract::parse_duration_ns(&task_interval)
                .ok_or_else(|| {
                    miette::miette!("--task-interval: `{task_interval}` is not a TIME")
                })?;
            compiler.set_task_options(plcc_codegen::TaskOptions {
                default_interval_ns: interval,
            });

            if let Err(e) = compiler.compile(&parsed.unit) {
                report_codegen_error(&e, &inputs);
                std::process::exit(1);
            }
            // Every output (.ll, .bc, .o) carries the target's triple and data
            // layout, so a later optimizer lays structs out as the header says.
            compiler
                .set_target(&target)
                .map_err(|e| miette::miette!("{e}"))?;
            if opt_level > 0 {
                compiler
                    .optimize(&target, opt_level)
                    .map_err(|e| miette::miette!("{e}"))?;
            }

            if emit_header.is_some() || emit_symbols.is_some() {
                let mut contract = compiler
                    .runtime_contract(&target)
                    .map_err(|e| miette::miette!("{e}"))?;
                contract.device = dev.map(|d| plcc_codegen::DeviceStamp {
                    id: d.device.id.clone(),
                    version: d.device.version,
                });
                if let Some(path) = &emit_header {
                    let guard = format!(
                        "PLCC_{}_H",
                        path.file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("program")
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
            io_map,
        } => {
            if inputs.is_empty() {
                eprintln!("Usage: plcc sim <program.st> [--scans 0] [--modbus 502]");
                std::process::exit(1);
            }

            let parsed = parse_inputs(&inputs, stdlib, &l5x_options(&io_map)?)?;
            if !no_typecheck && run_typecheck(&parsed) {
                eprintln!("not run: fix the type errors, or pass --no-typecheck");
                std::process::exit(1);
            }
            let context = inkwell::context::Context::create();
            let mut compiler = plcc_codegen::Compiler::new(&context, "sim");
            register_sources(&mut compiler, &parsed);
            if let Err(e) = compiler.compile(&parsed.unit) {
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

            // A division by zero stops the simulated PLC (see plcc_fault_impl).
            compiler.use_external_fault_handler();
            let ee = compiler
                .module()
                .create_jit_execution_engine(inkwell::OptimizationLevel::None)
                .map_err(|e| miette::miette!("JIT error: {e}"))?;
            if let Some(f) = compiler.module().get_function("plcc_fault") {
                ee.add_global_mapping(&f, plcc_fault_impl as *const () as usize);
            }

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

/// `--cpu`, `--features` and `--float-abi` as LLVM's CPU and feature string.
fn machine_options(
    triple: &str,
    cpu: Option<&str>,
    features: &[String],
    float_abi: Option<&str>,
) -> Result<plcc_codegen::MachineOptions> {
    let mut feats: Vec<String> = Vec::new();
    for f in features.iter().map(|f| f.trim()).filter(|f| !f.is_empty()) {
        if !(f.starts_with('+') || f.starts_with('-')) || f.len() < 2 {
            miette::bail!("--features: `{f}` is not a target feature; write +name or -name");
        }
        feats.push(f.to_string());
    }
    if let Some(abi) = float_abi {
        let abi = plcc_device::FloatAbi::parse(abi).ok_or_else(|| {
            miette::miette!("--float-abi: `{abi}` is not soft, softfp or hard")
        })?;
        let extra = abi
            .features_for(triple)
            .map_err(|e| miette::miette!("--float-abi: {e}"))?;
        feats.extend(extra.into_iter().map(str::to_string));
    }
    Ok(plcc_codegen::MachineOptions {
        cpu: cpu.map(str::trim).filter(|c| !c.is_empty()).unwrap_or("generic").to_string(),
        features: feats.join(","),
    })
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
        other => {
            return Err(miette::miette!(
                "--image-size: unknown area `{other}` (I, Q or M)"
            ));
        }
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
        && input.is_file()
        && !plcc_twincat::is_project_file(input)
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

/// `plcc_fault` for `plcc sim`: a runtime fault stops the PLC. Report it and exit
/// with status 3 — the hook must not return.
extern "C" fn plcc_fault_impl(code: u32, site: *const std::ffi::c_char) {
    let site = if site.is_null() {
        String::new()
    } else {
        unsafe { std::ffi::CStr::from_ptr(site) }.to_string_lossy().into_owned()
    };
    let what = plcc_runtime::fault::FaultCode::from_code(code)
        .map(|c| c.describe())
        .unwrap_or("runtime fault");
    eprintln!("[PLC] FAULT {code} ({what}) at {site}: program stopped");
    std::process::exit(3);
}

/// Tell the compiler which file each POU of the merged unit came from, so a
/// runtime fault can name `file:line:col`.
fn register_sources(compiler: &mut plcc_codegen::Compiler<'_>, parsed: &Parsed) {
    let mut files: Vec<(&Origin, Vec<String>)> = Vec::new();
    for (decl, origin) in parsed.unit.declarations.iter().zip(&parsed.origins) {
        let Some(name) = declaration_name(decl) else { continue };
        match files
            .iter_mut()
            .find(|(o, _)| std::rc::Rc::ptr_eq(&o.source, &origin.source))
        {
            Some((_, pous)) => pous.push(name),
            None => files.push((origin, vec![name])),
        }
    }
    for (origin, pous) in files {
        if origin.logix {
            compiler.fault_on_array_bounds(&pous);
        }
        compiler.add_source_file(&origin.name, &origin.source, pous);
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
