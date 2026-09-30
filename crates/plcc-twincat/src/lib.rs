// SPDX-License-Identifier: MPL-2.0

//! Beckhoff TwinCAT 3 front end.
//!
//! TwinCAT stores a PLC project as a `.plcproj` (an MSBuild file listing the
//! project's objects) and one XML file per object:
//!
//! * `.TcPOU` — a PROGRAM, FUNCTION_BLOCK or FUNCTION: its declaration and ST
//!   body as CDATA, plus nested `<Method>`, `<Property>` (`<Get>`/`<Set>`) and
//!   `<Action>` elements;
//! * `.TcDUT` — a `TYPE ... END_TYPE`;
//! * `.TcGVL` — a global variable list (`VAR_GLOBAL` blocks), whose name
//!   qualifies its variables (`GVL.x`);
//! * `.TcIO` — an INTERFACE with its method and property prototypes;
//! * `.TcTTO` — a task (cycle time, priority, the programs it calls).
//!
//! Each object is reassembled into ST text and parsed by plcc-st (see
//! [`text`]), so the result is the same AST `.st` files produce, and every span
//! in it is a byte range of the TwinCAT file. Only ST bodies compile: an LD,
//! FBD, CFC or SFC body is reported by POU name.

mod error;
mod object;
mod project;
mod text;

pub use error::TwinCatError;
pub use object::parse;
pub use project::{
    DirectoryInput, OBJECT_EXTENSIONS, Task, configuration_source, directory_input, is_object_file,
    ProjectInclude, is_project_file, is_task_file, parse_task, project_files, project_includes,
    project_libraries,
};

/// Whether `source` looks like a TwinCAT PLC object (root element
/// `<TcPlcObject>`), for inputs whose extension says nothing.
pub fn is_twincat_object(source: &str) -> bool {
    let s = source.trim_start_matches('\u{feff}');
    let head = s.char_indices().nth(512).map_or(s, |(i, _)| &s[..i]);
    head.contains("<TcPlcObject")
}

fn byte_offset(src: &str, pos: roxmltree::TextPos) -> usize {
    let mut line = 1;
    let mut offset = 0;
    for l in src.split_inclusive('\n') {
        if line == pos.row {
            let col = (pos.col as usize).saturating_sub(1);
            return offset + l.char_indices().nth(col).map_or(l.len(), |(i, _)| i);
        }
        offset += l.len();
        line += 1;
    }
    src.len()
}
