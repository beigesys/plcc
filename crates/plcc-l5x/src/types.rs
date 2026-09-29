// SPDX-License-Identifier: MPL-2.0

//! Logix data types as plcc sees them: the elementary types, the predefined
//! structures (TIMER, COUNTER, CONTROL, STRING, ...), user-defined types, Add-On
//! Instruction backing tags and module-defined I/O types.
//!
//! Layout is plcc's, not Logix's: a UDT's BOOL members (`DataType="BIT"`, packed
//! into hidden SINT hosts by Logix) become ordinary BOOL fields and the hidden
//! hosts are dropped. Member access, initial values and every instruction behave
//! the same; only byte-level views of a structure (COP between unlike types)
//! differ. See docs/l5x.md.

use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub(crate) enum Elem {
    Bool,
    Sint,
    Int,
    Dint,
    Lint,
    Usint,
    Uint,
    Udint,
    Ulint,
    Real,
    Lreal,
    Time,
    Ltime,
    Dt,
    Ldt,
}

impl Elem {
    pub fn parse(name: &str) -> Option<Elem> {
        Some(match name.to_ascii_uppercase().as_str() {
            "BOOL" | "BIT" => Elem::Bool,
            "SINT" => Elem::Sint,
            "INT" => Elem::Int,
            "DINT" => Elem::Dint,
            "LINT" => Elem::Lint,
            "USINT" => Elem::Usint,
            "UINT" => Elem::Uint,
            "UDINT" => Elem::Udint,
            "ULINT" => Elem::Ulint,
            "REAL" => Elem::Real,
            "LREAL" => Elem::Lreal,
            "TIME" | "TIME32" => Elem::Time,
            "LTIME" => Elem::Ltime,
            "DT" => Elem::Dt,
            "LDT" => Elem::Ldt,
            _ => return None,
        })
    }

    /// The plcc (IEC) type name.
    pub fn st(self) -> &'static str {
        match self {
            Elem::Bool => "BOOL",
            Elem::Sint => "SINT",
            Elem::Int => "INT",
            Elem::Dint => "DINT",
            Elem::Lint => "LINT",
            Elem::Usint => "USINT",
            Elem::Uint => "UINT",
            Elem::Udint => "UDINT",
            Elem::Ulint => "ULINT",
            Elem::Real => "REAL",
            Elem::Lreal => "LREAL",
            Elem::Time => "TIME",
            Elem::Ltime => "LTIME",
            Elem::Dt => "DT",
            Elem::Ldt => "LDT",
        }
    }

    pub fn is_int(self) -> bool {
        matches!(
            self,
            Elem::Sint
                | Elem::Int
                | Elem::Dint
                | Elem::Lint
                | Elem::Usint
                | Elem::Uint
                | Elem::Udint
                | Elem::Ulint
        )
    }

    pub fn is_real(self) -> bool {
        matches!(self, Elem::Real | Elem::Lreal)
    }

    pub fn is_num(self) -> bool {
        self.is_int() || self.is_real()
    }

    /// Conversion ranking of 1756-RM003 "Data conversions": SINT, USINT, INT,
    /// UINT, DINT, UDINT, LINT, ULINT, REAL, LREAL rank 1 (lowest) to 10.
    pub fn rank(self) -> u8 {
        match self {
            Elem::Bool => 0,
            Elem::Sint => 1,
            Elem::Usint => 2,
            Elem::Int => 3,
            Elem::Uint => 4,
            Elem::Dint => 5,
            Elem::Udint => 6,
            Elem::Lint => 7,
            Elem::Ulint => 8,
            Elem::Real => 9,
            Elem::Lreal => 10,
            _ => 7,
        }
    }

    /// Bit width of an integer or BOOL.
    pub fn bits(self) -> u32 {
        match self {
            Elem::Bool => 1,
            Elem::Sint | Elem::Usint => 8,
            Elem::Int | Elem::Uint => 16,
            Elem::Dint | Elem::Udint | Elem::Real => 32,
            _ => 64,
        }
    }

    pub fn signed(self) -> bool {
        !matches!(
            self,
            Elem::Usint | Elem::Uint | Elem::Udint | Elem::Ulint | Elem::Bool
        )
    }
}

#[derive(Clone, PartialEq, Debug)]
pub(crate) enum Ty {
    Elem(Elem),
    /// Index into [`TypeEnv::structs`].
    Struct(usize),
    /// Element type and dimensions (zero-based).
    Array(Box<Ty>, Vec<u32>),
}

impl Ty {
    pub fn elem(&self) -> Option<Elem> {
        match self {
            Ty::Elem(e) => Some(*e),
            _ => None,
        }
    }

    pub fn is_bool(&self) -> bool {
        self.elem() == Some(Elem::Bool)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum StructKind {
    /// TIMER, COUNTER, CONTROL and the other predefined structures, defined in
    /// the bundled prelude.
    Builtin,
    Udt,
    Aoi,
    /// A module-defined I/O structure, reconstructed from decorated data.
    Module,
    /// STRING or a user string type (`LEN` + `DATA`).
    Str,
}

#[derive(Clone, Debug)]
pub(crate) struct Field {
    pub logix: String,
    pub st: String,
    pub ty: Ty,
}

#[derive(Clone, Debug)]
pub(crate) struct StructDef {
    pub logix: String,
    pub st: String,
    pub kind: StructKind,
    pub fields: Vec<Field>,
    /// Declared (e.g. a UDT member refers to a type that failed): the ST
    /// declaration is not emitted.
    pub opaque: bool,
}

impl StructDef {
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields
            .iter()
            .find(|f| f.logix.eq_ignore_ascii_case(name))
    }
}

/// Structures declared by the prelude (TIMER ... STRING): not emitted.
pub(crate) const BUILTIN_COUNT: usize = 7;

#[derive(Default)]
pub(crate) struct TypeEnv {
    pub structs: Vec<StructDef>,
    by_name: HashMap<String, usize>,
}

impl TypeEnv {
    pub fn new() -> Self {
        let mut env = TypeEnv::default();
        let b = |n: &str| Ty::Elem(Elem::parse(n).expect("elementary"));
        let fields = |list: &[(&str, &str)]| -> Vec<Field> {
            list.iter()
                .map(|(n, t)| Field {
                    logix: n.to_string(),
                    st: n.to_string(),
                    ty: b(t),
                })
                .collect()
        };
        // 1756-RM003 "Timer and Counter Instructions" / "Array (File)/Shift
        // Instructions": the members a program may read or write. The prelude
        // (st/logix.st) declares the same fields plus hidden bookkeeping.
        env.add(StructDef {
            logix: "TIMER".into(),
            st: "TIMER".into(),
            kind: StructKind::Builtin,
            fields: fields(&[
                ("PRE", "DINT"),
                ("ACC", "DINT"),
                ("EN", "BOOL"),
                ("TT", "BOOL"),
                ("DN", "BOOL"),
                ("FS", "BOOL"),
                ("LS", "BOOL"),
                ("OV", "BOOL"),
                ("ER", "BOOL"),
            ]),
            opaque: false,
        });
        env.add(StructDef {
            logix: "COUNTER".into(),
            st: "COUNTER".into(),
            kind: StructKind::Builtin,
            fields: fields(&[
                ("PRE", "DINT"),
                ("ACC", "DINT"),
                ("CU", "BOOL"),
                ("CD", "BOOL"),
                ("DN", "BOOL"),
                ("OV", "BOOL"),
                ("UN", "BOOL"),
            ]),
            opaque: false,
        });
        env.add(StructDef {
            logix: "CONTROL".into(),
            st: "CONTROL".into(),
            kind: StructKind::Builtin,
            fields: fields(&[
                ("LEN", "DINT"),
                ("POS", "DINT"),
                ("EN", "BOOL"),
                ("EU", "BOOL"),
                ("DN", "BOOL"),
                ("EM", "BOOL"),
                ("ER", "BOOL"),
                ("UL", "BOOL"),
                ("IN", "BOOL"),
                ("FD", "BOOL"),
            ]),
            opaque: false,
        });
        env.add(StructDef {
            logix: "FBD_TIMER".into(),
            st: "FBD_TIMER".into(),
            kind: StructKind::Builtin,
            fields: fields(&[
                ("EnableIn", "BOOL"),
                ("TimerEnable", "BOOL"),
                ("PRE", "DINT"),
                ("Reset", "BOOL"),
                ("EnableOut", "BOOL"),
                ("ACC", "DINT"),
                ("EN", "BOOL"),
                ("TT", "BOOL"),
                ("DN", "BOOL"),
                ("Status", "DINT"),
                ("InstructFault", "BOOL"),
                ("PresetInv", "BOOL"),
            ]),
            opaque: false,
        });
        env.add(StructDef {
            logix: "FBD_COUNTER".into(),
            st: "FBD_COUNTER".into(),
            kind: StructKind::Builtin,
            fields: fields(&[
                ("EnableIn", "BOOL"),
                ("CUEnable", "BOOL"),
                ("CDEnable", "BOOL"),
                ("PRE", "DINT"),
                ("Reset", "BOOL"),
                ("EnableOut", "BOOL"),
                ("ACC", "DINT"),
                ("CU", "BOOL"),
                ("CD", "BOOL"),
                ("DN", "BOOL"),
                ("OV", "BOOL"),
                ("UN", "BOOL"),
            ]),
            opaque: false,
        });
        env.add(StructDef {
            logix: "FBD_ONESHOT".into(),
            st: "FBD_ONESHOT".into(),
            kind: StructKind::Builtin,
            fields: fields(&[
                ("EnableIn", "BOOL"),
                ("InputBit", "BOOL"),
                ("EnableOut", "BOOL"),
                ("OutputBit", "BOOL"),
            ]),
            opaque: false,
        });
        env.add_string("STRING", "LX_STRING", 82);
        env
    }

    pub fn add(&mut self, d: StructDef) -> usize {
        let i = self.structs.len();
        self.by_name.insert(d.logix.to_ascii_lowercase(), i);
        self.structs.push(d);
        i
    }

    pub fn add_string(&mut self, logix: &str, st: &str, len: u32) -> usize {
        self.add(StructDef {
            logix: logix.into(),
            st: st.into(),
            kind: StructKind::Str,
            fields: vec![
                Field {
                    logix: "LEN".into(),
                    st: "LEN".into(),
                    ty: Ty::Elem(Elem::Dint),
                },
                Field {
                    logix: "DATA".into(),
                    st: "DATA".into(),
                    ty: Ty::Array(Box::new(Ty::Elem(Elem::Sint)), vec![len]),
                },
            ],
            opaque: false,
        })
    }

    pub fn lookup(&self, logix: &str) -> Option<usize> {
        self.by_name.get(&logix.to_ascii_lowercase()).copied()
    }

    /// A Logix type name (elementary or structure).
    pub fn resolve(&self, logix: &str) -> Option<Ty> {
        if let Some(e) = Elem::parse(logix) {
            return Some(Ty::Elem(e));
        }
        self.lookup(logix).map(Ty::Struct)
    }

    pub fn get(&self, i: usize) -> &StructDef {
        &self.structs[i]
    }

    /// The ST spelling of a type.
    pub fn st(&self, t: &Ty) -> String {
        match t {
            Ty::Elem(e) => e.st().to_string(),
            Ty::Struct(i) => self.structs[*i].st.clone(),
            Ty::Array(e, dims) => {
                let ranges: Vec<String> = dims.iter().map(|d| format!("0..{}", d - 1)).collect();
                format!("ARRAY[{}] OF {}", ranges.join(", "), self.st(e))
            }
        }
    }

    /// The Logix spelling of a type, for diagnostics.
    pub fn logix(&self, t: &Ty) -> String {
        match t {
            Ty::Elem(e) => e.st().to_string(),
            Ty::Struct(i) => self.structs[*i].logix.clone(),
            Ty::Array(e, dims) => {
                let d: Vec<String> = dims.iter().map(|d| d.to_string()).collect();
                format!("{}[{}]", self.logix(e), d.join(","))
            }
        }
    }
}
