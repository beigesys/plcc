// SPDX-License-Identifier: MPL-2.0

//! Rockwell Logix 5000 (Studio 5000 / RSLogix 5000) `.L5X` front end.
//!
//! Reads a Logix project export and lowers it into the same [`plcc_st`] AST the
//! Structured Text parser produces, so type checking, codegen, the process image,
//! tasks and every runtime work unchanged. Logix semantics — rung-condition
//! flow, false-rung and prescan behaviour, TIMER/COUNTER instructions, integer
//! overflow and REAL rounding — are reproduced by the lowering and by a bundled
//! ST prelude ([`prelude`]) that must be compiled alongside. `docs/l5x.md` lists
//! every decision with its source in the Rockwell manuals.
//!
//! Lowering produces ST text with a span map; after parsing, every span of the
//! AST points back into the `.L5X` file, so diagnostics from any later stage
//! land on the rung, operand or tag they are about.

mod data;
mod emit;
mod error;
mod iomap;
pub mod ladder;
mod lower;
mod model;
mod names;
mod operand;
mod rll;
mod scope;
mod strings;
mod stsem;
mod stx;
mod types;
mod xml;

pub use error::L5xError;
pub use iomap::IoMap;
use plcc_st::{CompilationUnit, Span};

/// Options for [`parse_with`].
#[derive(Default, Clone, Debug)]
pub struct Options {
    /// Bindings of Logix tags (module tags, aliases, base tags) to process-image
    /// addresses (`--io-map`).
    pub io_map: IoMap,
    /// Lower every ladder instruction on its own through the rung-condition
    /// variable (`lx__rc := lx__rc AND Start;` ...), instead of folding runs
    /// of input instructions into one expression. Both run the same; the
    /// long form is kept for the tests that prove it.
    pub long_rungs: bool,
}

/// A comment for the head of printed Structured Text lowered from Logix
/// ladder: what the names it does not declare are.
pub const PRELUDE_NOTE: &str = "(* Logix ladder lowered by plcc. Names it does not declare come from the Logix\n   \
prelude, compiled with every L5X program: the data types (TIMER, COUNTER,\n   \
CONTROL, ...), the instructions (lx__ton, lx__ctu, lx__put_DINT_i, ...) and the\n   \
controller status flags (lx__S_FS first scan, lx__S_V overflow, lx__S_Z,\n   \
lx__S_N, ...). docs/l5x.md \"Reading the generated ST\". *)\n\n";

/// Name under which the prelude is reported in diagnostics.
pub const PRELUDE_NAME: &str = "logix.st";

const PRELUDE_ST: &str = include_str!("../st/logix.st");

/// The Logix instruction prelude: predefined structures (TIMER, COUNTER,
/// CONTROL, STRING, ...), controller status flags and the instruction helpers
/// the lowering calls. Compile it with every L5X input.
pub fn prelude() -> String {
    let mut s = String::from(PRELUDE_ST);
    s.push_str("\n(* ---- PID and MESSAGE (1756-RM003), generated from the type table ---- *)\n");
    s.push_str(&types::TypeEnv::prelude_types());
    s.push_str(
        "\n(* ---- Stores (1756-RM003 \"Data conversions\", \"Math status flags\") ----\n   \
         lx__put_<T>_i / _r store an integer / floating result into a <T>:\n   \
         integers keep their low bits (\"truncates the upper portion\"), REAL\n   \
         rounds half to even, and S:V (with a minor fault), S:Z, S:N follow the\n   \
         stored value. *)\n",
    );
    let ints = [
        "SINT", "INT", "DINT", "LINT", "USINT", "UINT", "UDINT", "ULINT",
    ];
    for t in ints {
        let signed = !t.starts_with('U');
        let check = if t == "LINT" || t == "ULINT" {
            "FALSE".to_string()
        } else {
            format!("{t}_TO_LINT(r) <> v")
        };
        let conv = if t == "LINT" {
            "v".to_string()
        } else {
            format!("LINT_TO_{t}(v)")
        };
        let neg = if signed { "r < 0" } else { "FALSE" };
        s.push_str(&format!(
            "FUNCTION lx__put_{t}_i : {t}\nVAR_INPUT v : LINT; END_VAR\nVAR r : {t}; END_VAR\n    \
             r := {conv};\n    IF {check} THEN lx__overflow(); END_IF;\n    \
             lx__S_Z := r = 0;\n    lx__S_N := {neg};\n    lx__put_{t}_i := r;\nEND_FUNCTION\n\n\
             FUNCTION lx__put_{t}_r : {t}\nVAR_INPUT v : LREAL; END_VAR\nVAR r : {t}; END_VAR\n    \
             r := lx__put_{t}_i(lx__r2l(lx__round(v)));\n    \
             lx__put_{t}_r := r;\nEND_FUNCTION\n\n"
        ));
    }
    s.push_str(
        "FUNCTION lx__put_REAL_r : REAL\nVAR_INPUT v : LREAL; END_VAR\nVAR r : REAL; END_VAR\n    \
         r := LREAL_TO_REAL(v);\n    IF v > 3.4028234663852886E38 OR v < -3.4028234663852886E38 THEN lx__overflow(); END_IF;\n    \
         lx__S_Z := r = 0.0;\n    lx__S_N := r < 0.0;\n    \
         lx__put_REAL_r := r;\nEND_FUNCTION\n\n\
         FUNCTION lx__put_REAL_i : REAL\nVAR_INPUT v : LINT; END_VAR\n    \
         lx__put_REAL_i := lx__put_REAL_r(LINT_TO_LREAL(v));\nEND_FUNCTION\n\n\
         FUNCTION lx__put_LREAL_r : LREAL\nVAR_INPUT v : LREAL; END_VAR\n    \
         lx__S_Z := v = 0.0;\n    lx__S_N := v < 0.0;\n    lx__put_LREAL_r := v;\nEND_FUNCTION\n\n\
         FUNCTION lx__put_LREAL_i : LREAL\nVAR_INPUT v : LINT; END_VAR\n    \
         lx__put_LREAL_i := lx__put_LREAL_r(LINT_TO_LREAL(v));\nEND_FUNCTION\n\n\
         (* BTD (1756-RM003 \"Bit Field Distribute\"): Length bits of s from bit sb\n   \
         into d from bit db. *)\n\
         FUNCTION lx__btd : LINT\nVAR_INPUT s : LINT; sb : LINT; d : LINT; db : LINT; n : LINT; END_VAR\n\
         VAR i : LINT; r : LINT; m : LINT; END_VAR\n    r := d;\n    FOR i := 0 TO n - 1 DO\n        \
         IF db + i >= 0 AND db + i <= 63 AND sb + i >= 0 AND sb + i <= 63 THEN\n            \
         m := lx__shl(1, db + i);\n            \
         IF lx__getbit(s, sb + i) THEN r := r OR m; ELSE r := r AND NOT m; END_IF;\n        \
         END_IF;\n    END_FOR;\n    lx__btd := r;\nEND_FUNCTION\n",
    );
    s
}

/// Whether `source` looks like an L5X export (root element
/// `<RSLogix5000Content>`).
pub fn is_l5x(source: &str) -> bool {
    let mut s = source.trim_start_matches('\u{feff}').trim_start();
    loop {
        if let Some(rest) = s.strip_prefix("<?") {
            match rest.find("?>") {
                Some(p) => s = rest[p + 2..].trim_start(),
                None => return false,
            }
        } else if let Some(rest) = s.strip_prefix("<!--") {
            match rest.find("-->") {
                Some(p) => s = rest[p + 3..].trim_start(),
                None => return false,
            }
        } else {
            return s.starts_with("<RSLogix5000Content");
        }
    }
}

/// Lower an L5X file to Structured Text: the generated text, its span map, and
/// the diagnostics found on the way.
type Lowered = (
    emit::Out,
    Vec<L5xError>,
    types::TypeEnv,
    strings::Helpers,
    Vec<String>,
);

fn lower(source: &str, opts: &Options, annotate: bool) -> Result<Lowered, Vec<L5xError>> {
    let opt = roxmltree::ParsingOptions {
        allow_dtd: false,
        ..Default::default()
    };
    let doc = match roxmltree::Document::parse_with_options(source, opt) {
        Ok(d) => d,
        Err(e) => {
            let at = byte_offset(source, e.pos());
            return Err(vec![L5xError::new(
                format!("malformed XML: {e}"),
                Span::new(at, at),
            )]);
        }
    };
    let root = doc.root_element();
    if xml::name(root) != "RSLogix5000Content" {
        return Err(vec![L5xError::new(
            format!(
                "not an L5X export: the root element is <{}>, expected <RSLogix5000Content>",
                xml::name(root)
            ),
            xml::tag_span(source, root),
        )]);
    }
    let mut reader = model::Reader {
        src: source,
        errors: Vec::new(),
    };
    let project = reader.project(root);
    let mut lw = lower::Lower::new(source, opts.io_map.clone());
    lw.errors = reader.errors;
    lw.annotate = annotate;
    lw.long_rungs = opts.long_rungs;
    let out = lw.project(&project);
    let comments = lw.comments.take();
    Ok((out, lw.errors, lw.env, lw.strings, comments))
}

/// The Structured Text an L5X file lowers to (for inspection:
/// `plcc parse --dump-st`), with the diagnostics of the lowering.
pub fn to_st(source: &str, opts: &Options) -> (String, Vec<L5xError>) {
    match lower(source, opts, false) {
        Ok((out, errs, _, _, _)) => (out.text, errs),
        Err(errs) => (String::new(), errs),
    }
}

/// Parse an L5X project into a compilation unit (without the prelude).
pub fn parse(source: &str) -> (CompilationUnit, Vec<L5xError>) {
    parse_with(source, &Options::default())
}

/// [`parse`] with options (I/O map).
pub fn parse_with(source: &str, opts: &Options) -> (CompilationUnit, Vec<L5xError>) {
    parse_impl(source, opts, false)
}

/// [`parse_with`] for printing the lowered ST (`plcc convert --to st`): each
/// rung's statements are preceded by a comment statement with the rung
/// number, its documentation and its neutral text. Compiling the result is
/// the same as compiling [`parse_with`]'s.
pub fn parse_annotated(source: &str, opts: &Options) -> (CompilationUnit, Vec<L5xError>) {
    parse_impl(source, opts, true)
}

fn parse_impl(source: &str, opts: &Options, annotate: bool) -> (CompilationUnit, Vec<L5xError>) {
    let empty = CompilationUnit {
        declarations: Vec::new(),
        span: Span::new(0, source.len()),
    };
    let (out, mut errors, env, strings, comments) = match lower(source, opts, annotate) {
        Ok(x) => x,
        Err(errs) => return (empty, errs),
    };
    let text = out.text.clone();
    let map = out.map();
    let (unit, parse_errors) = plcc_st::parse(&text);
    for e in &parse_errors {
        use plcc_st::ParseError as P;
        let (message, span) = match e {
            P::UnexpectedToken { span, .. } | P::UnexpectedEof { span, .. } => {
                (e.to_string(), *span)
            }
            P::General { message, span } => (message.clone(), *span),
        };
        let sp = map.span(Span::new(span.offset(), span.offset() + span.len()));
        let line = text[..span.offset().min(text.len())].lines().count();
        errors.push(L5xError::new(message, sp).with_help(format!(
            "in the Structured Text plcc generated for this element (line {line}; `plcc parse --dump-st` shows it)"
        )));
    }
    let mut declarations = unit.declarations;
    dedup_prescan(&mut declarations);
    if !comments.is_empty() {
        comment_markers(&mut declarations, &comments);
    }
    let before = strings.names();
    stsem::apply(&mut declarations, &env, &strings);
    // String helpers first needed by the AST pass (ST string compares and
    // assignments) are parsed and added now; they point at the file start.
    let extra = strings.source_except(&before);
    if !extra.is_empty() {
        let (u, errs) = plcc_st::parse(&extra);
        if errs.is_empty() {
            declarations.extend(emit::zero_spans(u.declarations));
        }
    }
    let declarations = map.remap(declarations);
    (
        CompilationUnit {
            declarations,
            span: Span::new(0, source.len()),
        },
        errors,
    )
}

/// Drop the assignments of a program's `lx__prescan` method that repeat an
/// earlier one with nothing in between that could have changed the target:
/// several instructions on one tag each contribute their prescan action (a
/// TON's `ACC := 0`, and its `Accum` operand `0`), and the method should say
/// it once. Only assignments of a literal are dropped; a write that may touch
/// the target (the same path, a path inside it or around it, any indexed
/// path on the same tag) ends the earlier assignment's reach, and a call ends
/// every one; an indexed target is never a repeat. The order of what stays is
/// unchanged.
fn dedup_prescan(decls: &mut [plcc_st::Declaration]) {
    use plcc_st::{ExpressionKind, StatementKind, print_expression};
    fn literal(e: &plcc_st::Expression) -> bool {
        match &e.kind {
            ExpressionKind::IntegerLiteral(_)
            | ExpressionKind::RealLiteral(_)
            | ExpressionKind::BoolLiteral(_) => true,
            ExpressionKind::UnaryOp { operand, .. } => literal(operand),
            ExpressionKind::Parenthesized(inner) => literal(inner),
            _ => false,
        }
    }
    fn root(path: &str) -> &str {
        path.split(['.', '[']).next().unwrap_or(path)
    }
    // Could a write to `a` change `b` (or the reverse)?
    fn overlap(a: &str, b: &str) -> bool {
        if root(a) != root(b) {
            return false;
        }
        if a.contains('[') || b.contains('[') {
            return true;
        }
        let inside = |x: &str, y: &str| {
            x == y || (x.starts_with(y) && x[y.len()..].starts_with(['.', '[']))
        };
        inside(a, b) || inside(b, a)
    }
    for d in decls {
        let plcc_st::Declaration::FunctionBlock(f) = d else {
            continue;
        };
        for m in &mut f.methods {
            if !m.name.name.eq_ignore_ascii_case("lx__prescan") {
                continue;
            }
            // (target, value) of the assignments still in effect.
            let mut seen: Vec<(String, String)> = Vec::new();
            m.body.retain(|s| {
                let StatementKind::Assignment { target, value } = &s.kind else {
                    seen.clear();
                    return true;
                };
                let t = print_expression(target).to_ascii_uppercase();
                let v = print_expression(value).to_ascii_uppercase();
                if seen.iter().any(|(st, sv)| *st == t && *sv == v) {
                    return false;
                }
                seen.retain(|(st, _)| !overlap(st, &t));
                // An indexed target names another element once its index
                // variable changes: never treated as a repeat.
                if literal(value) && !t.contains('[') {
                    seen.push((t, v));
                }
                true
            });
        }
    }
}

/// Replace the `__PLCC_COMMENT(k);` markers of an annotated lowering with
/// comment statements.
fn comment_markers(decls: &mut [plcc_st::Declaration], comments: &[String]) {
    use plcc_st::{ExpressionKind, Statement, StatementKind};
    fn stmts(list: &mut [Statement], comments: &[String]) {
        for s in list {
            match &mut s.kind {
                StatementKind::FunctionCall { callee, args } => {
                    if let ExpressionKind::Identifier(id) = &callee.kind
                        && id.name == "__PLCC_COMMENT"
                        && let [a] = args.as_slice()
                        && let ExpressionKind::IntegerLiteral(k) = a.value.kind
                        && let Some(text) = comments.get(k as usize)
                    {
                        s.kind = StatementKind::Comment(text.clone());
                    }
                }
                StatementKind::If {
                    then_body,
                    elsif_branches,
                    else_body,
                    ..
                } => {
                    stmts(then_body, comments);
                    for b in elsif_branches {
                        stmts(&mut b.body, comments);
                    }
                    if let Some(e) = else_body {
                        stmts(e, comments);
                    }
                }
                StatementKind::Case {
                    branches,
                    else_body,
                    ..
                } => {
                    for b in branches {
                        stmts(&mut b.body, comments);
                    }
                    if let Some(e) = else_body {
                        stmts(e, comments);
                    }
                }
                StatementKind::For { body, .. }
                | StatementKind::While { body, .. }
                | StatementKind::Repeat { body, .. } => stmts(body, comments),
                _ => {}
            }
        }
    }
    for d in decls {
        match d {
            plcc_st::Declaration::Program(p) => {
                stmts(&mut p.body, comments);
                for m in &mut p.methods {
                    stmts(&mut m.body, comments);
                }
            }
            plcc_st::Declaration::FunctionBlock(f) => {
                stmts(&mut f.body, comments);
                for m in &mut f.methods {
                    stmts(&mut m.body, comments);
                }
            }
            plcc_st::Declaration::Function(f) => stmts(&mut f.body, comments),
            _ => {}
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn prescan_after_dedup(body: &str) -> String {
        let src = format!(
            "FUNCTION_BLOCK F\nVAR a : ARRAY[0..3] OF BOOL; i : DINT; t : TIMER; x : BOOL; END_VAR\n\
             METHOD lx__prescan\n{body}\nEND_METHOD\nEND_FUNCTION_BLOCK\n"
        );
        let (unit, errs) = plcc_st::parse(&src);
        assert!(errs.is_empty(), "{errs:?}");
        let mut decls = unit.declarations;
        dedup_prescan(&mut decls);
        let plcc_st::Declaration::FunctionBlock(f) = &decls[0] else {
            panic!()
        };
        plcc_st::print_statements(&f.methods[0].body, 0)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn prescan_repeats_are_dropped_only_when_nothing_changed_them() {
        // A plain repeat goes; the first stays where it was.
        assert_eq!(
            prescan_after_dedup("x := FALSE; t.ACC := 0; x := FALSE; t.ACC := 0;"),
            "x := FALSE; t.ACC := 0;"
        );
        // Another value in between, a write of the whole structure, a call:
        // the repeat stays.
        assert_eq!(
            prescan_after_dedup("x := FALSE; x := TRUE; x := FALSE;"),
            "x := FALSE; x := TRUE; x := FALSE;"
        );
        assert_eq!(
            prescan_after_dedup("t.ACC := 0; t := t; t.ACC := 0;"),
            "t.ACC := 0; t := t; t.ACC := 0;"
        );
        assert_eq!(
            prescan_after_dedup("x := FALSE; t(); x := FALSE;"),
            "x := FALSE; t(); x := FALSE;"
        );
        // An indexed target is another element once the index changes.
        assert_eq!(
            prescan_after_dedup("a[i] := TRUE; i := 1; a[i] := TRUE;"),
            "a[i] := TRUE; i := 1; a[i] := TRUE;"
        );
        // A write to another member does not end the repeat.
        assert_eq!(
            prescan_after_dedup("t.ACC := 0; t.DN := FALSE; t.ACC := 0;"),
            "t.ACC := 0; t.DN := FALSE;"
        );
    }
}
