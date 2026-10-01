// SPDX-License-Identifier: MPL-2.0

//! The plcc front-end pipeline on in-memory files, with no file system and no
//! LLVM: input dispatch (Structured Text, PLCopen XML, Rockwell L5X, TwinCAT
//! objects and `.plcproj` projects), the Logix prelude and bundled standard
//! library, the type checker, and structured diagnostics.
//!
//! This is the same pipeline `plcc check` runs (`crates/plcc-cli`), expressed
//! over a map of `path → text` so it can run in a browser (`plcc-wasm`) or any
//! other host that keeps files in memory. It builds for `wasm32-unknown-unknown`.

pub mod convert;
pub mod diag;
pub mod ladder;
pub mod tags;

pub use diag::{Diagnostic, Label, LineIndex, Position, Severity, Stage};
pub use plcc_l5x::Options as L5xOptions;
pub use plcc_ladder::model::Dialect;
pub use plcc_st::ast::CompilationUnit;

use plcc_st::ast::Declaration;
use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;

/// A set of source files and how to build them.
#[derive(Clone, Debug, Default)]
pub struct Project {
    /// `path → text`. Paths are relative and `/`-separated (`src/main.st`).
    pub files: BTreeMap<String, String>,
    /// The inputs, in order. `None`: every `.plcproj` if there is one (the
    /// first is the application, later ones libraries), otherwise every source
    /// file, in path order.
    pub entry: Option<Vec<String>>,
    /// TOML binding Logix tags to process-image addresses (docs/l5x.md,
    /// `--io-map`); used by L5X inputs.
    pub io_map: Option<String>,
    /// Compile the bundled IEC standard function blocks (`--stdlib bundled-st`,
    /// the default). `false` is `--stdlib none`.
    pub no_stdlib: bool,
}

/// Where a declaration of the merged unit came from.
#[derive(Clone, Debug)]
pub struct Origin {
    /// The caller's path, or the name of a bundled unit.
    pub name: String,
    pub source: Rc<String>,
    /// Part of the bundled standard library / Logix prelude.
    pub prelude: bool,
    /// Lowered from a Rockwell L5X project (Logix semantics).
    pub logix: bool,
}

/// The merged compilation unit and the origin of each declaration, index for
/// index: what a code generator needs next.
#[derive(Clone, Debug)]
pub struct Parsed {
    pub unit: CompilationUnit,
    pub origins: Vec<Origin>,
    /// Libraries TwinCAT projects reference that nothing provides.
    pub missing_libraries: Vec<String>,
}

/// Outcome of [`check`].
#[derive(Debug)]
pub struct Checked {
    /// No diagnostic is an error.
    pub ok: bool,
    pub diagnostics: Vec<Diagnostic>,
    /// The merged unit, unless the inputs could not be parsed.
    pub parsed: Option<Parsed>,
}

/// Whether a caller-supplied path is acceptable: relative, `/`-separated, no
/// empty, `.` or `..` components, no NUL or backslash, at most 512 bytes.
pub fn validate_path(path: &str) -> Result<(), String> {
    if path.is_empty() || path.len() > 512 {
        return Err(format!("`{path}`: a path must be 1 to 512 bytes"));
    }
    if path.starts_with('/') || path.contains('\\') || path.contains('\0') || path.contains(':') {
        return Err(format!(
            "`{path}`: paths must be relative and `/`-separated (no drive, backslash or NUL)"
        ));
    }
    if path.split('/').any(|c| c.is_empty() || c == "." || c == "..") {
        return Err(format!("`{path}`: empty, `.` and `..` path components are not allowed"));
    }
    Ok(())
}

pub(crate) fn extension(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((_, e)) => e.to_ascii_lowercase(),
        None => String::new(),
    }
}

/// Extensions read as program source when no entry list is given.
const SOURCE_EXTENSIONS: &[&str] = &[
    "st", "iecst", "xml", "l5x", "tcpou", "tcdut", "tcgvl", "tcio", "tctto",
];

pub(crate) fn is_l5x(path: &str, source: &str) -> bool {
    extension(path) == "l5x" || plcc_l5x::is_l5x(source)
}

fn is_twincat_object(path: &str, source: &str) -> bool {
    plcc_twincat::OBJECT_EXTENSIONS.contains(&extension(path).as_str())
        || plcc_twincat::is_twincat_object(source)
}

/// Name of a top-level declaration, uppercased, when it has one.
pub fn declaration_name(decl: &Declaration) -> Option<String> {
    use Declaration as D;
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

/// The file in `files` that `dir/include` names, ignoring case as TwinCAT
/// (a Windows tool) does.
fn resolve_include<'a>(
    files: &'a BTreeMap<String, String>,
    dir: &str,
    include: &str,
) -> Option<&'a str> {
    let want = if dir.is_empty() {
        include.to_string()
    } else {
        format!("{dir}/{include}")
    };
    if let Some((k, _)) = files.get_key_value(&want) {
        return Some(k);
    }
    let lower = want.to_lowercase();
    files
        .keys()
        .find(|k| k.to_lowercase() == lower)
        .map(String::as_str)
}

/// The expanded input list.
pub(crate) struct Inputs {
    /// (path, belongs to a library project)
    pub(crate) files: Vec<(String, bool)>,
    pub(crate) tasks: Vec<plcc_twincat::Task>,
    /// (library references, project name) of each TwinCAT project.
    references: Vec<(Vec<String>, String)>,
}

pub(crate) fn expand(project: &Project, diags: &mut Vec<Diagnostic>) -> Inputs {
    let entry: Vec<String> = match &project.entry {
        Some(e) => e.clone(),
        None => {
            let projects: Vec<String> = project
                .files
                .keys()
                .filter(|p| extension(p) == "plcproj")
                .cloned()
                .collect();
            if projects.is_empty() {
                project
                    .files
                    .keys()
                    .filter(|p| SOURCE_EXTENSIONS.contains(&extension(p).as_str()))
                    .cloned()
                    .collect()
            } else {
                projects
            }
        }
    };
    let mut out = Inputs {
        files: Vec::new(),
        tasks: Vec::new(),
        references: Vec::new(),
    };
    let mut projects = 0;
    for path in entry {
        let Some(source) = project.files.get(&path) else {
            diags.push(Diagnostic::plain(
                Some(&path),
                Stage::Input,
                Severity::Error,
                format!("entry `{path}` is not one of the files"),
            ));
            continue;
        };
        if extension(&path) != "plcproj" {
            add_file(project, &mut out, &path, false, diags);
            continue;
        }
        projects += 1;
        let library = projects > 1;
        let stem = path
            .rsplit('/')
            .next()
            .and_then(|n| n.rsplit_once('.').map(|(s, _)| s.to_string()))
            .unwrap_or_default();
        out.references
            .push((plcc_twincat::project_libraries(source), stem));
        let dir = path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        match plcc_twincat::project_includes(source) {
            Ok(items) => {
                for item in items {
                    match resolve_include(&project.files, dir, &item.slash_path()) {
                        Some(file) => add_file(project, &mut out, file, library, diags),
                        None => diags.push(Diagnostic::from_miette(
                            &item.missing(),
                            &path,
                            source,
                            Stage::Twincat,
                            None,
                        )),
                    }
                }
            }
            Err(e) => diags.push(Diagnostic::from_miette(&e, &path, source, Stage::Twincat, None)),
        }
    }
    out
}

fn add_file(project: &Project, out: &mut Inputs, path: &str, library: bool, diags: &mut Vec<Diagnostic>) {
    if extension(path) != "tctto" {
        out.files.push((path.to_string(), library));
        return;
    }
    if library {
        return;
    }
    let source = &project.files[path];
    match plcc_twincat::parse_task(source) {
        Ok(Some(task)) => out.tasks.push(task),
        Ok(None) => {}
        Err(e) => diags.push(Diagnostic::from_miette(&e, path, source, Stage::Twincat, None)),
    }
}

/// Parse one file with the reader its extension or root element calls for.
/// Returns the unit and its diagnostics.
pub fn parse_file(
    path: &str,
    source: &str,
    l5x: &plcc_l5x::Options,
) -> (CompilationUnit, Vec<Diagnostic>) {
    parse_file_with(path, source, l5x, false)
}

/// [`parse_file`]; with `annotate`, ladder and FBD bodies (L5X, PLCopen) put a
/// `(* rung N *)` comment before each rung's statements, for printing the
/// lowered ST (`plcc convert --to st`).
pub fn parse_file_with(
    path: &str,
    source: &str,
    l5x: &plcc_l5x::Options,
    annotate: bool,
) -> (CompilationUnit, Vec<Diagnostic>) {
    if is_twincat_object(path, source) {
        let (unit, errors) = plcc_twincat::parse(source);
        let d = errors
            .iter()
            .map(|e| {
                let sev = e.is_warning().then_some(Severity::Warning);
                Diagnostic::from_miette(e, path, source, Stage::Twincat, sev)
            })
            .collect();
        return (unit, d);
    }
    if is_l5x(path, source) {
        let (unit, errors) = if annotate {
            plcc_l5x::parse_annotated(source, l5x)
        } else {
            plcc_l5x::parse_with(source, l5x)
        };
        let d = errors
            .iter()
            .map(|e| {
                let sev = Some(if e.is_warning() { Severity::Warning } else { Severity::Error });
                Diagnostic::from_miette(e, path, source, Stage::L5x, sev)
            })
            .collect();
        return (unit, d);
    }
    if extension(path) == "xml" || plcc_plcopen::is_plcopen(source) {
        let opts = plcc_plcopen::Options {
            annotate_rungs: annotate,
        };
        let (unit, errors) = plcc_plcopen::parse_with(source, &opts);
        let d = errors
            .iter()
            .map(|e| Diagnostic::from_miette(e, path, source, Stage::Plcopen, None))
            .collect();
        return (unit, d);
    }
    let (unit, errors) = plcc_st::parse(source);
    let d = errors
        .iter()
        .map(|e| Diagnostic::from_miette(e, path, source, Stage::Parse, None))
        .collect();
    (unit, d)
}

fn has_error(diags: &[Diagnostic]) -> bool {
    diags.iter().any(|d| d.severity == Severity::Error)
}

/// Read every input into one compilation unit, as `plcc check` does before
/// type checking: the application's files, library projects (whose
/// declarations the application already has are left out), the TwinCAT task
/// configuration, the Logix prelude for L5X inputs and the bundled standard
/// library (user declarations of the same name win).
///
/// Returns `None` for the unit when a file could not be parsed.
pub fn parse_project(project: &Project) -> (Option<Parsed>, Vec<Diagnostic>) {
    let mut diags = Vec::new();
    for path in project.files.keys() {
        if let Err(e) = validate_path(path) {
            diags.push(Diagnostic::plain(None, Stage::Input, Severity::Error, e));
        }
    }
    if has_error(&diags) {
        return (None, diags);
    }
    let mut l5x = plcc_l5x::Options::default();
    if let Some(text) = &project.io_map {
        match plcc_l5x::IoMap::parse(text) {
            Ok(map) => l5x.io_map = map,
            Err(e) => {
                diags.push(Diagnostic::plain(
                    Some("<io_map>"),
                    Stage::IoMap,
                    Severity::Error,
                    format!("io_map: {e}"),
                ));
                return (None, diags);
            }
        }
    }
    let inputs = expand(project, &mut diags);
    if inputs.files.is_empty() && !has_error(&diags) {
        diags.push(Diagnostic::plain(
            None,
            Stage::Input,
            Severity::Error,
            "no source files: give .st, PLCopen .xml, .L5X or TwinCAT files".to_string(),
        ));
    }

    let mut declarations = Vec::new();
    let mut origins = Vec::new();
    let mut any_l5x = false;
    let mut defined: HashSet<String> = HashSet::new();
    for (path, library) in &inputs.files {
        let mut source = &project.files[path];
        let lowered;
        let (logix, unit) = if ladder::is_model_path(path) {
            // A ladder model: as L5X (Logix) or lowered to ST (IEC).
            lowered = match ladder::lower(path, source) {
                Ok((lowered, d)) => {
                    diags.extend(d);
                    lowered
                }
                Err(d) => {
                    diags.extend(d);
                    continue;
                }
            };
            match &lowered.l5x {
                Some(text) => {
                    let mut opts = l5x.clone();
                    for (tag, addr) in &lowered.io {
                        if let Err(e) = opts.io_map.add(tag, addr) {
                            let mut d =
                                Diagnostic::plain(Some(path), Stage::IoMap, Severity::Error, e);
                            d.ladder = Some(ladder::LadderRef {
                                tag: Some(tag.clone()),
                                ..Default::default()
                            });
                            diags.push(d);
                        }
                    }
                    let (unit, mut file_diags) = parse_file(path, text, &opts);
                    ladder::remap(&mut file_diags, path, &lowered);
                    diags.extend(file_diags);
                    source = text;
                    (true, unit)
                }
                None => {
                    let (unit, errs) = plcc_ladder::to_unit(&lowered.model, false);
                    for e in errs {
                        let mut d = Diagnostic::plain(
                            Some(path),
                            Stage::Convert,
                            Severity::Error,
                            format!("the ladder model does not lower: {e}"),
                        );
                        d.ladder = Some(ladder::LadderRef::default());
                        diags.push(d);
                    }
                    (false, unit)
                }
            }
        } else {
            let logix = is_l5x(path, source);
            let (unit, file_diags) = parse_file(path, source, &l5x);
            diags.extend(file_diags);
            (logix, unit)
        };
        any_l5x |= logix;
        let origin = Origin {
            name: path.clone(),
            source: Rc::new(source.clone()),
            prelude: false,
            logix,
        };
        for decl in unit.declarations {
            let name = declaration_name(&decl);
            if *library && name.as_ref().is_some_and(|n| defined.contains(n)) {
                continue;
            }
            defined.extend(name);
            origins.push(origin.clone());
            declarations.push(decl);
        }
    }
    if has_error(&diags) {
        return (None, diags);
    }

    // TwinCAT's task configuration, unless the sources declare their own.
    let has_config = declarations
        .iter()
        .any(|d| matches!(d, Declaration::Configuration(_)));
    if !has_config && let Some(text) = plcc_twincat::configuration_source(&inputs.tasks) {
        let (unit, errors) = plcc_st::parse(&text);
        if errors.is_empty() {
            let origin = Origin {
                name: "<TwinCAT task configuration>".to_string(),
                source: Rc::new(text),
                prelude: false,
                logix: false,
            };
            origins.extend(std::iter::repeat_n(origin, unit.declarations.len()));
            declarations.extend(unit.declarations);
        } else {
            diags.push(Diagnostic::plain(
                None,
                Stage::Twincat,
                Severity::Warning,
                "the TwinCAT task configuration could not be used; programs run in the \
                 default task"
                    .to_string(),
            ));
        }
    }

    let mut prefix: Vec<(Declaration, Origin)> = Vec::new();
    if any_l5x {
        let src = plcc_l5x::prelude();
        let (unit, errors) = plcc_st::parse(&src);
        if let Some(e) = errors.first() {
            diags.push(Diagnostic::from_miette(e, plcc_l5x::PRELUDE_NAME, &src, Stage::Parse, None));
            return (None, diags);
        }
        let origin = Origin {
            name: plcc_l5x::PRELUDE_NAME.to_string(),
            source: Rc::new(src),
            prelude: true,
            logix: true,
        };
        prefix.extend(unit.declarations.into_iter().map(|d| (d, origin.clone())));
    }
    if !project.no_stdlib {
        // User declarations of the same name replace the bundled ones.
        // (The Logix prelude counts as user code here, as in `plcc check`.)
        let user: HashSet<String> = declarations
            .iter()
            .chain(prefix.iter().map(|(d, _)| d))
            .filter_map(declaration_name)
            .collect();
        let mut stdlib = Vec::new();
        for u in plcc_stdlib::UNITS {
            let (unit, errors) = plcc_st::parse(u.source);
            if let Some(e) = errors.first() {
                diags.push(Diagnostic::from_miette(e, u.name, u.source, Stage::Parse, None));
                return (None, diags);
            }
            let origin = Origin {
                name: u.name.to_string(),
                source: Rc::new(u.source.to_string()),
                prelude: true,
                logix: false,
            };
            for d in unit.declarations {
                if !declaration_name(&d).is_some_and(|n| user.contains(&n)) {
                    stdlib.push((d, origin.clone()));
                }
            }
        }
        // Standard library first, then the Logix prelude (which may use it).
        stdlib.extend(prefix);
        prefix = stdlib;
    }
    let (pre_decls, pre_origins): (Vec<_>, Vec<_>) = prefix.into_iter().unzip();
    let declarations: Vec<Declaration> = pre_decls.into_iter().chain(declarations).collect();
    let origins: Vec<Origin> = pre_origins.into_iter().chain(origins).collect();

    let given: Vec<String> = inputs
        .references
        .iter()
        .map(|(_, n)| n.to_uppercase())
        .collect();
    let mut missing_libraries: Vec<String> = Vec::new();
    for lib in inputs.references.iter().flat_map(|(l, _)| l) {
        if !plcc_hir::libraries::provided(lib)
            && !given.contains(&lib.to_uppercase())
            && !missing_libraries.iter().any(|m| m.eq_ignore_ascii_case(lib))
        {
            missing_libraries.push(lib.clone());
        }
    }
    let parsed = Parsed {
        unit: CompilationUnit {
            declarations,
            span: plcc_st::span::Span::empty(),
        },
        origins,
        missing_libraries,
    };
    (Some(parsed), diags)
}

/// Run the type checker over a parsed project. Warnings in the bundled
/// libraries are dropped (they are not the user's problem).
pub fn typecheck(parsed: &Parsed) -> Vec<Diagnostic> {
    let (_symbols, located) = plcc_hir::TypeChecker::new().check_located(&parsed.unit);
    let mut out = Vec::new();
    for (index, diag) in located {
        let Some(origin) = parsed.origins.get(index) else {
            continue;
        };
        let warning = diag.is_warning();
        if warning && origin.prelude {
            continue;
        }
        let sev = Some(if warning { Severity::Warning } else { Severity::Error });
        out.push(Diagnostic::from_miette(
            &diag,
            &origin.name,
            &origin.source,
            Stage::Typecheck,
            sev,
        ));
    }
    if has_error(&out) && !parsed.missing_libraries.is_empty() {
        out.push(Diagnostic::plain(
            None,
            Stage::Typecheck,
            Severity::Advice,
            format!(
                "the TwinCAT project references libraries plcc does not provide: {} (add \
                 the .plcproj of an open-source library after the application's)",
                parsed.missing_libraries.join(", ")
            ),
        ));
    }
    out
}

/// Trace diagnostics in ladder model inputs (reported against the L5X they
/// were written as) back to the model.
pub fn remap_ladder(project: &Project, diags: &mut [Diagnostic]) {
    for (path, source) in &project.files {
        if ladder::is_model_path(path)
            && diags.iter().any(|d| d.file.as_deref() == Some(path.as_str()))
            && let Ok((lowered, _)) = ladder::lower(path, source)
        {
            ladder::remap(diags, path, &lowered);
        }
    }
}

/// Parse and type-check: exactly what `plcc check` decides.
pub fn check(project: &Project) -> Checked {
    let (parsed, mut diagnostics) = parse_project(project);
    if let Some(p) = &parsed {
        let mut found = typecheck(p);
        remap_ladder(project, &mut found);
        diagnostics.extend(found);
    }
    Checked {
        ok: parsed.is_some() && !has_error(&diagnostics),
        diagnostics,
        parsed,
    }
}
