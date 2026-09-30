// SPDX-License-Identifier: MPL-2.0

//! What an editor needs to draw a block: its pins, per dialect.
//!
//! Logix operand names follow the instruction descriptions of Rockwell
//! 1756-RM003 (*Logix 5000 Controllers General Instructions*); IEC pins are the
//! formal parameters of IEC 61131-3 §6.6.3 (standard function blocks) and
//! the operator functions of §6.6.2.

use crate::model::{Dialect, PinDir};
use serde::Serialize;

/// Broad kind of an instruction, for a palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Bit,
    Timer,
    Counter,
    Edge,
    Compare,
    Math,
    Move,
    Logical,
    Expression,
    ProgramControl,
    File,
    String,
    Other,
}

/// How a block sits in a rung.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Logix input instruction: sets the rung condition for what follows
    /// (compares, ONS).
    Input,
    /// Logix output instruction: acts on the rung condition and passes it on.
    Output,
    /// IEC function block / function: power in on `power_in`, out on
    /// `power_out`.
    Box,
}

#[derive(Clone, Debug, Serialize)]
pub struct PinSpec {
    pub name: &'static str,
    pub dir: PinDir,
}

#[derive(Clone, Debug, Serialize)]
pub struct Spec {
    pub name: &'static str,
    pub category: Category,
    pub role: Role,
    pub pins: Vec<PinSpec>,
    /// The last pin repeats (JSR parameters, extensible ADD inputs).
    pub variadic: bool,
    /// IEC: the pin power enters / leaves by default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power_in: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power_out: Option<&'static str>,
    /// IEC: a function block (needs an instance), not a function.
    pub instance: bool,
}

use PinDir::{InOut as IO, Input as I, Output as O};

fn pins(list: &[(&'static str, PinDir)]) -> Vec<PinSpec> {
    list.iter()
        .map(|&(name, dir)| PinSpec { name, dir })
        .collect()
}

fn logix(
    name: &'static str,
    category: Category,
    role: Role,
    list: &[(&'static str, PinDir)],
) -> Spec {
    Spec {
        name,
        category,
        role,
        pins: pins(list),
        variadic: false,
        power_in: None,
        power_out: None,
        instance: false,
    }
}

/// The Logix instruction `name` (case-insensitive), if the catalog knows it.
pub fn logix_spec(name: &str) -> Option<Spec> {
    use Category as C;
    use Role::{Input as In, Output as Out};
    let up = name.to_ascii_uppercase();
    let s = match up.as_str() {
        "XIC" => logix("XIC", C::Bit, In, &[("Data Bit", I)]),
        "XIO" => logix("XIO", C::Bit, In, &[("Data Bit", I)]),
        "OTE" => logix("OTE", C::Bit, Out, &[("Data Bit", O)]),
        "OTL" => logix("OTL", C::Bit, Out, &[("Data Bit", O)]),
        "OTU" => logix("OTU", C::Bit, Out, &[("Data Bit", O)]),
        "ONS" => logix("ONS", C::Edge, In, &[("Storage Bit", IO)]),
        "OSR" => logix(
            "OSR",
            C::Edge,
            Out,
            &[("Storage Bit", IO), ("Output Bit", O)],
        ),
        "OSF" => logix(
            "OSF",
            C::Edge,
            Out,
            &[("Storage Bit", IO), ("Output Bit", O)],
        ),
        "TON" | "TOF" | "RTO" => logix(
            leak(&up),
            C::Timer,
            Out,
            &[("Timer", IO), ("Preset", I), ("Accum", I)],
        ),
        "CTU" | "CTD" => logix(
            leak(&up),
            C::Counter,
            Out,
            &[("Counter", IO), ("Preset", I), ("Accum", I)],
        ),
        "RES" => logix("RES", C::Counter, Out, &[("Structure", IO)]),
        "EQU" | "EQ" | "NEQ" | "NE" | "LES" | "LT" | "LEQ" | "LE" | "GRT" | "GT" | "GEQ" | "GE" => {
            logix(
                leak(&up),
                C::Compare,
                In,
                &[("Source A", I), ("Source B", I)],
            )
        }
        "LIM" | "LIMIT" => logix(
            leak(&up),
            C::Compare,
            In,
            &[("Low Limit", I), ("Test", I), ("High Limit", I)],
        ),
        "MEQ" => logix(
            "MEQ",
            C::Compare,
            In,
            &[("Source", I), ("Mask", I), ("Compare", I)],
        ),
        "CMP" => logix("CMP", C::Expression, In, &[("Expression", I)]),
        "ADD" | "SUB" | "MUL" | "DIV" | "MOD" | "XPY" => logix(
            leak(&up),
            C::Math,
            Out,
            &[("Source A", I), ("Source B", I), ("Dest", O)],
        ),
        "CPT" => logix("CPT", C::Expression, Out, &[("Dest", O), ("Expression", I)]),
        "NEG" | "ABS" | "SQR" | "SQRT" | "SIN" | "COS" | "TAN" | "ASN" | "ASIN" | "ACS"
        | "ACOS" | "ATN" | "ATAN" | "LN" | "LOG" | "DEG" | "RAD" | "TRN" | "TRUNC" | "TOD"
        | "FRD" => logix(leak(&up), C::Math, Out, &[("Source", I), ("Dest", O)]),
        "MOV" | "MOVE" => logix(leak(&up), C::Move, Out, &[("Source", I), ("Dest", O)]),
        "MVM" => logix(
            "MVM",
            C::Move,
            Out,
            &[("Source", I), ("Mask", I), ("Dest", IO)],
        ),
        "CLR" => logix("CLR", C::Move, Out, &[("Dest", O)]),
        "AND" | "OR" | "XOR" | "BAND" | "BOR" | "BXOR" => logix(
            leak(&up),
            C::Logical,
            Out,
            &[("Source A", I), ("Source B", I), ("Dest", O)],
        ),
        "NOT" => logix("NOT", C::Logical, Out, &[("Source", I), ("Dest", O)]),
        "BTD" => logix(
            "BTD",
            C::Move,
            Out,
            &[
                ("Source", I),
                ("Source Bit", I),
                ("Dest", IO),
                ("Dest Bit", I),
                ("Length", I),
            ],
        ),
        "COP" | "CPS" | "FLL" => logix(
            leak(&up),
            C::File,
            Out,
            &[("Source", I), ("Dest", O), ("Length", I)],
        ),
        "SWPB" => logix(
            "SWPB",
            C::Move,
            Out,
            &[("Source", I), ("Order Mode", I), ("Dest", O)],
        ),
        "CONCAT" => logix(
            "CONCAT",
            C::String,
            Out,
            &[("Source A", I), ("Source B", I), ("Dest", O)],
        ),
        "MID" | "DELETE" => logix(
            leak(&up),
            C::String,
            Out,
            &[("Source", I), ("Qty", I), ("Start", I), ("Dest", O)],
        ),
        "INSERT" => logix(
            "INSERT",
            C::String,
            Out,
            &[("Source A", I), ("Source B", I), ("Start", I), ("Dest", O)],
        ),
        "FIND" => logix(
            "FIND",
            C::String,
            Out,
            &[("Source", I), ("Search", I), ("Start", I), ("Result", O)],
        ),
        "UPPER" | "LOWER" | "DTOS" | "STOD" => {
            logix(leak(&up), C::String, Out, &[("Source", I), ("Dest", O)])
        }
        "BSL" | "BSR" => logix(
            leak(&up),
            C::File,
            Out,
            &[
                ("Array", IO),
                ("Control", IO),
                ("Source Bit", I),
                ("Length", I),
            ],
        ),
        "FFL" | "LFL" => logix(
            leak(&up),
            C::File,
            Out,
            &[
                ("Source", I),
                ("Stack", IO),
                ("Control", IO),
                ("Length", I),
                ("Position", I),
            ],
        ),
        "FFU" | "LFU" => logix(
            leak(&up),
            C::File,
            Out,
            &[
                ("Stack", IO),
                ("Dest", O),
                ("Control", IO),
                ("Length", I),
                ("Position", I),
            ],
        ),
        "JSR" => {
            let mut s = logix(
                "JSR",
                C::ProgramControl,
                Out,
                &[("Routine Name", I), ("Input Count", I), ("Parameter", IO)],
            );
            s.variadic = true;
            s
        }
        "SBR" | "RET" => {
            let mut s = logix(leak(&up), C::ProgramControl, Out, &[("Parameter", IO)]);
            s.variadic = true;
            s
        }
        "JMP" | "LBL" => logix(leak(&up), C::ProgramControl, Out, &[("Label Name", I)]),
        "MCR" | "TND" | "AFI" | "NOP" | "UID" | "UIE" | "BRK" => {
            logix(leak(&up), C::ProgramControl, Out, &[])
        }
        "FOR" => logix(
            "FOR",
            C::ProgramControl,
            Out,
            &[
                ("Routine Name", I),
                ("Index", IO),
                ("Initial Value", I),
                ("Terminal Value", I),
                ("Step Size", I),
            ],
        ),
        "EVENT" => logix("EVENT", C::ProgramControl, Out, &[("Task", I)]),
        "SIZE" => logix(
            "SIZE",
            C::File,
            Out,
            &[("Source", I), ("Dimension to Vary", I), ("Size", O)],
        ),
        "GSV" => logix(
            "GSV",
            C::Other,
            Out,
            &[
                ("Class Name", I),
                ("Instance Name", I),
                ("Attribute Name", I),
                ("Dest", O),
            ],
        ),
        "SSV" => logix(
            "SSV",
            C::Other,
            Out,
            &[
                ("Class Name", I),
                ("Instance Name", I),
                ("Attribute Name", I),
                ("Source", I),
            ],
        ),
        "MSG" => logix("MSG", C::Other, Out, &[("Message Control", IO)]),
        _ => return None,
    };
    Some(s)
}

/// Static names for the few specs built from the upper-cased input.
fn leak(up: &str) -> &'static str {
    const NAMES: &[&str] = &[
        "TON", "TOF", "RTO", "CTU", "CTD", "EQU", "EQ", "NEQ", "NE", "LES", "LT", "LEQ", "LE",
        "GRT", "GT", "GEQ", "GE", "LIM", "LIMIT", "ADD", "SUB", "MUL", "DIV", "MOD", "XPY", "NEG",
        "ABS", "SQR", "SQRT", "SIN", "COS", "TAN", "ASN", "ASIN", "ACS", "ACOS", "ATN", "ATAN",
        "LN", "LOG", "DEG", "RAD", "TRN", "TRUNC", "TOD", "FRD", "MOV", "MOVE", "AND", "OR", "XOR",
        "BAND", "BOR", "BXOR", "COP", "CPS", "FLL", "MID", "DELETE", "UPPER", "LOWER", "DTOS",
        "STOD", "BSL", "BSR", "FFL", "LFL", "FFU", "LFU", "SBR", "RET", "JMP", "LBL", "MCR", "TND",
        "AFI", "NOP", "UID", "UIE", "BRK",
    ];
    NAMES.iter().copied().find(|n| *n == up).unwrap_or("?")
}

fn iec_fb(
    name: &'static str,
    list: &[(&'static str, PinDir)],
    pin: &'static str,
    out: &'static str,
) -> Spec {
    let category = match name {
        "TON" | "TOF" | "TP" | "RTO" => Category::Timer,
        "CTU" | "CTD" | "CTUD" => Category::Counter,
        "R_TRIG" | "F_TRIG" => Category::Edge,
        _ => Category::Bit,
    };
    Spec {
        name,
        category,
        role: Role::Box,
        pins: pins(list),
        variadic: false,
        power_in: Some(pin),
        power_out: Some(out),
        instance: true,
    }
}

fn iec_fn(
    name: &'static str,
    category: Category,
    list: &[(&'static str, PinDir)],
    variadic: bool,
) -> Spec {
    Spec {
        name,
        category,
        role: Role::Box,
        pins: pins(list),
        variadic,
        power_in: Some("EN"),
        power_out: Some("ENO"),
        instance: false,
    }
}

/// The IEC standard function block or operator function `name`.
pub fn iec_spec(name: &str) -> Option<Spec> {
    use Category as C;
    let up = name.to_ascii_uppercase();
    Some(match up.as_str() {
        "TON" => iec_fb(
            "TON",
            &[("IN", I), ("PT", I), ("Q", O), ("ET", O)],
            "IN",
            "Q",
        ),
        "TOF" => iec_fb(
            "TOF",
            &[("IN", I), ("PT", I), ("Q", O), ("ET", O)],
            "IN",
            "Q",
        ),
        "TP" => iec_fb(
            "TP",
            &[("IN", I), ("PT", I), ("Q", O), ("ET", O)],
            "IN",
            "Q",
        ),
        // plcc's retentive on-delay (the Logix RTO as an IEC FB, see
        // docs/ladder-translation.md).
        "RTO" => iec_fb(
            "RTO",
            &[("IN", I), ("R", I), ("PT", I), ("Q", O), ("ET", O)],
            "IN",
            "Q",
        ),
        "CTU" => iec_fb(
            "CTU",
            &[("CU", I), ("R", I), ("PV", I), ("Q", O), ("CV", O)],
            "CU",
            "Q",
        ),
        "CTD" => iec_fb(
            "CTD",
            &[("CD", I), ("LD", I), ("PV", I), ("Q", O), ("CV", O)],
            "CD",
            "Q",
        ),
        "CTUD" => iec_fb(
            "CTUD",
            &[
                ("CU", I),
                ("CD", I),
                ("R", I),
                ("LD", I),
                ("PV", I),
                ("QU", O),
                ("QD", O),
                ("CV", O),
            ],
            "CU",
            "QU",
        ),
        "R_TRIG" => iec_fb("R_TRIG", &[("CLK", I), ("Q", O)], "CLK", "Q"),
        "F_TRIG" => iec_fb("F_TRIG", &[("CLK", I), ("Q", O)], "CLK", "Q"),
        "SR" => iec_fb("SR", &[("S1", I), ("R", I), ("Q1", O)], "S1", "Q1"),
        "RS" => iec_fb("RS", &[("S", I), ("R1", I), ("Q1", O)], "S", "Q1"),
        "ADD" => iec_fn("ADD", C::Math, &[("IN1", I), ("IN2", I), ("OUT", O)], true),
        "MUL" => iec_fn("MUL", C::Math, &[("IN1", I), ("IN2", I), ("OUT", O)], true),
        "SUB" => iec_fn("SUB", C::Math, &[("IN1", I), ("IN2", I), ("OUT", O)], false),
        "DIV" => iec_fn("DIV", C::Math, &[("IN1", I), ("IN2", I), ("OUT", O)], false),
        "MOD" => iec_fn("MOD", C::Math, &[("IN1", I), ("IN2", I), ("OUT", O)], false),
        "EXPT" => iec_fn(
            "EXPT",
            C::Math,
            &[("IN1", I), ("IN2", I), ("OUT", O)],
            false,
        ),
        "AND" => iec_fn(
            "AND",
            C::Logical,
            &[("IN1", I), ("IN2", I), ("OUT", O)],
            true,
        ),
        "OR" => iec_fn(
            "OR",
            C::Logical,
            &[("IN1", I), ("IN2", I), ("OUT", O)],
            true,
        ),
        "XOR" => iec_fn(
            "XOR",
            C::Logical,
            &[("IN1", I), ("IN2", I), ("OUT", O)],
            true,
        ),
        "NOT" => iec_fn("NOT", C::Logical, &[("IN", I), ("OUT", O)], false),
        "MOVE" => iec_fn("MOVE", C::Move, &[("IN", I), ("OUT", O)], false),
        "GT" | "GE" | "EQ" | "LE" | "LT" | "NE" => {
            let n = match up.as_str() {
                "GT" => "GT",
                "GE" => "GE",
                "EQ" => "EQ",
                "LE" => "LE",
                "LT" => "LT",
                _ => "NE",
            };
            iec_fn(
                n,
                C::Compare,
                &[("IN1", I), ("IN2", I), ("OUT", O)],
                n != "NE",
            )
        }
        "LIMIT" => iec_fn(
            "LIMIT",
            C::Compare,
            &[("MN", I), ("IN", I), ("MX", I), ("OUT", O)],
            false,
        ),
        "ABS" | "SQRT" | "SIN" | "COS" | "TAN" | "ASIN" | "ACOS" | "ATAN" | "LN" | "LOG"
        | "EXP" | "TRUNC" => {
            let n = [
                "ABS", "SQRT", "SIN", "COS", "TAN", "ASIN", "ACOS", "ATAN", "LN", "LOG", "EXP",
                "TRUNC",
            ]
            .into_iter()
            .find(|n| *n == up)
            .unwrap_or("ABS");
            iec_fn(n, C::Math, &[("IN", I), ("OUT", O)], false)
        }
        _ => return None,
    })
}

/// The spec of `name` in `dialect`.
pub fn spec(dialect: Dialect, name: &str) -> Option<Spec> {
    match dialect {
        Dialect::Iec => iec_spec(name),
        Dialect::Logix => logix_spec(name),
    }
}

/// Name of Logix operand `k` (0-based) of `mnemonic`: the catalog name, the
/// repeated last name for variadic instructions, else `Operand <k+1>`.
pub fn logix_operand_name(mnemonic: &str, k: usize) -> String {
    match logix_spec(mnemonic) {
        Some(s) if s.variadic && !s.pins.is_empty() && k + 1 >= s.pins.len() => {
            let base = s.pins[s.pins.len() - 1].name;
            format!("{base} {}", k + 2 - s.pins.len())
        }
        Some(s) if k < s.pins.len() => s.pins[k].name.to_string(),
        _ => format!("Operand {}", k + 1),
    }
}

/// Direction of Logix operand `k` of `mnemonic`.
pub fn logix_operand_dir(mnemonic: &str, k: usize) -> PinDir {
    match logix_spec(mnemonic) {
        Some(s) if k < s.pins.len() => s.pins[k].dir,
        Some(s) if s.variadic && !s.pins.is_empty() => s.pins[s.pins.len() - 1].dir,
        _ => PinDir::Input,
    }
}

/// Every instruction the catalog knows, for an editor's palette.
pub fn all(dialect: Dialect) -> Vec<Spec> {
    let names: &[&str] = match dialect {
        Dialect::Iec => &[
            "TON", "TOF", "TP", "RTO", "CTU", "CTD", "CTUD", "R_TRIG", "F_TRIG", "SR", "RS", "ADD",
            "SUB", "MUL", "DIV", "MOD", "EXPT", "AND", "OR", "XOR", "NOT", "MOVE", "GT", "GE",
            "EQ", "LE", "LT", "NE", "LIMIT", "ABS", "SQRT", "SIN", "COS", "TAN", "ASIN", "ACOS",
            "ATAN", "LN", "LOG", "EXP", "TRUNC",
        ],
        Dialect::Logix => &[
            "XIC", "XIO", "OTE", "OTL", "OTU", "ONS", "OSR", "OSF", "TON", "TOF", "RTO", "CTU",
            "CTD", "RES", "EQU", "NEQ", "LES", "LEQ", "GRT", "GEQ", "LIM", "MEQ", "CMP", "ADD",
            "SUB", "MUL", "DIV", "MOD", "XPY", "CPT", "NEG", "ABS", "SQR", "SIN", "COS", "TAN",
            "ASN", "ACS", "ATN", "LN", "LOG", "DEG", "RAD", "TRN", "TOD", "FRD", "MOV", "MVM",
            "CLR", "AND", "OR", "XOR", "NOT", "BTD", "COP", "CPS", "FLL", "SWPB", "CONCAT", "MID",
            "DELETE", "INSERT", "FIND", "UPPER", "LOWER", "DTOS", "STOD", "BSL", "BSR", "FFL",
            "FFU", "LFL", "LFU", "JSR", "SBR", "RET", "JMP", "LBL", "MCR", "TND", "AFI", "NOP",
            "UID", "UIE", "FOR", "BRK", "EVENT", "SIZE", "GSV", "SSV", "MSG",
        ],
    };
    names.iter().filter_map(|n| spec(dialect, n)).collect()
}
