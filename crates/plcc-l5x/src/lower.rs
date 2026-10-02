// SPDX-License-Identifier: MPL-2.0

//! The project as Structured Text.
//!
//! * UDTs, string types and module I/O structures → `TYPE ... STRUCT`.
//! * Add-On Instructions → `FUNCTION_BLOCK`s: Input / Output / InOut parameters
//!   become VAR_INPUT / VAR_OUTPUT / VAR_IN_OUT, local tags VAR, plus
//!   `EnableIn` / `EnableOut`.
//! * Controller-scoped tags and module tags → one `VAR_GLOBAL` block.
//! * Each program → a `FUNCTION_BLOCK lx__P_<name>` holding the program tags,
//!   one METHOD per routine, and a global instance named like the program (so
//!   `Program:Main.Motor` is `Main.Motor`), called by a small runner
//!   `PROGRAM lx__run_<name>`.
//! * Tasks → CONFIGURATION / TASK; a continuous task's programs run in plcc's
//!   free-running background task.

use crate::data::{self, L5kSlot};
use crate::emit::Out;
use crate::error::L5xError;
use crate::iomap::IoMap;
use crate::model::*;
use crate::names::ident;
use crate::rll::{self, RoutineOut, Shared};
use crate::scope::{Ctx, Scope, Sym};
use crate::stx;
use crate::types::{Elem, Field, StructDef, StructKind, Ty, TypeEnv};
use crate::xml::{self, XNode};
use plcc_st::Span;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub(crate) struct ParamSig {
    pub logix: String,
    /// ST member path in the backing tag (`PCmd.0` for an alias parameter).
    pub st: String,
    pub usage: Usage,
    pub required: bool,
    pub ty: Ty,
    /// An alias parameter: a view of another member, not an FB input.
    pub alias: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct AoiSig {
    pub st: String,
    pub params: Vec<ParamSig>,
}

pub(crate) struct Lower<'s> {
    pub src: &'s str,
    pub errors: Vec<L5xError>,
    pub env: TypeEnv,
    pub layouts: Vec<Option<Vec<L5kSlot>>>,
    pub ctrl: Scope,
    pub prog_scopes: Vec<Scope>,
    /// Lower-case program name → (global instance, index into prog_scopes).
    pub programs: HashMap<String, (String, usize)>,
    pub aois: HashMap<String, AoiSig>,
    pub io: IoMap,
    pub strings: crate::strings::Helpers,
    /// Names taken in the global namespace (lower-case).
    globals: HashMap<String, Span>,
    /// Emit a comment before each rung (`plcc convert`).
    pub annotate: bool,
    /// [`crate::Options::long_rungs`].
    pub long_rungs: bool,
    /// Texts of the rung comments, by marker number.
    pub comments: std::cell::RefCell<Vec<String>>,
}

fn dims_ty(base: Ty, dims: &[u32]) -> Ty {
    if dims.is_empty() {
        base
    } else {
        Ty::Array(Box::new(base), dims.to_vec())
    }
}

impl<'s> Lower<'s> {
    pub fn new(src: &'s str, io: IoMap) -> Self {
        Lower {
            src,
            errors: Vec::new(),
            env: TypeEnv::new(),
            layouts: Vec::new(),
            ctrl: Scope::default(),
            prog_scopes: Vec::new(),
            programs: HashMap::new(),
            aois: HashMap::new(),
            io,
            strings: Default::default(),
            globals: HashMap::new(),
            annotate: false,
            long_rungs: false,
            comments: Default::default(),
        }
    }

    fn err(&mut self, msg: impl Into<String>, span: Span) {
        self.errors.push(L5xError::new(msg, span));
    }

    fn warn(&mut self, msg: impl Into<String>, span: Span) {
        self.errors.push(L5xError::warning(msg, span));
    }

    // ── Types ──

    fn register_datatypes(&mut self, p: &Project) {
        // Pass 1: names, so members may refer to types declared later.
        let mut ids = Vec::new();
        for dt in &p.datatypes {
            if self.env.lookup(&dt.name.text).is_some() {
                // A predefined type plcc models itself (TIMER, CONTROL, ...)
                // listed by an export with dependencies: plcc's definition
                // stands. Others (AXIS_*, ALARM_*, module types) are read
                // like UDTs, so their members can be used.
                if !dt.predefined {
                    self.err(
                        format!("data type `{}` is declared twice", dt.name.text),
                        dt.name.span,
                    );
                }
                ids.push(None);
                continue;
            }
            let kind = if dt.string_family {
                StructKind::Str
            } else {
                StructKind::Udt
            };
            let id = self.env.add(StructDef {
                logix: dt.name.text.clone(),
                st: ident(&dt.name.text),
                kind,
                fields: Vec::new(),
                opaque: false,
            });
            ids.push(Some(id));
        }
        // Pass 2: members.
        for (dt, id) in p.datatypes.iter().zip(ids) {
            let Some(id) = id else { continue };
            let mut fields = Vec::new();
            let mut layout = Vec::new();
            let mut hosts: HashMap<String, usize> = HashMap::new();
            for m in &dt.members {
                if let Some((target, bit)) = &m.bit_of {
                    fields.push(Field {
                        logix: m.name.text.clone(),
                        st: ident(&m.name.text),
                        ty: Ty::Elem(Elem::Bool),
                    });
                    let fi = fields.len() - 1;
                    if let Some(&slot) = hosts.get(&target.to_ascii_lowercase())
                        && let Some(L5kSlot::Host(bits)) = layout.get_mut(slot)
                    {
                        bits.push((*bit, fi));
                    }
                    continue;
                }
                if m.hidden {
                    hosts.insert(m.name.text.to_ascii_lowercase(), layout.len());
                    layout.push(L5kSlot::Host(Vec::new()));
                    continue;
                }
                let Some(base) = self.env.resolve(&m.data_type.text) else {
                    // Left out: using the member is an error at the use.
                    self.warn(
                        format!(
                            "member `{}` has data type `{}`, which plcc cannot model; it was left out",
                            m.name.text, m.data_type.text
                        ),
                        m.data_type.span,
                    );
                    continue;
                };
                let ty = if m.dim > 0 {
                    Ty::Array(Box::new(base), vec![m.dim])
                } else {
                    base
                };
                fields.push(Field {
                    logix: m.name.text.clone(),
                    st: ident(&m.name.text),
                    ty,
                });
                layout.push(L5kSlot::Field(fields.len() - 1));
            }
            if dt.string_family {
                let cap = fields
                    .iter()
                    .find(|f| f.logix.eq_ignore_ascii_case("DATA"))
                    .and_then(|f| match &f.ty {
                        Ty::Array(_, d) => d.first().copied(),
                        _ => None,
                    })
                    .unwrap_or(82);
                fields = vec![
                    Field {
                        logix: "LEN".into(),
                        st: "LEN".into(),
                        ty: Ty::Elem(Elem::Dint),
                    },
                    Field {
                        logix: "DATA".into(),
                        st: "DATA".into(),
                        ty: Ty::Array(Box::new(Ty::Elem(Elem::Sint)), vec![cap]),
                    },
                ];
            }
            self.env.structs[id].fields = fields;
            self.set_layout(id, Some(layout));
        }
    }

    fn set_layout(&mut self, id: usize, l: Option<Vec<L5kSlot>>) {
        if self.layouts.len() <= id {
            self.layouts.resize(id + 1, None);
        }
        self.layouts[id] = l;
    }

    fn tag_ty(&mut self, t: &TagDef) -> Option<Ty> {
        let dt = t.data_type.as_ref()?;
        let base = self.env.resolve(&dt.text)?;
        Some(dims_ty(base, &t.dims))
    }

    fn register_aois(&mut self, p: &Project) {
        for a in &p.aois {
            if self.env.lookup(&a.name.text).is_some() {
                self.err(format!("`{}` is declared twice", a.name.text), a.name.span);
                continue;
            }
            self.env.add(StructDef {
                logix: a.name.text.clone(),
                st: ident(&a.name.text),
                kind: StructKind::Aoi,
                fields: Vec::new(),
                opaque: false,
            });
        }
        for a in &p.aois {
            let Some(id) = self.env.lookup(&a.name.text) else {
                continue;
            };
            let mut fields = Vec::new();
            let mut params = Vec::new();
            for t in a.params.iter().chain(&a.locals) {
                let ty = match self.tag_ty(t) {
                    Some(ty) => ty,
                    None if t.kind == TagKind::Alias => continue,
                    None => {
                        // A required parameter is part of every call; a local
                        // tag or optional parameter can be left out (using it
                        // is an error at the use).
                        let required = t.required || t.usage == Usage::InOut;
                        let msg = format!(
                            "`{}` has data type `{}`, which plcc cannot model{}",
                            t.name.text,
                            t.data_type.as_ref().map_or("?", |d| d.text.as_str()),
                            if required { "" } else { "; it was left out" }
                        );
                        let at = t.data_type.as_ref().map_or(t.span, |d| d.span);
                        if required {
                            self.err(msg, at);
                        } else {
                            self.warn(msg, at);
                        }
                        continue;
                    }
                };
                fields.push(Field {
                    logix: t.name.text.clone(),
                    st: ident(&t.name.text),
                    ty: ty.clone(),
                });
                if !matches!(t.usage, Usage::Local) {
                    params.push(ParamSig {
                        logix: t.name.text.clone(),
                        st: ident(&t.name.text),
                        usage: t.usage,
                        required: t.required
                            || t.usage == Usage::InOut
                                && !t.name.text.eq_ignore_ascii_case("EnableIn"),
                        ty,
                        alias: false,
                    });
                }
            }
            // Alias parameters and local tags (`PCmd_Red` AliasFor
            // `PCmd.0`): views of other members of the backing tag.
            let mut scope = Scope::default();
            for f in &fields {
                scope.insert(
                    &f.logix,
                    Sym {
                        st: f.st.clone(),
                        ty: Some(f.ty.clone()),
                        unknown_type: None,
                        decl: a.span,
                    },
                );
            }
            for t in a.params.iter().chain(&a.locals) {
                if t.kind != TagKind::Alias {
                    continue;
                }
                let Some(target) = &t.alias_for else { continue };
                let resolved = crate::operand::parse_expr(&target.text, 0..target.text.len())
                    .ok()
                    .and_then(|e| match e.kind {
                        crate::operand::LKind::Path(tp) => {
                            let ctx = Ctx {
                                env: &self.env,
                                layers: vec![&scope],
                                programs: &self.programs,
                                program_scopes: &self.prog_scopes,
                            };
                            ctx.path(&tp, target).ok()
                        }
                        _ => None,
                    });
                let Some((st, ty)) = resolved else {
                    self.warn(
                        format!(
                            "alias `{}` of `{}` could not be resolved and was left out",
                            t.name.text, a.name.text
                        ),
                        t.span,
                    );
                    continue;
                };
                fields.push(Field {
                    logix: t.name.text.clone(),
                    st: st.clone(),
                    ty: ty.clone(),
                });
                if !matches!(t.usage, Usage::Local) {
                    params.push(ParamSig {
                        logix: t.name.text.clone(),
                        st,
                        usage: t.usage,
                        required: t.required,
                        ty,
                        alias: true,
                    });
                }
            }
            // Required parameters are operands in definition order.
            params.sort_by_key(|p| {
                a.params
                    .iter()
                    .position(|t| t.name.text.eq_ignore_ascii_case(&p.logix))
                    .unwrap_or(usize::MAX)
            });
            self.env.structs[id].fields = fields;
            self.aois.insert(
                a.name.text.to_ascii_lowercase(),
                AoiSig {
                    st: ident(&a.name.text),
                    params,
                },
            );
        }
    }

    /// Module I/O structures are not in <DataTypes>; rebuild them from the
    /// decorated data of the module's connection tags.
    fn struct_from_decorated(&mut self, n: XNode) -> Option<Ty> {
        let dt = xml::attr(n, "DataType")?;
        if let Some(t) = self.env.resolve(dt) {
            return Some(t);
        }
        let id = self.env.add(StructDef {
            logix: dt.to_string(),
            st: ident(dt),
            kind: StructKind::Module,
            fields: Vec::new(),
            opaque: false,
        });
        let mut fields = Vec::new();
        for m in xml::elements(n) {
            let Some(name) = xml::attr(m, "Name") else {
                continue;
            };
            let ty = match xml::name(m) {
                "DataValueMember" => {
                    let Some(e) = xml::attr(m, "DataType").and_then(Elem::parse) else {
                        continue;
                    };
                    Ty::Elem(e)
                }
                "ArrayMember" => {
                    let dims: Vec<u32> = xml::attr(m, "Dimensions")
                        .map(|d| {
                            d.split([' ', ','])
                                .filter_map(|x| x.trim().parse().ok())
                                .collect()
                        })
                        .unwrap_or_default();
                    let elem = match xml::attr(m, "DataType").and_then(Elem::parse) {
                        Some(e) => Ty::Elem(e),
                        None => {
                            let first =
                                xml::child(m, "Element").and_then(|e| xml::elements(e).next());
                            match first.and_then(|s| self.struct_from_decorated(s)) {
                                Some(t) => t,
                                None => continue,
                            }
                        }
                    };
                    if dims.is_empty() {
                        continue;
                    }
                    Ty::Array(Box::new(elem), dims)
                }
                "StructureMember" => match self.struct_from_decorated(m) {
                    Some(t) => t,
                    None => continue,
                },
                _ => continue,
            };
            fields.push(Field {
                logix: name.to_string(),
                st: ident(name),
                ty,
            });
        }
        self.env.structs[id].fields = fields;
        Some(Ty::Struct(id))
    }

    fn register_modules(&mut self, p: &Project, globals: &mut Out) {
        // Module tag names (Logix Designer's convention): a module in a
        // chassis — on a backplane port of its parent (the controller's own
        // chassis `Local`, or a remote chassis behind a communication
        // adapter) — is `<parent>:<slot>:I`; a module on a network port is
        // `<module>:I`.
        let network = |ty: &str| {
            let t = ty.to_ascii_lowercase();
            [
                "ethernet",
                "controlnet",
                "devicenet",
                "dhplus",
                "rio",
                "serial",
                "usb",
                "sercos",
            ]
            .iter()
            .any(|n| t.contains(n))
        };
        for m in &p.modules {
            let parent = p.modules.iter().find(|x| {
                x.name.text.eq_ignore_ascii_case(&m.parent) && x.name.text != m.name.text
            });
            let in_chassis = parent.is_some_and(|par| {
                par.ports
                    .iter()
                    .find(|(id, _)| Some(*id) == m.parent_port)
                    .is_some_and(|(_, ty)| !network(ty))
            });
            for (suffix, data, span) in &m.io_tags {
                let tag = match (&m.slot, in_chassis) {
                    (Some(slot), true) => format!("{}:{slot}:{suffix}", m.parent),
                    _ => format!("{}:{suffix}", m.name.text),
                };
                // Several connections of one module can each carry the
                // same tag (a drive's input data); it is one tag.
                if self.ctrl.get(&tag).is_some_and(|s| s.ty.is_some()) {
                    continue;
                }
                let Some(ty) = data
                    .and_then(|d| xml::elements(d).next())
                    .and_then(|s| self.struct_from_decorated(s))
                else {
                    self.ctrl.insert(
                        &tag,
                        Sym {
                            st: ident(&tag),
                            ty: None,
                            unknown_type: Some(format!("module tag {tag} without decorated data")),
                            decl: *span,
                        },
                    );
                    continue;
                };
                let st = ident(&tag);
                let init = data.and_then(|d| data::decorated(&self.env, &ty, d));
                self.declare_global(globals, &tag, &st, &ty, init, *span);
                self.ctrl.insert(
                    &tag,
                    Sym {
                        st,
                        ty: Some(ty),
                        unknown_type: None,
                        decl: *span,
                    },
                );
            }
        }
    }

    /// A module's name used as an operand (the MODULE InOut of an AOI that
    /// reads the module with GSV): a hidden global of the opaque MODULE type,
    /// unless a tag already has the name.
    fn module_refs(&mut self, p: &Project, globals: &mut Out) {
        let Some(id) = self.env.lookup("MODULE") else {
            return;
        };
        for m in &p.modules {
            let name = &m.name.text;
            if self.ctrl.get(name).is_some() || name.contains(':') {
                continue;
            }
            let st = format!("lx__module_{}", ident(name));
            let ty = Ty::Struct(id);
            self.declare_global(globals, name, &st, &ty, None, m.name.span);
            self.ctrl.insert(
                name,
                Sym {
                    st,
                    ty: Some(ty),
                    unknown_type: None,
                    decl: m.name.span,
                },
            );
        }
    }

    fn declare_global(
        &mut self,
        out: &mut Out,
        logix: &str,
        st: &str,
        ty: &Ty,
        init: Option<String>,
        span: Span,
    ) {
        if let Some(prev) = self.globals.insert(st.to_ascii_lowercase(), span) {
            let _ = prev;
            self.err(format!("`{logix}` is declared twice"), span);
            return;
        }
        out.push_ctx(span);
        out.s("    ").s(st).s(" : ").s(&self.env.st(ty));
        if let Some(i) = init {
            out.s(" := ").s(&i);
        }
        out.s(";\n");
        out.pop_ctx();
    }

    fn init_of(&self, t: &TagDef, ty: &Ty) -> Option<String> {
        if let Some(d) = t.decorated {
            return data::decorated(&self.env, ty, d);
        }
        if let Some(s) = &t.string_data
            && crate::scope::is_string(&self.env, ty)
        {
            let body = s
                .text
                .trim()
                .trim_start_matches('\'')
                .trim_end_matches('\'');
            return data::decorated_string(&self.env, ty, body);
        }
        if let Some(l) = &t.l5k {
            return data::l5k(&self.env, ty, &l.text, &self.layouts);
        }
        None
    }

    /// Declare the base tags of one scope; returns (logix name, alias text)
    /// pairs to resolve afterwards.
    fn scope_tags(&mut self, tags: &[TagDef], scope: &mut Scope, out: &mut Out, global: bool) {
        for t in tags {
            if t.kind == TagKind::Alias {
                continue;
            }
            // Some exports list module tags (`FAN_030:C`) among the
            // controller tags too; the module already declared it.
            if global && t.name.text.contains(':') && scope.get(&t.name.text).is_some() {
                continue;
            }
            let st = ident(&t.name.text);
            let ty = self.tag_ty(t);
            let Some(ty) = ty else {
                let dt = t
                    .data_type
                    .as_ref()
                    .map_or("?".to_string(), |d| d.text.clone());
                self.warn(
                    format!(
                        "tag `{}` of data type `{dt}` is not supported and was left out",
                        t.name.text
                    ),
                    t.data_type.as_ref().map_or(t.span, |d| d.span),
                );
                scope.insert(
                    &t.name.text,
                    Sym {
                        st,
                        ty: None,
                        unknown_type: Some(dt),
                        decl: t.span,
                    },
                );
                continue;
            };
            let init = self.init_of(t, &ty);
            if global {
                self.declare_global(out, &t.name.text, &st, &ty, init, t.span);
            } else {
                out.push_ctx(t.span);
                out.s("        ").s(&st).s(" : ").s(&self.env.st(&ty));
                if let Some(i) = init {
                    out.s(" := ").s(&i);
                }
                out.s(";\n");
                out.pop_ctx();
            }
            if !scope.insert(
                &t.name.text,
                Sym {
                    st,
                    ty: Some(ty),
                    unknown_type: None,
                    decl: t.span,
                },
            ) {
                self.err(
                    format!("tag `{}` is declared twice", t.name.text),
                    t.name.span,
                );
            }
        }
    }

    /// Resolve alias tags of `tags` into `scope` (looked up through `outer`).
    fn scope_aliases(&mut self, tags: &[TagDef], which: Option<usize>) {
        let mut pending: Vec<&TagDef> = tags.iter().filter(|t| t.kind == TagKind::Alias).collect();
        // Aliases may refer to aliases: resolve in rounds.
        for _round in 0..8 {
            if pending.is_empty() {
                break;
            }
            let mut next = Vec::new();
            let pending_len = pending.len();
            for t in pending {
                let Some(target) = &t.alias_for else {
                    self.err("alias tag without AliasFor", t.span);
                    continue;
                };
                let parsed = crate::operand::parse_expr(&target.text, 0..target.text.len());
                let res = match &parsed {
                    Ok(e) => match &e.kind {
                        crate::operand::LKind::Path(tp) => {
                            let ctx = self.ctx_for(which);
                            ctx.path(tp, target)
                        }
                        _ => Err(L5xError::new("an alias must name a tag", target.whole())),
                    },
                    Err(e) => Err(L5xError::new(
                        e.message.clone(),
                        target.span(e.span.clone()),
                    )),
                };
                match res {
                    Ok((st, ty)) => {
                        let sym = Sym {
                            st,
                            ty: Some(ty),
                            unknown_type: None,
                            decl: t.span,
                        };
                        match which {
                            Some(i) => self.prog_scopes[i].insert(&t.name.text, sym),
                            None => self.ctrl.insert(&t.name.text, sym),
                        };
                    }
                    Err(e) if e.message.starts_with("unknown tag") => next.push((t, e)),
                    Err(e) => {
                        // The target exists but has a type plcc cannot model:
                        // record the alias as unusable, report at use.
                        let sym = Sym {
                            st: ident(&t.name.text),
                            ty: None,
                            unknown_type: Some(format!("alias for {}", target.text.trim())),
                            decl: t.span,
                        };
                        match which {
                            Some(i) => self.prog_scopes[i].insert(&t.name.text, sym),
                            None => self.ctrl.insert(&t.name.text, sym),
                        };
                        let _ = e;
                    }
                }
            }
            // Stop when a round resolves nothing more.
            if next.len() == pending_len || _round == 7 {
                for (_, e) in next {
                    self.errors.push(e);
                }
                break;
            }
            pending = next.iter().map(|(t, _)| *t).collect();
            if pending.is_empty() {
                break;
            }
        }
    }

    pub fn ctx_for(&self, which: Option<usize>) -> Ctx<'_> {
        let mut layers = Vec::new();
        if let Some(i) = which {
            layers.push(&self.prog_scopes[i]);
        }
        layers.push(&self.ctrl);
        Ctx {
            env: &self.env,
            layers,
            programs: &self.programs,
            program_scopes: &self.prog_scopes,
        }
    }

    // ── Project ──

    pub fn project(&mut self, p: &Project) -> Out {
        let mut types = Out::new();
        let mut globals = Out::new();
        let mut pous = Out::new();

        self.register_datatypes(p);
        self.register_aois(p);

        globals.push_ctx(p.root_span);
        globals.s("VAR_GLOBAL\n");
        self.register_modules(p, &mut globals);
        let mut ctrl = std::mem::take(&mut self.ctrl);
        self.scope_tags(&p.tags, &mut ctrl, &mut globals, true);
        self.ctrl = ctrl;
        self.module_refs(p, &mut globals);
        self.scope_aliases(&p.tags, None);

        // Programs: scopes first (for Program:X.Tag and aliases).
        let mut prog_decls = Vec::new();
        for pr in &p.programs {
            let idx = self.prog_scopes.len();
            self.prog_scopes.push(Scope::default());
            let mut inst = ident(&pr.name.text);
            if self.globals.contains_key(&inst.to_ascii_lowercase())
                || self.env.lookup(&pr.name.text).is_some()
            {
                inst = format!("{inst}__prog");
            }
            self.globals.insert(inst.to_ascii_lowercase(), pr.name.span);
            self.programs
                .insert(pr.name.text.to_ascii_lowercase(), (inst.clone(), idx));
            let mut decl = Out::new();
            let mut scope = Scope::default();
            self.scope_tags(&pr.tags, &mut scope, &mut decl, false);
            self.prog_scopes[idx] = scope;
            prog_decls.push(decl);
        }
        for (idx, pr) in p.programs.iter().enumerate() {
            self.scope_aliases(&pr.tags, Some(idx));
        }
        // I/O map: every scope is known now.
        if !self.io.is_empty() {
            let mut io = std::mem::take(&mut self.io);
            let errs = io.resolve(&self.ctx_for(None), p.root_span);
            self.errors.extend(errs);
            globals.append(io.hidden_decls());
            self.io = io;
        }
        for (idx, pr) in p.programs.iter().enumerate() {
            let (inst, _) = self.programs[&pr.name.text.to_ascii_lowercase()].clone();
            globals.push_ctx(pr.name.span);
            globals
                .s("    ")
                .s(&inst)
                .s(" : lx__P_")
                .s(&ident(&pr.name.text))
                .s(";\n");
            globals.pop_ctx();
            let decl = std::mem::take(&mut prog_decls[idx]);
            let event = p
                .tasks
                .iter()
                .find(|t| {
                    t.kind == TaskType::Event
                        && t.programs
                            .iter()
                            .any(|n| n.text.eq_ignore_ascii_case(&pr.name.text))
                })
                .map(|t| format!("lx__event_{}", ident(&t.name.text)));
            self.program(pr, idx, decl, event, &mut pous);
        }
        for ev in self.event_flags(p) {
            globals.s("    ").s(&ev).s(" : BOOL;\n");
        }
        globals.s("END_VAR\n\n");
        globals.pop_ctx();

        // Types (after modules/AOIs registered everything).
        for def in self.env.structs.iter() {
            if matches!(def.kind, StructKind::Builtin | StructKind::Aoi) || def.opaque {
                continue;
            }
            let span = p
                .datatypes
                .iter()
                .find(|d| d.name.text.eq_ignore_ascii_case(&def.logix))
                .map_or(p.root_span, |d| d.span);
            types.push_ctx(span);
            types.s("TYPE ").s(&def.st).s(" :\nSTRUCT\n");
            for f in &def.fields {
                types
                    .s("    ")
                    .s(&f.st)
                    .s(" : ")
                    .s(&self.env.st(&f.ty))
                    .s(";\n");
            }
            if def.fields.is_empty() {
                types.s("    lx__empty : BOOL;\n");
            }
            types.s("END_STRUCT;\nEND_TYPE\n\n");
            types.pop_ctx();
        }

        let mut aois = Out::new();
        for a in &p.aois {
            self.aoi(a, &mut aois);
        }

        let mut config = Out::new();
        self.configuration(p, &mut config);

        let mut all = Out::new();
        all.append(types);
        all.append(aois);
        all.append(globals);
        all.append(pous);
        all.append(config);
        if !self.strings.is_empty() {
            all.push_ctx(p.root_span);
            all.s("\n").s(&self.strings.source());
            all.pop_ctx();
        }
        all
    }

    fn event_flags(&self, p: &Project) -> Vec<String> {
        p.tasks
            .iter()
            .filter(|t| t.kind == TaskType::Event)
            .map(|t| format!("lx__event_{}", ident(&t.name.text)))
            .collect()
    }

    fn routine_names(routines: &[RoutineDef]) -> HashMap<String, String> {
        routines
            .iter()
            .map(|r| {
                (
                    r.name.text.to_ascii_lowercase(),
                    format!("R_{}", ident(&r.name.text)),
                )
            })
            .collect()
    }

    fn program(
        &mut self,
        pr: &ProgramDef,
        idx: usize,
        decl: Out,
        event: Option<String>,
        out: &mut Out,
    ) {
        let fb = format!("lx__P_{}", ident(&pr.name.text));
        let routines = Self::routine_names(&pr.routines);
        let mut methods = Out::new();
        let mut prescan = Out::new();
        let mut errors = Vec::new();
        let mut ret_vars: Vec<String> = Vec::new();
        {
            let ctx = Ctx {
                env: &self.env,
                layers: vec![&self.prog_scopes[idx], &self.ctrl],
                programs: &self.programs,
                program_scopes: &self.prog_scopes,
            };
            let shared = Shared {
                src: self.src,
                env: &self.env,
                aois: &self.aois,
                strings: &self.strings,
                comments: self.annotate.then_some(&self.comments),
                long_rungs: self.long_rungs,
            };
            let sbr = rll::subroutines(&ctx, &pr.routines, &routines);
            for (method, tys) in &sbr.ret {
                for (k, t) in tys.iter().enumerate() {
                    ret_vars.push(format!(
                        "        {} : {};\n",
                        rll::Subs::ret_var(method, k),
                        self.env.st(t)
                    ));
                }
            }
            for r in &pr.routines {
                let name = &routines[&r.name.text.to_ascii_lowercase()];
                let lowered = if r.encoded {
                    errors.push(L5xError::new(
                        format!(
                            "routine `{}` is source-protected (encoded) and cannot be compiled",
                            r.name.text
                        ),
                        r.span,
                    ));
                    None
                } else {
                    match r.kind {
                        RoutineKind::Rll => {
                            Some(rll::routine_with(&shared, &ctx, r, &routines, false, &sbr))
                        }
                        RoutineKind::St => {
                            Some(stx::routine_with(&shared, &ctx, r, &routines, false, &sbr))
                        }
                        // `Use="Reference"/"Context"` placeholders of a
                        // component export: the routine is named (a JSR
                        // calls it) but not part of the file.
                        RoutineKind::Other if r.kind_text.is_empty() => {
                            errors.push(L5xError::warning(
                                format!(
                                    "routine `{}` is not part of this export; calling it does nothing",
                                    r.name.text
                                ),
                                r.span,
                            ));
                            Some((
                                RoutineOut {
                                    temps: Default::default(),
                                    body: Out::new(),
                                    prescan: Out::new(),
                                },
                                Vec::new(),
                            ))
                        }
                        _ => {
                            errors.push(L5xError::new(
                                format!(
                                    "routine `{}`: {} routines are not supported yet",
                                    r.name.text,
                                    if r.kind_text.is_empty() {
                                        "this kind of"
                                    } else {
                                        &r.kind_text
                                    }
                                ),
                                r.span,
                            ));
                            None
                        }
                    }
                };
                methods.push_ctx(r.span);
                methods.s("METHOD ").s(name).s("\n");
                if let Some((o, errs)) = lowered {
                    errors.extend(errs);
                    methods.s("VAR\n");
                    methods.append(o.temps_out());
                    methods.s("END_VAR\n");
                    methods.append(o.body);
                    prescan.append(o.prescan);
                }
                methods.s("END_METHOD\n\n");
                methods.pop_ctx();
            }
        }
        self.errors.extend(errors);

        out.push_ctx(pr.span);
        out.s("FUNCTION_BLOCK ").s(&fb).s("\nVAR\n");
        out.append(decl);
        for v in &ret_vars {
            out.s(v);
        }
        out.s("        lx__first : BOOL := TRUE;\n        lx__brk : BOOL;\n        lx__for_depth : DINT;\nEND_VAR\n");
        // S:FS is set during the program's first scan; the prescan pass runs
        // just before it (1756-RM003 "Math status flags", each instruction's
        // "Prescan" row).
        out.s("lx__S_FS := lx__first;\nIF lx__first THEN\n    lx__prescan();\n    lx__first := FALSE;\nEND_IF;\n");
        match &pr.main_routine {
            Some(m) => match routines.get(&m.text.to_ascii_lowercase()) {
                Some(name) => {
                    out.m(name, m.span).s("();\n");
                }
                None => {
                    self.err(format!("main routine `{}` does not exist", m.text), m.span);
                }
            },
            None => {
                self.warn(
                    format!(
                        "program `{}` has no main routine; it does nothing",
                        pr.name.text
                    ),
                    pr.name.span,
                );
            }
        }
        out.s("\nMETHOD lx__prescan\n");
        out.append(prescan);
        out.s("END_METHOD\n\n");
        out.append(methods);
        out.s("END_FUNCTION_BLOCK\n\n");
        // The runner program scheduled by the task.
        let (inst, _) = self.programs[&pr.name.text.to_ascii_lowercase()].clone();
        out.s("PROGRAM lx__run_").s(&ident(&pr.name.text)).s("\n");
        if let Some(flag) = &event {
            // An event task clears its trigger when it runs, so the next
            // EVENT instruction is a new rising edge of the task's SINGLE.
            out.s(flag).s(" := FALSE;\n");
        }
        let cin = self.io.copy_in(&self.ctx_for(None));
        out.append(cin);
        out.s(&inst).s("();\n");
        let cout = self.io.copy_out(&self.ctx_for(None));
        out.append(cout);
        out.s("END_PROGRAM\n\n");
        out.pop_ctx();
    }
    fn aoi(&mut self, a: &AoiDef, out: &mut Out) {
        let Some(sig) = self.aois.get(&a.name.text.to_ascii_lowercase()).cloned() else {
            return;
        };
        if a.execute_postscan {
            self.warn(
                format!("Add-On Instruction `{}`: the Postscan routine (SFC postscan) is not run by plcc", a.name.text),
                a.span,
            );
        }
        if a.encoded {
            self.err(
                format!(
                    "Add-On Instruction `{}` is source-protected (encoded) and cannot be compiled",
                    a.name.text
                ),
                a.span,
            );
        }
        let mut scope = Scope::default();
        let Some(id) = self.env.lookup(&a.name.text) else {
            return;
        };
        for f in self.env.get(id).fields.clone() {
            scope.insert(
                &f.logix,
                Sym {
                    st: f.st.clone(),
                    ty: Some(f.ty.clone()),
                    unknown_type: None,
                    decl: a.span,
                },
            );
        }
        out.push_ctx(a.span);
        out.s("FUNCTION_BLOCK ").s(&sig.st).s("\n");
        let mut has_enable_in = false;
        for (kw, usage) in [
            ("VAR_INPUT", Usage::Input),
            ("VAR_OUTPUT", Usage::Output),
            ("VAR_IN_OUT", Usage::InOut),
        ] {
            let items: Vec<&TagDef> = a
                .params
                .iter()
                .filter(|t| t.usage == usage && t.kind != TagKind::Alias)
                .collect();
            if items.is_empty() && usage != Usage::Input {
                continue;
            }
            out.s(kw).s("\n");
            for t in items {
                let Some(ty) = self.tag_ty(t) else { continue };
                if t.name.text.eq_ignore_ascii_case("EnableIn") {
                    has_enable_in = true;
                    out.m(&format!("    EnableIn : BOOL := TRUE;\n"), t.span);
                    continue;
                }
                let init = if usage == Usage::InOut {
                    None
                } else {
                    self.init_of(t, &ty)
                };
                out.push_ctx(t.span);
                out.s("    ")
                    .s(&ident(&t.name.text))
                    .s(" : ")
                    .s(&self.env.st(&ty));
                if let Some(i) = init {
                    out.s(" := ").s(&i);
                }
                out.s(";\n");
                out.pop_ctx();
            }
            if usage == Usage::Input {
                if !has_enable_in {
                    out.s("    EnableIn : BOOL := TRUE;\n");
                }
                out.s("    lx__prescan : BOOL;\n");
            }
            out.s("END_VAR\n");
        }
        if !a
            .params
            .iter()
            .any(|t| t.name.text.eq_ignore_ascii_case("EnableOut"))
        {
            out.s("VAR_OUTPUT\n    EnableOut : BOOL;\nEND_VAR\n");
        }
        let routines = Self::routine_names(&a.routines);
        let find = |n: &str| {
            a.routines
                .iter()
                .find(|r| r.name.text.eq_ignore_ascii_case(n))
        };
        let ctx = Ctx {
            env: &self.env,
            layers: vec![&scope],
            programs: &self.programs,
            program_scopes: &self.prog_scopes,
        };
        let shared = Shared {
            src: self.src,
            env: &self.env,
            aois: &self.aois,
            strings: &self.strings,
            comments: self.annotate.then_some(&self.comments),
            long_rungs: self.long_rungs,
        };
        let mut lowered: Vec<(&str, RoutineOut)> = Vec::new();
        let mut errs = Vec::new();
        for (role, name) in [
            ("logic", "Logic"),
            ("prescan", "Prescan"),
            ("postscan", "Postscan"),
            ("eif", "EnableInFalse"),
        ] {
            let Some(r) = find(name) else { continue };
            if role == "postscan" {
                continue;
            }
            if r.encoded {
                continue;
            }
            match r.kind {
                RoutineKind::Rll => {
                    let (o, e) = rll::routine(&shared, &ctx, r, &routines, true);
                    errs.extend(e);
                    lowered.push((role, o));
                }
                RoutineKind::St => {
                    let (o, e) = stx::routine(&shared, &ctx, r, &routines, true);
                    errs.extend(e);
                    lowered.push((role, o));
                }
                _ => errs.push(L5xError::new(
                    format!(
                        "AOI routine `{}`: {} routines are not supported yet",
                        r.name.text, r.kind_text
                    ),
                    r.span,
                )),
            }
        }
        self.errors.extend(errs);
        let mut temps: std::collections::BTreeSet<String> = Default::default();
        let mut body = Out::new();
        let mut take = |role: &str| -> Option<RoutineOut> {
            let i = lowered.iter().position(|(r, _)| *r == role)?;
            Some(lowered.remove(i).1)
        };
        let logic = take("logic");
        let pre = take("prescan");
        let eif = take("eif");
        // Prescan (called by the caller's prescan with lx__prescan := TRUE):
        // the Logic routine's instructions in prescan mode, then the Prescan
        // routine when "Execute Prescan" is set.
        body.s("IF lx__prescan THEN\n    lx__prescan := FALSE;\n");
        if let Some(l) = &logic {
            let _ = l;
        }
        let mut logic_body = None;
        if let Some(l) = logic {
            temps.extend(l.temps);
            body.append(l.prescan);
            logic_body = Some(l.body);
        }
        if let Some(p) = pre
            && a.execute_prescan
        {
            temps.extend(p.temps);
            body.append(p.body);
        }
        body.s("    EnableOut := FALSE;\n    RETURN;\nEND_IF;\n");
        // EnableOut follows EnableIn; the Logic routine runs only when
        // EnableIn is TRUE, the EnableInFalse routine (if "Execute
        // EnableInFalse" is set) only when it is FALSE.
        body.s("EnableOut := EnableIn;\nIF EnableIn THEN\n");
        if let Some(lb) = logic_body {
            body.append(lb);
        }
        body.s("END_IF;\n");
        if let Some(e) = eif
            && a.execute_enable_in_false
        {
            temps.extend(e.temps);
            body.s("IF NOT EnableIn THEN\n");
            body.append(e.body);
            body.s("END_IF;\n");
        }
        out.s("VAR\n");
        for t in &a.locals {
            if t.kind == TagKind::Alias {
                continue;
            }
            let Some(ty) = self.tag_ty(t) else { continue };
            let init = self.init_of(t, &ty);
            out.push_ctx(t.span);
            out.s("    ")
                .s(&ident(&t.name.text))
                .s(" : ")
                .s(&self.env.st(&ty));
            if let Some(i) = init {
                out.s(" := ").s(&i);
            }
            out.s(";\n");
            out.pop_ctx();
        }
        for tmp in &temps {
            out.s("    ").s(tmp).s(";\n");
        }
        out.s("END_VAR\n");
        out.append(body);
        out.s("END_FUNCTION_BLOCK\n\n");
        out.pop_ctx();
    }

    fn configuration(&mut self, p: &Project, out: &mut Out) {
        if p.tasks.is_empty() {
            return;
        }
        let mut scheduled: HashMap<String, bool> = HashMap::new();
        out.push_ctx(p.root_span);
        out.s("CONFIGURATION lx__controller\n");
        let mut instances = Out::new();
        for t in &p.tasks {
            if t.inhibited {
                self.warn(
                    format!("task `{}` is inhibited and does not run", t.name.text),
                    t.span,
                );
                continue;
            }
            let tname = ident(&t.name.text);
            let with = match t.kind {
                TaskType::Continuous => None,
                TaskType::Periodic => {
                    let ms = t.rate_ms.unwrap_or(10.0);
                    // Logix periodic rates run from 0.1 ms to 2,000,000 ms.
                    if !(0.1..=2_000_000.0).contains(&ms) {
                        self.err(
                            format!(
                                "task `{}`: rate {ms} ms is outside 0.1 .. 2,000,000 ms",
                                t.name.text
                            ),
                            t.span,
                        );
                        continue;
                    }
                    let us = (ms * 1000.0).round() as i64;
                    out.push_ctx(t.span);
                    out.s("    TASK ").s(&tname).s(&format!(
                        " (INTERVAL := T#{us}us, PRIORITY := {});\n",
                        t.priority
                    ));
                    out.pop_ctx();
                    Some(tname.clone())
                }
                TaskType::Event => {
                    let trig = t.event_trigger.clone().unwrap_or_default();
                    if !trig.eq_ignore_ascii_case("EVENT instruction only") {
                        self.warn(
                            format!(
                                "event task `{}`: trigger `{trig}`{} has no plcc equivalent; the task runs only when an EVENT instruction triggers it",
                                t.name.text,
                                t.event_tag.as_ref().map(|g| format!(" on `{g}`")).unwrap_or_default()
                            ),
                            t.span,
                        );
                    }
                    out.push_ctx(t.span);
                    out.s("    TASK ").s(&tname).s(&format!(
                        " (SINGLE := lx__event_{tname}, PRIORITY := {});\n",
                        t.priority
                    ));
                    out.pop_ctx();
                    Some(tname.clone())
                }
            };
            for pn in &t.programs {
                let Some(prog) = p
                    .programs
                    .iter()
                    .find(|x| x.name.text.eq_ignore_ascii_case(&pn.text))
                else {
                    self.err(
                        format!(
                            "task `{}` schedules unknown program `{}`",
                            t.name.text, pn.text
                        ),
                        pn.span,
                    );
                    continue;
                };
                if prog.disabled {
                    self.warn(
                        format!("program `{}` is disabled and does not run", prog.name.text),
                        pn.span,
                    );
                    continue;
                }
                if scheduled
                    .insert(prog.name.text.to_ascii_lowercase(), true)
                    .is_some()
                {
                    self.err(format!("program `{}` is scheduled twice", pn.text), pn.span);
                    continue;
                }
                let pid = ident(&prog.name.text);
                instances.push_ctx(pn.span);
                instances.s("    PROGRAM lx__i_").s(&pid);
                if let Some(w) = &with {
                    instances.s(" WITH ").s(w);
                }
                instances.s(" : lx__run_").s(&pid).s(";\n");
                instances.pop_ctx();
            }
        }
        out.append(instances);
        out.s("END_CONFIGURATION\n");
        out.pop_ctx();
        for pr in &p.programs {
            if !scheduled.contains_key(&pr.name.text.to_ascii_lowercase()) && !p.tasks.is_empty() {
                self.warn(
                    format!(
                        "program `{}` is not scheduled in any task and does not run",
                        pr.name.text
                    ),
                    pr.name.span,
                );
            }
        }
    }
}
