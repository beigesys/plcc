// SPDX-License-Identifier: MPL-2.0

//! A source-level outline of a project's tags, for an editor's tag browser and
//! I/O view: every `AT` binding with its process-image location, each PROGRAM
//! and FUNCTION_BLOCK's variables, the VAR_GLOBALs, and the declared tasks.
//!
//! Process-image locations are exact (they follow from the address alone, see
//! docs/process-image.md). Offsets of ordinary variables inside program state
//! depend on code generation and are not here: the compiled module's symbol
//! table (`plcc compile --emit-symbols`) has them.

use crate::{Origin, Parsed};
use plcc_runtime::direct_address::{self, ParsedAddress};
use plcc_st::ast::{
    Declaration, Expression, ExpressionKind, TypeSpec, TypeSpecKind, VarBlock, VarBlockKind,
};
use serde::Serialize;

/// A variable bound to the process image with `AT`.
#[derive(Clone, Debug, Serialize)]
pub struct ImageTag {
    /// `Main.start`, or `GLOBAL.estop` for a VAR_GLOBAL.
    pub path: String,
    pub scope: String,
    pub name: String,
    /// Canonical address (`%IX0.3`, `%QW1`).
    pub address: String,
    /// `I`, `Q` or `M`.
    pub area: char,
    /// `X`, `B`, `W`, `D` or `L`.
    pub size_prefix: char,
    pub byte_offset: u32,
    /// Bit within the byte for an `X` address.
    pub bit: Option<u8>,
    /// Width in bits (1 for `X`).
    pub bits: u32,
    pub iec_type: String,
    pub file: String,
}

/// A declared variable.
#[derive(Clone, Debug, Serialize)]
pub struct VarTag {
    pub name: String,
    /// `var`, `var_input`, `var_output`, `var_in_out`, `var_global`, …
    pub kind: &'static str,
    pub iec_type: String,
    pub retain: bool,
    pub constant: bool,
    /// The `AT` address as written.
    pub at: Option<String>,
}

/// A PROGRAM or FUNCTION_BLOCK and its variables.
#[derive(Clone, Debug, Serialize)]
pub struct PouTags {
    pub name: String,
    /// `program` or `function_block`.
    pub kind: &'static str,
    pub file: String,
    pub variables: Vec<VarTag>,
}

/// A program instance of a task.
#[derive(Clone, Debug, Serialize)]
pub struct InstanceTag {
    pub name: String,
    pub program: String,
}

/// A TASK as declared (or the implicit `MainTask`).
#[derive(Clone, Debug, Serialize)]
pub struct TaskTag {
    pub name: String,
    /// Cyclic interval; `None` when not cyclic or not a literal.
    pub interval_ns: Option<i64>,
    pub priority: Option<u32>,
    /// The SINGLE trigger, as written.
    pub single: Option<String>,
    pub instances: Vec<InstanceTag>,
    /// No CONFIGURATION: the compiler's implicit task (its interval is the
    /// compile option `--task-interval`, T#20ms by default).
    pub implicit: bool,
}

/// The outline.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Tags {
    pub image: Vec<ImageTag>,
    pub programs: Vec<PouTags>,
    pub function_blocks: Vec<PouTags>,
    pub globals: Vec<VarTag>,
    pub tasks: Vec<TaskTag>,
}

fn kind_name(k: VarBlockKind) -> &'static str {
    match k {
        VarBlockKind::Var => "var",
        VarBlockKind::VarInput => "var_input",
        VarBlockKind::VarOutput => "var_output",
        VarBlockKind::VarInOut => "var_in_out",
        VarBlockKind::VarGlobal => "var_global",
        VarBlockKind::VarExternal => "var_external",
        VarBlockKind::VarTemp => "var_temp",
        VarBlockKind::VarAccess => "var_access",
        VarBlockKind::VarConfig => "var_config",
        VarBlockKind::VarInst => "var_inst",
        VarBlockKind::VarStat => "var_stat",
    }
}

fn expr_text(e: &Expression) -> String {
    match &e.kind {
        ExpressionKind::IntegerLiteral(v) => v.to_string(),
        ExpressionKind::Identifier(id) => id.name.clone(),
        ExpressionKind::TimeLiteral(t) => t.clone(),
        ExpressionKind::BoolLiteral(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        ExpressionKind::UnaryOp { operand, .. } => match &operand.kind {
            ExpressionKind::IntegerLiteral(v) => format!("-{v}"),
            _ => "?".to_string(),
        },
        _ => "?".to_string(),
    }
}

/// The type as IEC text (`INT`, `ARRAY[0..3] OF BOOL`, `STRING[20]`, `TON`).
pub fn type_text(t: &TypeSpec) -> String {
    match &t.kind {
        TypeSpecKind::Named(id) => id.name.clone(),
        TypeSpecKind::StringType { wide, length } => {
            let base = if *wide { "WSTRING" } else { "STRING" };
            match length {
                Some(l) => format!("{base}[{}]", expr_text(l)),
                None => base.to_string(),
            }
        }
        TypeSpecKind::Array { ranges, base } => {
            let r: Vec<String> = ranges
                .iter()
                .map(|r| format!("{}..{}", expr_text(&r.low), expr_text(&r.high)))
                .collect();
            format!("ARRAY[{}] OF {}", r.join(", "), type_text(base))
        }
        TypeSpecKind::VarLengthArray { dimensions, base } => {
            format!("ARRAY[{}] OF {}", vec!["*"; *dimensions].join(", "), type_text(base))
        }
        TypeSpecKind::Pointer(b) => format!("POINTER TO {}", type_text(b)),
        TypeSpecKind::Reference(b) => format!("REFERENCE TO {}", type_text(b)),
        TypeSpecKind::Subrange { base, low, high } => {
            format!("{}({}..{})", base.name, expr_text(low), expr_text(high))
        }
        TypeSpecKind::Struct(_) => "STRUCT".to_string(),
        TypeSpecKind::Enum(_) => "ENUM".to_string(),
        TypeSpecKind::Union(_) => "UNION".to_string(),
    }
}

/// Nanoseconds of a duration literal (`T#1s500ms`, `TIME#10ms`).
fn duration_ns(text: &str) -> Option<i64> {
    let t = text.trim();
    let body = ["LTIME#", "TIME#", "LT#", "T#"]
        .iter()
        .find_map(|p| {
            (t.len() > p.len() && t[..p.len()].eq_ignore_ascii_case(p)).then(|| &t[p.len()..])
        })
        .unwrap_or(t);
    let (neg, body) = match body.strip_prefix('-') {
        Some(b) => (true, b),
        None => (false, body),
    };
    let mut total: f64 = 0.0;
    let mut num = String::new();
    let mut chars = body.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() || c == '.' || c == '_' {
            if c != '_' {
                num.push(c);
            }
            chars.next();
            continue;
        }
        let mut unit = String::new();
        while let Some(&u) = chars.peek().filter(|u| u.is_ascii_alphabetic()) {
            unit.push(u);
            chars.next();
        }
        let mult = match unit.to_ascii_lowercase().as_str() {
            "d" => 86_400e9,
            "h" => 3_600e9,
            "m" => 60e9,
            "s" => 1e9,
            "ms" => 1e6,
            "us" => 1e3,
            "ns" => 1.0,
            _ => return None,
        };
        total += num.parse::<f64>().ok()? * mult;
        num.clear();
    }
    if !num.is_empty() {
        return None;
    }
    let v = total.round() as i64;
    Some(if neg { -v } else { v })
}

fn const_int(e: &Expression) -> Option<i128> {
    match &e.kind {
        ExpressionKind::IntegerLiteral(v) => Some(*v),
        ExpressionKind::TypedLiteral { value, .. } => const_int(value),
        _ => None,
    }
}

fn collect_vars(
    blocks: &[VarBlock],
    scope: &str,
    origin: &Origin,
    image: &mut Vec<ImageTag>,
) -> Vec<VarTag> {
    let mut vars = Vec::new();
    for b in blocks {
        for d in &b.declarations {
            let iec_type = type_text(&d.type_spec);
            if let Some(at) = &d.at_address
                && let Ok(ParsedAddress::Located(a)) = direct_address::parse(&at.repr)
            {
                image.push(ImageTag {
                    path: format!("{scope}.{}", d.name.name),
                    scope: scope.to_string(),
                    name: d.name.name.clone(),
                    address: a.to_string(),
                    area: a.area.letter(),
                    size_prefix: a.size.letter(),
                    byte_offset: a.byte,
                    bit: a.bit,
                    bits: a.size.bits(),
                    iec_type: iec_type.clone(),
                    file: origin.name.clone(),
                });
            }
            vars.push(VarTag {
                name: d.name.name.clone(),
                kind: kind_name(b.kind),
                iec_type,
                retain: b.is_retain,
                constant: b.is_constant,
                at: d.at_address.as_ref().map(|a| a.repr.clone()),
            });
        }
    }
    vars
}

/// The outline of the user's declarations (bundled libraries left out).
pub fn tags(parsed: &Parsed) -> Tags {
    let mut t = Tags::default();
    let mut programs_in_order = Vec::new();
    let mut config_seen = false;
    for (decl, origin) in parsed.unit.declarations.iter().zip(&parsed.origins) {
        if origin.prelude {
            continue;
        }
        match decl {
            Declaration::Program(p) => {
                let variables = collect_vars(&p.var_blocks, &p.name.name, origin, &mut t.image);
                programs_in_order.push(p.name.name.clone());
                t.programs.push(PouTags {
                    name: p.name.name.clone(),
                    kind: "program",
                    file: origin.name.clone(),
                    variables,
                });
            }
            Declaration::FunctionBlock(fb) => {
                let variables = collect_vars(&fb.var_blocks, &fb.name.name, origin, &mut t.image);
                t.function_blocks.push(PouTags {
                    name: fb.name.name.clone(),
                    kind: "function_block",
                    file: origin.name.clone(),
                    variables,
                });
            }
            Declaration::GlobalVarDecl(b) => {
                t.globals.extend(collect_vars(
                    std::slice::from_ref(b),
                    "GLOBAL",
                    origin,
                    &mut t.image,
                ));
            }
            Declaration::Configuration(c) if !config_seen => {
                config_seen = true;
                let blocks: Vec<VarBlock> = c
                    .global_vars
                    .iter()
                    .chain(c.resources.iter().flat_map(|r| r.global_vars.iter()))
                    .cloned()
                    .collect();
                t.globals
                    .extend(collect_vars(&blocks, "GLOBAL", origin, &mut t.image));
                let multi = c.resources.len() > 1;
                let mut background: Option<usize> = None;
                for r in &c.resources {
                    let first = t.tasks.len();
                    for task in &r.tasks {
                        let mut tag = TaskTag {
                            name: if multi {
                                format!("{}.{}", r.name.name, task.name.name)
                            } else {
                                task.name.name.clone()
                            },
                            interval_ns: None,
                            priority: None,
                            single: None,
                            instances: Vec::new(),
                            implicit: false,
                        };
                        for (k, v) in &task.properties {
                            match k.name.to_ascii_uppercase().as_str() {
                                "INTERVAL" => {
                                    tag.interval_ns = match &v.kind {
                                        ExpressionKind::TimeLiteral(s) => duration_ns(s),
                                        _ => None,
                                    }
                                }
                                "PRIORITY" => {
                                    tag.priority = const_int(v).and_then(|n| u32::try_from(n).ok())
                                }
                                "SINGLE" => tag.single = Some(expr_text(v)),
                                _ => {}
                            }
                        }
                        t.tasks.push(tag);
                    }
                    for pc in &r.program_configs {
                        let inst = InstanceTag {
                            name: format!("{}.{}", r.name.name, pc.name.name),
                            program: pc.program_type.name.clone(),
                        };
                        let idx = match &pc.task {
                            Some(tn) => r
                                .tasks
                                .iter()
                                .position(|x| x.name.name.eq_ignore_ascii_case(&tn.name))
                                .map(|i| first + i),
                            None => Some(*background.get_or_insert_with(|| {
                                t.tasks.push(TaskTag {
                                    name: "__background".to_string(),
                                    interval_ns: Some(0),
                                    priority: Some(u32::MAX),
                                    single: None,
                                    instances: Vec::new(),
                                    implicit: true,
                                });
                                t.tasks.len() - 1
                            })),
                        };
                        if let Some(i) = idx {
                            t.tasks[i].instances.push(inst);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if !config_seen && !programs_in_order.is_empty() {
        t.tasks.push(TaskTag {
            name: "MainTask".to_string(),
            interval_ns: None,
            priority: Some(1),
            single: None,
            instances: programs_in_order
                .iter()
                .map(|p| InstanceTag {
                    name: p.clone(),
                    program: p.clone(),
                })
                .collect(),
            implicit: true,
        });
    }
    t
}

#[cfg(test)]
mod tests {
    use super::duration_ns;

    #[test]
    fn durations() {
        assert_eq!(duration_ns("T#10ms"), Some(10_000_000));
        assert_eq!(duration_ns("TIME#1s500ms"), Some(1_500_000_000));
        assert_eq!(duration_ns("t#1.5s"), Some(1_500_000_000));
        assert_eq!(duration_ns("T#5x"), None);
    }
}
