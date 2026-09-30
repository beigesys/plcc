// SPDX-License-Identifier: MPL-2.0

//! `plcc convert`: one program, other notations.
//!
//! * `--to st` prints any input (ST, PLCopen XML, L5X, TwinCAT) as canonical
//!   Structured Text: the AST the front end produces, through
//!   [`plcc_st::print_unit`]. Ladder bodies print as the ST their rungs lower
//!   to, each rung's statements after a `(* rung N *)` comment.

use crate::{is_l5x_input, parse_file_with, print_parse_diagnostics, read_source};
use miette::{IntoDiagnostic, Result};
use std::path::{Path, PathBuf};

/// `--to` values.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    /// Structured Text
    St,
}

pub struct Args {
    pub inputs: Vec<PathBuf>,
    pub to: Format,
    pub output: Option<PathBuf>,
    pub prelude: bool,
}

pub fn run(args: Args) -> Result<()> {
    if args.inputs.is_empty() {
        miette::bail!("at least one input file is required");
    }
    let text = match args.to {
        Format::St => to_st(&args)?,
    };
    match &args.output {
        Some(p) => std::fs::write(p, text).into_diagnostic()?,
        None => print!("{text}"),
    }
    Ok(())
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
