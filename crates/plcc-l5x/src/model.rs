// SPDX-License-Identifier: MPL-2.0

//! The L5X document as a plain model: data types, Add-On Instructions, modules,
//! tags, programs and their routines, tasks. Nothing is lowered here; values
//! keep a handle on their XML node so they can be converted once every type is
//! known. Element and attribute names follow the Logix 5000 Controllers
//! Import/Export Reference Manual (1756-RM014, formerly 1756-RM084).

use crate::error::L5xError;
use crate::xml::{self, Text, XNode};
use plcc_st::Span;

#[derive(Clone, Debug)]
pub(crate) struct Name {
    pub text: String,
    pub span: Span,
}

pub(crate) struct Project<'a, 'i> {
    pub datatypes: Vec<DataTypeDef>,
    pub aois: Vec<AoiDef<'a, 'i>>,
    pub modules: Vec<ModuleDef<'a, 'i>>,
    pub tags: Vec<TagDef<'a, 'i>>,
    pub programs: Vec<ProgramDef<'a, 'i>>,
    pub tasks: Vec<TaskDef>,
    /// Span of the `<Controller>` start tag (or the root) for project-wide
    /// diagnostics.
    pub root_span: Span,
}

pub(crate) struct DataTypeDef {
    pub name: Name,
    /// `StringFamily` for string types (`LEN` + `DATA`).
    pub string_family: bool,
    /// `Class="ProductDefined"` (TIMER, PID, AXIS_...) or `"IO"` (module
    /// types): exports with dependencies list them too.
    pub predefined: bool,
    pub members: Vec<MemberDef>,
    pub span: Span,
}

pub(crate) struct MemberDef {
    pub name: Name,
    pub data_type: Name,
    pub dim: u32,
    pub hidden: bool,
    /// `DataType="BIT"` members: the hidden host member and the bit number.
    pub bit_of: Option<(String, u32)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TagKind {
    Base,
    Alias,
    Produced,
    Consumed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Usage {
    Local,
    Input,
    Output,
    InOut,
    Public,
}

pub(crate) struct TagDef<'a, 'i> {
    pub name: Name,
    pub kind: TagKind,
    pub data_type: Option<Name>,
    pub dims: Vec<u32>,
    pub alias_for: Option<Text>,
    #[allow(dead_code)] // read-only in Logix; plcc does not enforce it
    pub constant: bool,
    pub usage: Usage,
    pub required: bool,
    /// `<Data Format="Decorated">` / `<DefaultData Format="Decorated">`.
    pub decorated: Option<XNode<'a, 'i>>,
    /// `<Data Format="L5K">` / `<DefaultData Format="L5K">`.
    pub l5k: Option<Text>,
    /// `<Data Format="String">` (string tags).
    pub string_data: Option<Text>,
    pub span: Span,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RoutineKind {
    Rll,
    St,
    Fbd,
    Sfc,
    Other,
}

pub(crate) struct Rung {
    pub number: Option<u32>,
    pub text: Option<Text>,
    pub span: Span,
}

pub(crate) struct RoutineDef {
    pub name: Name,
    pub kind: RoutineKind,
    pub kind_text: String,
    pub rungs: Vec<Rung>,
    /// ST lines, in order.
    pub lines: Vec<Text>,
    pub encoded: bool,
    pub span: Span,
}

pub(crate) struct AoiDef<'a, 'i> {
    pub name: Name,
    pub params: Vec<TagDef<'a, 'i>>,
    pub locals: Vec<TagDef<'a, 'i>>,
    pub routines: Vec<RoutineDef>,
    pub execute_prescan: bool,
    pub execute_postscan: bool,
    pub execute_enable_in_false: bool,
    pub encoded: bool,
    pub span: Span,
}

pub(crate) struct ProgramDef<'a, 'i> {
    pub name: Name,
    pub main_routine: Option<Name>,
    pub disabled: bool,
    pub tags: Vec<TagDef<'a, 'i>>,
    pub routines: Vec<RoutineDef>,
    pub span: Span,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TaskType {
    Continuous,
    Periodic,
    Event,
}

pub(crate) struct TaskDef {
    pub name: Name,
    pub kind: TaskType,
    /// Rate in ms (periodic).
    pub rate_ms: Option<f64>,
    pub priority: u32,
    pub inhibited: bool,
    pub event_trigger: Option<String>,
    pub event_tag: Option<String>,
    pub programs: Vec<Name>,
    pub span: Span,
}

pub(crate) struct ModuleDef<'a, 'i> {
    pub name: Name,
    pub parent: String,
    pub parent_port: Option<u32>,
    /// Address of the upstream port (the slot, in a chassis).
    pub slot: Option<String>,
    /// `(Id, Type)` of every port.
    pub ports: Vec<(u32, String)>,
    /// `(suffix, data node)` of `InputTag`/`OutputTag`/`ConfigTag`; suffix is
    /// `I`, `O` or `C`.
    pub io_tags: Vec<(&'static str, Option<XNode<'a, 'i>>, Span)>,
    #[allow(dead_code)]
    pub span: Span,
}

pub(crate) struct Reader<'s> {
    pub src: &'s str,
    pub errors: Vec<L5xError>,
}

impl<'s> Reader<'s> {
    fn name_attr(&mut self, n: XNode, a: &str) -> Option<Name> {
        match xml::attr(n, a).map(str::trim).filter(|s| !s.is_empty()) {
            Some(v) => Some(Name {
                text: v.to_string(),
                span: xml::attr_span(self.src, n, a),
            }),
            None => {
                self.errors.push(L5xError::new(
                    format!("<{}> is missing its `{a}` attribute", xml::name(n)),
                    xml::tag_span(self.src, n),
                ));
                None
            }
        }
    }

    fn opt_name(&self, n: XNode, a: &str) -> Option<Name> {
        xml::attr(n, a)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|v| Name {
                text: v.to_string(),
                span: xml::attr_span(self.src, n, a),
            })
    }

    pub fn project<'a, 'i>(&mut self, root: XNode<'a, 'i>) -> Project<'a, 'i> {
        // Full exports have the content under <Controller>; component exports
        // (a rung, routine, program, AOI or data type) wrap theirs in a
        // <Controller Use="Context"> as well.
        let ctrl = xml::child(root, "Controller").unwrap_or(root);
        let mut p = Project {
            datatypes: Vec::new(),
            aois: Vec::new(),
            modules: Vec::new(),
            tags: Vec::new(),
            programs: Vec::new(),
            tasks: Vec::new(),
            root_span: xml::tag_span(self.src, ctrl),
        };
        if let Some(dts) = xml::child(ctrl, "DataTypes") {
            for dt in xml::children(dts, "DataType") {
                if let Some(d) = self.datatype(dt) {
                    p.datatypes.push(d);
                }
            }
        }
        if let Some(mods) = xml::child(ctrl, "Modules") {
            for m in xml::children(mods, "Module") {
                if let Some(d) = self.module(m) {
                    p.modules.push(d);
                }
            }
        }
        if let Some(aois) = xml::child(ctrl, "AddOnInstructionDefinitions") {
            for a in xml::children(aois, "AddOnInstructionDefinition") {
                if let Some(d) = self.aoi(a) {
                    p.aois.push(d);
                }
            }
        }
        if let Some(tags) = xml::child(ctrl, "Tags") {
            p.tags = self.tags(tags, "Tag");
        }
        if let Some(progs) = xml::child(ctrl, "Programs") {
            for pr in xml::children(progs, "Program") {
                if let Some(d) = self.program(pr) {
                    p.programs.push(d);
                }
            }
        }
        if let Some(tasks) = xml::child(ctrl, "Tasks") {
            for t in xml::children(tasks, "Task") {
                if let Some(d) = self.task(t) {
                    p.tasks.push(d);
                }
            }
        }
        p
    }

    fn datatype(&mut self, dt: XNode) -> Option<DataTypeDef> {
        let name = self.name_attr(dt, "Name")?;
        let string_family = xml::attr(dt, "Family") == Some("StringFamily");
        let predefined = matches!(xml::attr(dt, "Class"), Some("ProductDefined") | Some("IO"));
        let mut members = Vec::new();
        if let Some(ms) = xml::child(dt, "Members") {
            for m in xml::children(ms, "Member") {
                let Some(mname) = self.name_attr(m, "Name") else {
                    continue;
                };
                let Some(ty) = self.name_attr(m, "DataType") else {
                    continue;
                };
                let dim = xml::attr(m, "Dimension")
                    .and_then(|d| d.trim().parse().ok())
                    .unwrap_or(0);
                let bit_of = if ty.text.eq_ignore_ascii_case("BIT") {
                    let target = xml::attr(m, "Target").unwrap_or("").to_string();
                    let bit = xml::attr(m, "BitNumber")
                        .and_then(|b| b.trim().parse().ok())
                        .unwrap_or(0);
                    Some((target, bit))
                } else {
                    None
                };
                members.push(MemberDef {
                    name: mname,
                    data_type: ty,
                    dim,
                    hidden: xml::attr_bool(m, "Hidden"),
                    bit_of,
                });
            }
        }
        Some(DataTypeDef {
            name,
            string_family,
            predefined,
            members,
            span: xml::tag_span(self.src, dt),
        })
    }

    fn module<'a, 'i>(&mut self, m: XNode<'a, 'i>) -> Option<ModuleDef<'a, 'i>> {
        // `Use="Reference"` placeholders of component exports carry no name;
        // neither do some tool-made files. No tags can be derived from one.
        if xml::attr(m, "Name").is_none() {
            if xml::attr(m, "Use") != Some("Reference") {
                self.errors.push(L5xError::warning(
                    "<Module> without a Name: its I/O tags cannot be named and are left out",
                    xml::tag_span(self.src, m),
                ));
            }
            return None;
        }
        let name = self.name_attr(m, "Name")?;
        let parent = xml::attr(m, "ParentModule").unwrap_or("").to_string();
        let parent_port = xml::attr(m, "ParentModPortId").and_then(|v| v.trim().parse().ok());
        let mut slot = None;
        let mut port_list = Vec::new();
        if let Some(ports) = xml::child(m, "Ports") {
            for port in xml::children(ports, "Port") {
                if xml::attr_bool(port, "Upstream") {
                    slot = xml::attr(port, "Address").map(str::to_string);
                }
                if let Some(id) = xml::attr(port, "Id").and_then(|v| v.trim().parse().ok()) {
                    port_list.push((id, xml::attr(port, "Type").unwrap_or("").to_string()));
                }
            }
        }
        let mut io_tags = Vec::new();
        if let Some(comm) = xml::child(m, "Communications") {
            if let Some(cfg) = xml::child(comm, "ConfigTag") {
                io_tags.push(("C", decorated(cfg), xml::tag_span(self.src, cfg)));
            }
            if let Some(conns) = xml::child(comm, "Connections") {
                for c in xml::children(conns, "Connection") {
                    // Standard and (GuardLogix) safety connections.
                    for (elem, suffix) in [
                        ("InputTag", "I"),
                        ("OutputTag", "O"),
                        ("SafetyInputTag", "SI"),
                        ("SafetyOutputTag", "SO"),
                    ] {
                        if let Some(t) = xml::child(c, elem) {
                            io_tags.push((suffix, decorated(t), xml::tag_span(self.src, t)));
                        }
                    }
                }
            }
        }
        Some(ModuleDef {
            name,
            parent,
            parent_port,
            slot,
            ports: port_list,
            io_tags,
            span: xml::tag_span(self.src, m),
        })
    }

    pub fn tags<'a, 'i>(
        &mut self,
        parent: XNode<'a, 'i>,
        elem: &'static str,
    ) -> Vec<TagDef<'a, 'i>> {
        let mut out = Vec::new();
        for t in xml::children(parent, elem) {
            if let Some(d) = self.tag(t) {
                out.push(d);
            }
        }
        out
    }

    fn tag<'a, 'i>(&mut self, t: XNode<'a, 'i>) -> Option<TagDef<'a, 'i>> {
        let name = self.name_attr(t, "Name")?;
        let kind = match xml::attr(t, "TagType").unwrap_or("Base") {
            "Alias" => TagKind::Alias,
            "Produced" => TagKind::Produced,
            "Consumed" => TagKind::Consumed,
            _ => TagKind::Base,
        };
        let data_type = self.opt_name(t, "DataType");
        let dims = xml::attr(t, "Dimensions")
            .map(|d| {
                d.split([' ', ','])
                    .filter_map(|x| x.trim().parse().ok())
                    .filter(|&x: &u32| x > 0)
                    .collect()
            })
            .unwrap_or_default();
        let usage = match xml::attr(t, "Usage") {
            Some("Input") => Usage::Input,
            Some("Output") => Usage::Output,
            Some("InOut") => Usage::InOut,
            Some("Public") => Usage::Public,
            _ => Usage::Local,
        };
        let mut decorated_node = None;
        let mut l5k = None;
        let mut string_data = None;
        for d in xml::elements(t) {
            if !matches!(xml::name(d), "Data" | "DefaultData") {
                continue;
            }
            match xml::attr(d, "Format") {
                Some("Decorated") => decorated_node = Some(d),
                Some("L5K") => l5k = Some(Text::content(self.src, d)),
                Some("String") => string_data = Some(Text::content(self.src, d)),
                _ => {}
            }
        }
        Some(TagDef {
            name,
            kind,
            data_type,
            dims,
            alias_for: Text::attr(self.src, t, "AliasFor"),
            constant: xml::attr_bool(t, "Constant"),
            usage,
            required: xml::attr_bool(t, "Required"),
            decorated: decorated_node,
            l5k,
            string_data,
            span: xml::tag_span(self.src, t),
        })
    }

    fn aoi<'a, 'i>(&mut self, a: XNode<'a, 'i>) -> Option<AoiDef<'a, 'i>> {
        let name = self.name_attr(a, "Name")?;
        let params = xml::child(a, "Parameters")
            .map(|p| self.tags(p, "Parameter"))
            .unwrap_or_default();
        let locals = xml::child(a, "LocalTags")
            .map(|p| self.tags(p, "LocalTag"))
            .unwrap_or_default();
        let routines = xml::child(a, "Routines")
            .map(|r| self.routines(r))
            .unwrap_or_default();
        Some(AoiDef {
            name,
            params,
            locals,
            routines,
            execute_prescan: xml::attr_bool(a, "ExecutePrescan"),
            execute_postscan: xml::attr_bool(a, "ExecutePostscan"),
            execute_enable_in_false: xml::attr_bool(a, "ExecuteEnableInFalse"),
            encoded: xml::child(a, "EncodedData").is_some() || xml::name(a) == "EncodedData",
            span: xml::tag_span(self.src, a),
        })
    }

    fn routines(&mut self, parent: XNode) -> Vec<RoutineDef> {
        let mut out = Vec::new();
        for r in xml::elements(parent) {
            let encoded = xml::name(r) == "EncodedData";
            if xml::name(r) != "Routine" && !encoded {
                continue;
            }
            let Some(name) = self.name_attr(r, "Name") else {
                continue;
            };
            let kind_text = xml::attr(r, "Type").unwrap_or("").to_string();
            let kind = match kind_text.as_str() {
                "RLL" => RoutineKind::Rll,
                "ST" => RoutineKind::St,
                "FBD" => RoutineKind::Fbd,
                "SFC" => RoutineKind::Sfc,
                _ => RoutineKind::Other,
            };
            let mut rungs = Vec::new();
            let mut lines = Vec::new();
            if let Some(rll) = xml::child(r, "RLLContent") {
                for rung in xml::children(rll, "Rung") {
                    rungs.push(Rung {
                        number: xml::attr(rung, "Number").and_then(|n| n.trim().parse().ok()),
                        text: xml::child(rung, "Text").map(|t| Text::content(self.src, t)),
                        span: xml::tag_span(self.src, rung),
                    });
                }
            }
            if let Some(st) = xml::child(r, "STContent") {
                for line in xml::children(st, "Line") {
                    lines.push(Text::content(self.src, line));
                }
            }
            out.push(RoutineDef {
                name,
                kind,
                kind_text,
                rungs,
                lines,
                encoded: encoded || xml::child(r, "EncodedData").is_some(),
                span: xml::tag_span(self.src, r),
            });
        }
        out
    }

    fn program<'a, 'i>(&mut self, p: XNode<'a, 'i>) -> Option<ProgramDef<'a, 'i>> {
        let name = self.name_attr(p, "Name")?;
        let tags = xml::child(p, "Tags")
            .map(|t| self.tags(t, "Tag"))
            .unwrap_or_default();
        let routines = xml::child(p, "Routines")
            .map(|r| self.routines(r))
            .unwrap_or_default();
        Some(ProgramDef {
            main_routine: self.opt_name(p, "MainRoutineName"),
            disabled: xml::attr_bool(p, "Disabled"),
            name,
            tags,
            routines,
            span: xml::tag_span(self.src, p),
        })
    }

    fn task(&mut self, t: XNode) -> Option<TaskDef> {
        let name = self.name_attr(t, "Name")?;
        let kind = match xml::attr(t, "Type")
            .map(|s| s.to_ascii_uppercase())
            .as_deref()
        {
            Some("PERIODIC") => TaskType::Periodic,
            Some("EVENT") => TaskType::Event,
            _ => TaskType::Continuous,
        };
        let mut programs = Vec::new();
        if let Some(sp) = xml::child(t, "ScheduledPrograms") {
            for s in xml::children(sp, "ScheduledProgram") {
                if let Some(n) = self.name_attr(s, "Name") {
                    programs.push(n);
                }
            }
        }
        let ev = xml::child(t, "EventInfo");
        Some(TaskDef {
            name,
            kind,
            rate_ms: xml::attr(t, "Rate").and_then(|r| r.trim().parse().ok()),
            priority: xml::attr(t, "Priority")
                .and_then(|r| r.trim().parse().ok())
                .unwrap_or(10),
            inhibited: xml::attr_bool(t, "InhibitTask"),
            event_trigger: ev
                .and_then(|e| xml::attr(e, "EventTrigger"))
                .map(str::to_string),
            event_tag: ev
                .and_then(|e| xml::attr(e, "EventTag"))
                .map(str::to_string),
            programs,
            span: xml::tag_span(self.src, t),
        })
    }
}

fn decorated<'a, 'i>(n: XNode<'a, 'i>) -> Option<XNode<'a, 'i>> {
    xml::children(n, "Data").find(|d| xml::attr(*d, "Format") == Some("Decorated"))
}
