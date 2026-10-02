// SPDX-License-Identifier: MPL-2.0

//! `plcc convert`: one program, other notations.
//!
//! * `--to st` prints any input (ST, PLCopen XML, L5X, TwinCAT, ladder JSON)
//!   as canonical Structured Text: the AST the front end produces, through
//!   [`plcc_st::print_unit`]. Ladder bodies print as the ST their rungs lower
//!   to, each rung's statements after a `(* rung N *)` comment.
//! * `--to ladder-json | plcopen | l5x` go through the ladder model of
//!   `plcc-ladder`: PLCopen LD and L5X are read into it (a `.json` input is a
//!   model already; an `.st` input is converted, its drawable statements as
//!   rungs), translated to the dialect of the output when it differs
//!   (`--dialect` for ladder-json), and written.

use crate::{is_l5x_input, parse_file_with, print_parse_diagnostics, read_source};
use miette::{IntoDiagnostic, Result};
use plcc_ladder::model::{Dialect, Project};
use std::path::{Path, PathBuf};

/// `--to` values.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    /// Structured Text
    St,
    /// PLCopen XML (TC6 v2.01) with LD bodies
    Plcopen,
    /// Rockwell L5X with RLL routines
    L5x,
    /// The ladder model as JSON (plcc-ladder)
    LadderJson,
}

/// `--dialect` values.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum DialectOpt {
    Iec,
    Logix,
}

impl From<DialectOpt> for Dialect {
    fn from(d: DialectOpt) -> Dialect {
        match d {
            DialectOpt::Iec => Dialect::Iec,
            DialectOpt::Logix => Dialect::Logix,
        }
    }
}

pub struct Args {
    pub inputs: Vec<PathBuf>,
    pub to: Format,
    pub output: Option<PathBuf>,
    pub prelude: bool,
    pub dialect: Option<DialectOpt>,
}

fn joined<E: std::fmt::Display>(errs: &[E]) -> String {
    errs.iter()
        .map(|e| format!("  {e}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn run(args: Args) -> Result<()> {
    if args.inputs.is_empty() {
        miette::bail!("at least one input file is required");
    }
    let text = match args.to {
        Format::St if !args.inputs.iter().any(|p| is_json(p)) && args.dialect.is_none() => {
            to_st(&args)?
        }
        Format::St => {
            let mut model = load_model(&args)?;
            if let Some(d) = args.dialect {
                model = translate(model, d.into());
            }
            model_to_st(&model, args.prelude)?
        }
        Format::LadderJson => {
            let mut model = load_model(&args)?;
            if let Some(d) = args.dialect {
                model = translate(model, d.into());
            }
            model.to_json() + "\n"
        }
        Format::Plcopen => {
            let model = translate(load_model(&args)?, Dialect::Iec);
            let (text, warnings) = plcc_plcopen::ladder::write(&model)
                .map_err(|errs| miette::miette!("cannot write PLCopen XML:\n{}", joined(&errs)))?;
            for w in warnings {
                eprintln!("warning: {w}");
            }
            text
        }
        Format::L5x => {
            let model = translate(load_model(&args)?, Dialect::Logix);
            let (text, warnings) = plcc_l5x::ladder::write(&model)
                .map_err(|errs| miette::miette!("cannot write L5X:\n{}", joined(&errs)))?;
            for w in warnings {
                eprintln!("warning: {w}");
            }
            text
        }
    };
    match &args.output {
        Some(p) => std::fs::write(p, text).into_diagnostic()?,
        None => print!("{text}"),
    }
    Ok(())
}

fn is_json(p: &Path) -> bool {
    p.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("json"))
}

/// Translate between dialects, printing each warning.
fn translate(model: Project, to: Dialect) -> Project {
    if model.dialect == to {
        return model;
    }
    let (out, warnings) = plcc_l5x::ladder::translate(&model, to);
    for w in &warnings {
        eprintln!("warning: {w}");
    }
    out
}

/// The single input as a ladder model.
fn load_model(args: &Args) -> Result<Project> {
    let [input] = args.inputs.as_slice() else {
        miette::bail!("ladder conversions take one input file");
    };
    let source = read_source(input)?;
    let name = input.display().to_string();
    if is_json(input) {
        return Project::from_json(&source)
            .map_err(|e| miette::miette!("{name}: not a ladder model: {e}"));
    }
    if is_l5x_input(input, &source) {
        let (model, errs) = plcc_l5x::ladder::read(&source);
        let failed = errs.iter().any(|e| !e.is_warning());
        print_parse_diagnostics(
            input,
            &source,
            errs.into_iter().map(miette::Report::new).collect(),
        );
        return match model {
            Some(m) if !failed => Ok(m),
            _ => std::process::exit(1),
        };
    }
    if input
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("xml"))
        || plcc_plcopen::is_plcopen(&source)
    {
        let (model, errs) = plcc_plcopen::ladder::read(&source);
        let failed = !errs.is_empty();
        print_parse_diagnostics(
            input,
            &source,
            errs.into_iter().map(miette::Report::new).collect(),
        );
        return match model {
            Some(m) if !failed => Ok(m),
            _ => std::process::exit(1),
        };
    }
    // Structured Text: the drawable subset becomes rungs.
    let (unit, errs) = plcc_st::parse(&source);
    let failed = !errs.is_empty();
    print_parse_diagnostics(
        input,
        &source,
        errs.into_iter().map(miette::Report::new).collect(),
    );
    if failed {
        std::process::exit(1);
    }
    let (model, notes) = plcc_ladder::from_st::from_unit(&unit);
    for n in notes {
        eprintln!("note: {n}");
    }
    Ok(model)
}

/// A model as Structured Text: IEC through the model lowering, Logix through
/// the L5X it writes.
fn model_to_st(model: &Project, prelude: bool) -> Result<String> {
    match model.dialect {
        Dialect::Iec => {
            let (unit, errs) = plcc_ladder::to_unit(model, true);
            if !errs.is_empty() {
                miette::bail!("the ladder model does not lower:\n{}", joined(&errs));
            }
            Ok(plcc_st::print_unit(&unit))
        }
        Dialect::Logix => {
            let (l5x, _) = plcc_l5x::ladder::write(model)
                .map_err(|errs| miette::miette!("{}", joined(&errs)))?;
            let (unit, errs) = plcc_l5x::parse_annotated(&l5x, &plcc_l5x::Options::default());
            if errs.iter().any(|e| !e.is_warning()) {
                miette::bail!("the Logix model does not lower:\n{}", joined(&errs));
            }
            // The text is not complete without the prelude: say what it holds.
            let mut out = String::from(plcc_l5x::PRELUDE_NOTE);
            out.push_str(&plcc_st::print_unit(&unit));
            if prelude {
                out.push_str("\n(* ---- Logix prelude (plcc-l5x) ---- *)\n");
                out.push_str(&plcc_l5x::prelude());
            }
            Ok(out)
        }
    }
}

fn to_st(args: &Args) -> Result<String> {
    let inputs = crate::expand_inputs(&args.inputs)?;
    let opts = plcc_l5x::Options::default();
    let mut out = String::new();
    let mut failed = false;
    let mut any_l5x = false;
    let mut all = Vec::new();
    for (file, _) in &inputs.files {
        let source = read_source(file)?;
        let logix = is_l5x_input(file, &source);
        any_l5x |= logix;
        let (unit, reports, file_failed) = parse_file_with(file, &source, &opts, true);
        let printed_failed = print_parse_diagnostics(file, &source, reports);
        if if logix { file_failed } else { printed_failed } {
            failed = true;
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&header(file, logix));
        out.push_str(&plcc_st::print_unit(&unit));
        all.extend(unit.declarations);
    }
    if failed {
        std::process::exit(1);
    }
    // TwinCAT tasks (.TcTTO) are a CONFIGURATION in ST.
    if let Some((_, unit)) = crate::twincat_configuration(&inputs, &all) {
        out.push_str("\n(* TwinCAT task configuration *)\n");
        out.push_str(&plcc_st::print_unit(&unit));
    }
    if any_l5x && args.prelude {
        out.push_str("\n(* ---- Logix prelude (plcc-l5x) ---- *)\n");
        out.push_str(&plcc_l5x::prelude());
    }
    Ok(out)
}

fn header(file: &Path, logix: bool) -> String {
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut h = format!("(* Converted by plcc from {name}. *)\n");
    if logix {
        h.push_str(
            "(* Logix instructions are calls into the Logix prelude (lx__ton, lx__put_DINT_i, ...;\n   \
             docs/l5x.md \"Reading the generated ST\"); `--prelude` appends it. *)\n",
        );
    }
    h.push('\n');
    h
}
