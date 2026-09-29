// SPDX-License-Identifier: MPL-2.0

//! One TwinCAT PLC object file (`<TcPlcObject>`): `.TcPOU`, `.TcDUT`, `.TcGVL`,
//! `.TcIO`. Each is reassembled into ST text (see [`crate::text`]) and parsed by
//! plcc-st.

use crate::error::TwinCatError;
use crate::text::{Source, first_word};
use plcc_st::{CompilationUnit, Declaration, Ident, Span};
use roxmltree::Node;
use std::ops::Range;

pub(crate) struct Lower<'a> {
    src: &'a str,
    pub errors: Vec<TwinCatError>,
}

fn name<'a>(n: Node<'a, '_>) -> &'a str {
    n.tag_name().name()
}

fn child<'a, 'i>(n: Node<'a, 'i>, tag: &str) -> Option<Node<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == tag)
}

impl<'a> Lower<'a> {
    pub fn new(src: &'a str) -> Self {
        Lower {
            src,
            errors: Vec::new(),
        }
    }

    /// Byte range between an element's start and end tags (empty for `<X/>`).
    fn inner(&self, n: Node) -> Range<usize> {
        let r = n.range();
        let tag = &self.src[r.clone()];
        let Some(gt) = tag.find('>') else {
            return r.end..r.end;
        };
        let open_end = r.start + gt + 1;
        if tag[..gt + 1].ends_with("/>") {
            return open_end..open_end;
        }
        let close = self.src[..r.end]
            .rfind("</")
            .filter(|p| *p >= open_end)
            .unwrap_or(r.end);
        open_end..close
    }

    /// Span of an element's start tag.
    fn tag(&self, n: Node) -> Span {
        let r = n.range();
        let end = self.src[r.clone()]
            .find('>')
            .map_or(r.end, |p| r.start + p + 1);
        Span::new(r.start, end)
    }

    /// The `Name="..."` attribute and the span of its value.
    fn name_attr(&self, n: Node) -> (String, Span) {
        let value = n.attribute("Name").unwrap_or("").to_string();
        let tag = self.tag(n);
        let text = &self.src[tag.start..tag.end];
        let span = text
            .find("Name=\"")
            .map(|p| {
                let s = tag.start + p + 6;
                Span::new(s, s + value.len())
            })
            .unwrap_or(tag);
        (value, span)
    }

    fn decl_text(&self, n: Node) -> String {
        let mut s = Source::default();
        if let Some(d) = child(n, "Declaration") {
            s.push_xml(self.src, self.inner(d));
        }
        s.text
    }

    fn push_decl(&self, s: &mut Source, n: Node) {
        if let Some(d) = child(n, "Declaration") {
            s.push_xml(self.src, self.inner(d));
        }
        s.push_synthetic("\n", n.range().start);
    }

    /// Append `n`'s `<Implementation>`; only ST can be compiled.
    fn push_body(&mut self, s: &mut Source, n: Node, owner: &str) {
        if let Some(imp) = child(n, "Implementation") {
            for c in imp.children().filter(|c| c.is_element()) {
                let lang = c.tag_name().name();
                if name(c) == "ST" {
                    s.push_xml(self.src, self.inner(c));
                } else {
                    let what = match lang {
                        "NWL" => "LD/FBD (network list)",
                        "CFC" => "CFC",
                        "SFC" => "SFC",
                        "LD" => "LD",
                        "FBD" => "FBD",
                        other => other,
                    };
                    self.errors.push(
                        TwinCatError::new(
                            format!("{owner} is written in {what}, which is not yet supported"),
                            self.tag(c),
                        )
                        .with_help(
                            "only Structured Text bodies of TwinCAT objects compile; export LD/FBD \
                             as PLCopen XML to compile it with plcc",
                        ),
                    );
                }
            }
        }
        s.push_synthetic("\n", n.range().end);
    }

    /// Lower the whole file.
    pub fn object(&mut self, root: Node) -> Vec<Declaration> {
        let mut decls = Vec::new();
        for obj in root.children().filter(|c| c.is_element()) {
            match name(obj) {
                "POU" => decls.extend(self.pou(obj)),
                "DUT" => {
                    let mut s = Source::default();
                    self.push_decl(&mut s, obj);
                    decls.extend(self.parse(&s));
                }
                "GVL" => decls.extend(self.gvl(obj)),
                "Itf" => decls.extend(self.interface(obj)),
                // Task configuration: read by `crate::task`, no declarations.
                "Task" => {}
                _ => self.errors.push(TwinCatError::warning(
                    format!(
                        "TwinCAT object <{}> holds no code; ignored",
                        obj.tag_name().name()
                    ),
                    self.tag(obj),
                )),
            }
        }
        decls
    }

    fn parse(&mut self, s: &Source) -> Vec<Declaration> {
        let (unit, errors) = plcc_st::parse(&s.text);
        self.errors.extend(errors.iter().map(|e| s.error(e)));
        let mut decls = s.remap(unit.declarations);
        for d in &mut decls {
            unlocate_io_links(d);
        }
        decls
    }

    fn pou(&mut self, pou: Node) -> Vec<Declaration> {
        let (pou_name, name_span) = self.name_attr(pou);
        let kw = first_word(&self.decl_text(pou));
        let (end_kw, members_ok) = match kw.as_str() {
            "FUNCTION_BLOCK" | "FUNCTIONBLOCK" => ("END_FUNCTION_BLOCK", true),
            "PROGRAM" => ("END_PROGRAM", true),
            "FUNCTION" => ("END_FUNCTION", false),
            _ => {
                self.errors.push(TwinCatError::new(
                    format!(
                        "cannot tell what kind of POU `{pou_name}` is: its declaration should \
                         start with PROGRAM, FUNCTION_BLOCK or FUNCTION"
                    ),
                    name_span,
                ));
                return Vec::new();
            }
        };
        let owner = format!("{} `{pou_name}`", kw.replace('_', " ").to_lowercase());
        let mut s = Source::default();
        self.push_decl(&mut s, pou);
        self.push_body(&mut s, pou, &format!("the body of {owner}"));

        for m in pou.children().filter(|c| c.is_element()) {
            let (m_name, m_span) = self.name_attr(m);
            let kind = name(m);
            if matches!(kind, "Method" | "Property" | "Action") && !members_ok {
                self.errors.push(TwinCatError::new(
                    format!(
                        "{} `{m_name}` of {owner}: a FUNCTION has no methods, properties or \
                         actions",
                        kind.to_uppercase()
                    ),
                    m_span,
                ));
                continue;
            }
            match kind {
                "Method" => {
                    self.push_decl(&mut s, m);
                    self.push_body(&mut s, m, &format!("method `{pou_name}.{m_name}`"));
                    s.push_synthetic("END_METHOD\n", m.range().end);
                }
                "Property" => self.push_property(&mut s, m, &pou_name, &m_name, true),
                "Action" => {
                    s.push_synthetic("ACTION ", m.range().start);
                    s.push_synthetic(&m_name, m_span.start);
                    s.push_synthetic(":\n", m.range().start);
                    self.push_body(&mut s, m, &format!("action `{pou_name}.{m_name}`"));
                    s.push_synthetic("END_ACTION\n", m.range().end);
                }
                "Transition" => self.errors.push(
                    TwinCatError::warning(
                        format!("SFC transition `{m_name}` of {owner} ignored"),
                        m_span,
                    )
                    .with_help("SFC is not supported; transitions are only used by SFC bodies"),
                ),
                _ => {}
            }
        }
        s.push_synthetic(end_kw, pou.range().end);
        s.push_synthetic("\n", pou.range().end);
        self.parse(&s)
    }

    fn push_property(&mut self, s: &mut Source, p: Node, owner: &str, name_: &str, bodies: bool) {
        self.push_decl(s, p);
        for acc in p.children().filter(|c| c.is_element()) {
            let (open, close) = match name(acc) {
                "Get" => ("GET\n", "END_GET\n"),
                "Set" => ("SET\n", "END_SET\n"),
                _ => continue,
            };
            s.push_synthetic(open, acc.range().start);
            self.push_decl(s, acc);
            if bodies {
                let which = if open.starts_with('G') { "GET" } else { "SET" };
                self.push_body(s, acc, &format!("{which} of property `{owner}.{name_}`"));
            }
            s.push_synthetic(close, acc.range().end);
        }
        s.push_synthetic("END_PROPERTY\n", p.range().end);
    }

    fn gvl(&mut self, gvl: Node) -> Vec<Declaration> {
        let (gvl_name, span) = self.name_attr(gvl);
        let mut s = Source::default();
        self.push_decl(&mut s, gvl);
        let mut decls = self.parse(&s);
        for d in &mut decls {
            if let Declaration::GlobalVarDecl(block) = d {
                block.list_name = Some(Ident::new(gvl_name.clone(), span));
            }
        }
        decls
    }

    fn interface(&mut self, itf: Node) -> Vec<Declaration> {
        let (itf_name, _) = self.name_attr(itf);
        let mut s = Source::default();
        self.push_decl(&mut s, itf);
        for m in itf.children().filter(|c| c.is_element()) {
            let (m_name, _) = self.name_attr(m);
            match name(m) {
                "Method" => {
                    self.push_decl(&mut s, m);
                    s.push_synthetic("END_METHOD\n", m.range().end);
                }
                "Property" => self.push_property(&mut s, m, &itf_name, &m_name, false),
                _ => {}
            }
        }
        s.push_synthetic("END_INTERFACE\n", itf.range().end);
        self.parse(&s)
    }
}

/// `x AT %I* : BOOL;` / `%Q*` / `%M*`: in TwinCAT the address is assigned by
/// linking the variable to an I/O channel in the device tree, not in source
/// (there is no VAR_CONFIG). The variable is kept as an ordinary variable,
/// reachable by name through the symbol table (`--emit-symbols`).
fn unlocate_io_links(decl: &mut Declaration) {
    let blocks: Vec<&mut plcc_st::VarBlock> = match decl {
        Declaration::Program(p) => p.var_blocks.iter_mut().collect(),
        Declaration::FunctionBlock(fb) => fb.var_blocks.iter_mut().collect(),
        Declaration::GlobalVarDecl(b) => vec![b],
        _ => Vec::new(),
    };
    for b in blocks {
        for v in &mut b.declarations {
            if v.at_address.as_ref().is_some_and(|a| a.repr.ends_with('*')) {
                v.at_address = None;
            }
        }
    }
}

/// Parse one `<TcPlcObject>` file.
pub fn parse(source: &str) -> (CompilationUnit, Vec<TwinCatError>) {
    let unit = |declarations| CompilationUnit {
        declarations,
        span: Span::new(0, source.len()),
    };
    let doc = match roxmltree::Document::parse(source) {
        Ok(d) => d,
        Err(e) => {
            let at = crate::byte_offset(source, e.pos());
            return (
                unit(Vec::new()),
                vec![TwinCatError::new(
                    format!("malformed XML: {e}"),
                    Span::new(at, at),
                )],
            );
        }
    };
    let root = doc.root_element();
    let mut lower = Lower::new(source);
    if root.tag_name().name() != "TcPlcObject" {
        let tag = lower.tag(root);
        return (
            unit(Vec::new()),
            vec![TwinCatError::new(
                format!(
                    "not a TwinCAT PLC object: the root element is <{}>, expected <TcPlcObject>",
                    root.tag_name().name()
                ),
                tag,
            )],
        );
    }
    let decls = lower.object(root);
    (unit(decls), lower.errors)
}
