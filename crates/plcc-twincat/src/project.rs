// SPDX-License-Identifier: MPL-2.0

//! TwinCAT PLC projects: `.plcproj` files, project directories, and task
//! configuration (`.TcTTO`).

use crate::error::TwinCatError;
use plcc_st::Span;
use std::path::{Path, PathBuf};

/// Extensions of the TwinCAT object files plcc reads (lowercase).
pub const OBJECT_EXTENSIONS: &[&str] = &["tcpou", "tcdut", "tcgvl", "tcio", "tctto"];

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Whether `path` is a TwinCAT object file (`.TcPOU`, `.TcDUT`, `.TcGVL`,
/// `.TcIO`, `.TcTTO`).
pub fn is_object_file(path: &Path) -> bool {
    OBJECT_EXTENSIONS.contains(&extension(path).as_str())
}

/// Whether `path` is a TwinCAT task configuration (`.TcTTO`).
pub fn is_task_file(path: &Path) -> bool {
    extension(path) == "tctto"
}

/// Whether `path` is a TwinCAT PLC project file (`.plcproj`).
pub fn is_project_file(path: &Path) -> bool {
    extension(path) == "plcproj"
}

/// Directories TwinCAT generates next to a project; never source.
const GENERATED_DIRS: &[&str] = &["_boot", "_compileinfo", "_libraries", ".git"];

/// The object files a `.plcproj` compiles: its `<Compile Include="...">` items
/// with a TwinCAT object extension, in project order. Other items
/// (visualizations, text lists, image pools) hold no code and are skipped.
pub fn project_files(project: &Path, source: &str) -> Result<Vec<PathBuf>, TwinCatError> {
    let doc = roxmltree::Document::parse(source).map_err(|e| {
        let at = crate::byte_offset(source, e.pos());
        TwinCatError::new(format!("malformed .plcproj: {e}"), Span::new(at, at))
    })?;
    let dir = project.parent().unwrap_or(Path::new("."));
    // `<Folder Include="A\B"><ExcludeFromBuild>true</ExcludeFromBuild>`:
    // everything below the folder is left out of the build, unless a deeper
    // folder or the file itself says otherwise.
    let folders: Vec<(String, bool)> = doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "Folder")
        .filter_map(|n| Some((normalize_include(n.attribute("Include")?), exclude_flag(n)?)))
        .collect();
    let mut files = Vec::new();
    for item in doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "Compile")
    {
        let Some(include) = item.attribute("Include") else {
            continue;
        };
        // MSBuild paths use backslashes.
        let rel: PathBuf = include.split(['\\', '/']).collect();
        if !is_object_file(&rel) {
            continue;
        }
        if excluded(&normalize_include(include), exclude_flag(item), &folders) {
            continue;
        }
        // TwinCAT runs on Windows, whose paths ignore case: `Constants.TcGVL`
        // may name `CONSTANTS.TcGVL`.
        let path = resolve_case_insensitive(dir, &rel).unwrap_or_else(|| dir.join(&rel));
        if !path.exists() {
            let r = item.range();
            return Err(TwinCatError::new(
                format!("the project includes `{include}`, which does not exist"),
                Span::new(r.start, r.end),
            ));
        }
        files.push(path);
    }
    Ok(files)
}

/// An MSBuild `Include` path as a comparable key: forward slashes, lowercase
/// (TwinCAT runs on Windows), no trailing separator.
fn normalize_include(include: &str) -> String {
    include
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
}

/// The item's `<ExcludeFromBuild>` setting, when it has one.
fn exclude_flag(item: roxmltree::Node) -> Option<bool> {
    item.children()
        .find(|c| c.is_element() && c.tag_name().name() == "ExcludeFromBuild")
        .and_then(|c| c.text())
        .map(|t| t.trim().eq_ignore_ascii_case("true"))
}

/// Whether a `<Compile>` item is left out of the build: its own
/// `<ExcludeFromBuild>`, else that of the deepest enclosing folder that sets it.
fn excluded(file: &str, own: Option<bool>, folders: &[(String, bool)]) -> bool {
    if let Some(own) = own {
        return own;
    }
    folders
        .iter()
        .filter(|(folder, _)| {
            file.len() > folder.len()
                && file.starts_with(folder.as_str())
                && file.as_bytes()[folder.len()] == b'/'
        })
        .max_by_key(|(folder, _)| folder.len())
        .is_some_and(|(_, flag)| *flag)
}

/// `dir/rel`, matching each component without regard to case when the exact
/// spelling does not exist.
fn resolve_case_insensitive(dir: &Path, rel: &Path) -> Option<PathBuf> {
    let mut cur = dir.to_path_buf();
    for comp in rel.components() {
        let want = comp.as_os_str();
        let exact = cur.join(want);
        if exact.exists() {
            cur = exact;
            continue;
        }
        let want = want.to_string_lossy().to_lowercase();
        let found = std::fs::read_dir(&cur).ok()?.find_map(|e| {
            let e = e.ok()?;
            (e.file_name().to_string_lossy().to_lowercase() == want).then(|| e.path())
        })?;
        cur = found;
    }
    Some(cur)
}

/// Libraries a `.plcproj` references (`<PlaceholderReference>` and
/// `<LibraryReference>`), by name.
pub fn project_libraries(source: &str) -> Vec<String> {
    let Ok(doc) = roxmltree::Document::parse(source) else {
        return Vec::new();
    };
    doc.descendants()
        .filter(|n| {
            n.is_element()
                && matches!(
                    n.tag_name().name(),
                    "PlaceholderReference" | "LibraryReference"
                )
        })
        .filter_map(|n| n.attribute("Include"))
        .map(|inc| inc.split(',').next().unwrap_or(inc).trim().to_string())
        .collect()
}

/// What a directory argument stands for.
pub enum DirectoryInput {
    /// The directory holds exactly one `.plcproj`: compile that project.
    Project(PathBuf),
    /// No `.plcproj`: every object file below the directory, sorted.
    Files(Vec<PathBuf>),
}

/// Resolve a directory argument. More than one `.plcproj` below it is an
/// error (a repository often holds a library and its test project, which
/// declare the same POUs): name the one to build.
pub fn directory_input(dir: &Path) -> Result<DirectoryInput, String> {
    let mut objects = Vec::new();
    let mut projects = Vec::new();
    walk(dir, &mut objects, &mut projects).map_err(|e| format!("{}: {e}", dir.display()))?;
    match projects.len() {
        0 => {
            objects.sort();
            Ok(DirectoryInput::Files(objects))
        }
        1 => Ok(DirectoryInput::Project(projects.remove(0))),
        _ => {
            projects.sort();
            Err(format!(
                "{} holds {} TwinCAT projects; pass the .plcproj to build:\n{}",
                dir.display(),
                projects.len(),
                projects
                    .iter()
                    .map(|p| format!("  {}", p.display()))
                    .collect::<Vec<_>>()
                    .join("\n")
            ))
        }
    }
}

fn walk(
    dir: &Path,
    objects: &mut Vec<PathBuf>,
    projects: &mut Vec<PathBuf>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !GENERATED_DIRS.contains(&name.as_str()) {
                walk(&path, objects, projects)?;
            }
        } else if is_project_file(&path) {
            projects.push(path);
        } else if is_object_file(&path) {
            objects.push(path);
        }
    }
    Ok(())
}

/// A TwinCAT task (`.TcTTO`).
#[derive(Debug, Clone)]
pub struct Task {
    pub name: String,
    /// `<CycleTime>`, in microseconds.
    pub cycle_time_us: Option<u64>,
    /// `<Priority>`; lower is more urgent, as for IEC tasks.
    pub priority: Option<u32>,
    /// The programs the task calls (`<PouCall><Name>`), in order.
    pub programs: Vec<String>,
}

/// Read the task of a `.TcTTO` file.
pub fn parse_task(source: &str) -> Result<Option<Task>, TwinCatError> {
    let doc = roxmltree::Document::parse(source).map_err(|e| {
        let at = crate::byte_offset(source, e.pos());
        TwinCatError::new(format!("malformed XML: {e}"), Span::new(at, at))
    })?;
    let Some(task) = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "Task")
    else {
        return Ok(None);
    };
    let text = |tag: &str| {
        task.children()
            .find(|c| c.is_element() && c.tag_name().name() == tag)
            .and_then(|c| c.text())
            .map(|t| t.trim().to_string())
    };
    let programs = task
        .children()
        .filter(|c| c.is_element() && c.tag_name().name() == "PouCall")
        .filter_map(|c| {
            c.children()
                .find(|n| n.is_element() && n.tag_name().name() == "Name")
                .and_then(|n| n.text())
                .map(|t| t.trim().to_string())
        })
        .collect();
    Ok(Some(Task {
        name: task.attribute("Name").unwrap_or("PlcTask").to_string(),
        cycle_time_us: text("CycleTime").and_then(|t| t.parse().ok()),
        priority: text("Priority").and_then(|t| t.parse().ok()),
        programs,
    }))
}

/// Whether a name can be written in ST as it is.
fn is_st_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// An IEC CONFIGURATION equivalent to TwinCAT's task configuration: one
/// RESOURCE `PLC` with a TASK per `.TcTTO` (`INTERVAL` from the cycle time,
/// `PRIORITY` as given) and a program instance, named like the program, for
/// every `PouCall`. `None` when no task calls a program.
///
/// A task without a cycle time runs free-wheeling (`INTERVAL := T#0ms`).
pub fn configuration_source(tasks: &[Task]) -> Option<String> {
    if tasks.iter().all(|t| t.programs.is_empty()) {
        return None;
    }
    let mut out = String::from("CONFIGURATION TwinCAT\n  RESOURCE PLC\n");
    let mut used: Vec<String> = Vec::new();
    for t in tasks {
        if !is_st_identifier(&t.name) {
            continue;
        }
        let us = t.cycle_time_us.unwrap_or(0);
        let interval = if us % 1000 == 0 {
            format!("T#{}ms", us / 1000)
        } else {
            format!("T#{}us", us)
        };
        out.push_str(&format!(
            "    TASK {} (INTERVAL := {}, PRIORITY := {});\n",
            t.name,
            interval,
            t.priority.unwrap_or(20)
        ));
    }
    for t in tasks {
        if !is_st_identifier(&t.name) {
            continue;
        }
        for p in &t.programs {
            if !is_st_identifier(p) || used.iter().any(|u| u.eq_ignore_ascii_case(p)) {
                continue;
            }
            used.push(p.clone());
            out.push_str(&format!("    PROGRAM {p} WITH {} : {p};\n", t.name));
        }
    }
    out.push_str("  END_RESOURCE\nEND_CONFIGURATION\n");
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excluded_files_and_folders_are_not_compiled() {
        let dir = std::env::temp_dir().join(format!("plcc-twincat-exclude-{}", std::process::id()));
        for sub in ["POUs/Off/Deeper", "POUs/On"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        for f in ["POUs/A.TcPOU", "POUs/B.TcPOU", "POUs/Off/C.TcPOU", "POUs/Off/Deeper/D.TcPOU", "POUs/On/E.TcPOU"] {
            std::fs::write(dir.join(f), "").unwrap();
        }
        let proj = r#"<Project xmlns="http://schemas.microsoft.com/developer/msbuild/2003">
  <ItemGroup>
    <Compile Include="POUs\A.TcPOU" />
    <Compile Include="POUs\B.TcPOU"><ExcludeFromBuild>true</ExcludeFromBuild></Compile>
    <Compile Include="POUs\Off\C.TcPOU" />
    <Compile Include="POUs\Off\Deeper\D.TcPOU"><ExcludeFromBuild>false</ExcludeFromBuild></Compile>
    <Compile Include="POUs\On\E.TcPOU" />
  </ItemGroup>
  <ItemGroup>
    <Folder Include="POUs\off"><ExcludeFromBuild>true</ExcludeFromBuild></Folder>
    <Folder Include="POUs\On"><ExcludeFromBuild>false</ExcludeFromBuild></Folder>
  </ItemGroup>
</Project>"#;
        let files = project_files(&dir.join("P.plcproj"), proj).unwrap();
        let names: Vec<String> = files
            .iter()
            .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(names, ["A.TcPOU", "D.TcPOU", "E.TcPOU"]);
    }

    #[test]
    fn task_file_becomes_a_configuration() {
        let tto = r#"<?xml version="1.0" encoding="utf-8"?>
<TcPlcObject Version="1.1.0.1">
  <Task Name="PlcTask" Id="{x}">
    <CycleTime>10000</CycleTime>
    <Priority>20</Priority>
    <PouCall><Name>MAIN</Name></PouCall>
  </Task>
</TcPlcObject>"#;
        let task = parse_task(tto).unwrap().unwrap();
        assert_eq!(task.name, "PlcTask");
        assert_eq!(task.cycle_time_us, Some(10000));
        assert_eq!(task.programs, vec!["MAIN".to_string()]);
        let cfg = configuration_source(&[task]).unwrap();
        assert!(cfg.contains("TASK PlcTask (INTERVAL := T#10ms, PRIORITY := 20);"));
        assert!(cfg.contains("PROGRAM MAIN WITH PlcTask : MAIN;"));
        let (_, errors) = plcc_st::parse(&cfg);
        assert!(errors.is_empty(), "{errors:?}");
    }
}
