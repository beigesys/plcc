// SPDX-License-Identifier: MPL-2.0

use std::collections::HashMap;
use std::fmt;

/// IEC 61131-3 type hierarchy.
#[derive(Debug, Clone, PartialEq)]
pub enum IecType {
    // ANY_ELEMENTARY -> ANY_BIT
    Bool,
    Byte,
    Word,
    Dword,
    Lword,
    // ANY_ELEMENTARY -> ANY_MAGNITUDE -> ANY_NUM -> ANY_SIGNED
    Sint,
    Int,
    Dint,
    Lint,
    // ANY_ELEMENTARY -> ANY_MAGNITUDE -> ANY_NUM -> ANY_UNSIGNED
    Usint,
    Uint,
    Udint,
    Ulint,
    // ANY_ELEMENTARY -> ANY_MAGNITUDE -> ANY_NUM -> ANY_REAL
    Real,
    Lreal,
    // ANY_ELEMENTARY -> ANY_MAGNITUDE -> ANY_DURATION
    Time,
    Ltime,
    // ANY_ELEMENTARY -> ANY_DATE
    Date,
    Tod,
    Dt,
    Ldate,
    Ltod,
    Ldt,
    // ANY_ELEMENTARY -> ANY_CHARS
    StringType {
        max_len: Option<usize>,
    },
    WstringType {
        max_len: Option<usize>,
    },
    Char,
    Wchar,
    // ANY_DERIVED
    Array {
        ranges: Vec<(i64, i64)>,
        element_type: Box<IecType>,
    },
    Struct {
        name: String,
        fields: Vec<(String, IecType)>,
    },
    Enum {
        name: String,
        base_type: Box<IecType>,
        values: Vec<(String, i64)>,
    },
    Subrange {
        base_type: Box<IecType>,
        low: i64,
        high: i64,
    },
    Alias {
        name: String,
        base_type: Box<IecType>,
    },
    Pointer(Box<IecType>),
    /// A function block instance type.
    FbInstance(String),
    /// A named type that hasn't been resolved yet.
    Unresolved(String),
    /// Void (for procedures with no return type).
    Void,
}

impl IecType {
    /// Returns the type category for the ANY hierarchy.
    pub fn is_any_bit(&self) -> bool {
        matches!(
            self,
            IecType::Bool | IecType::Byte | IecType::Word | IecType::Dword | IecType::Lword
        )
    }

    pub fn is_any_signed(&self) -> bool {
        matches!(
            self,
            IecType::Sint | IecType::Int | IecType::Dint | IecType::Lint
        )
    }

    pub fn is_any_unsigned(&self) -> bool {
        matches!(
            self,
            IecType::Usint | IecType::Uint | IecType::Udint | IecType::Ulint
        )
    }

    pub fn is_any_int(&self) -> bool {
        self.is_any_signed() || self.is_any_unsigned()
    }

    pub fn is_any_real(&self) -> bool {
        matches!(self, IecType::Real | IecType::Lreal)
    }

    pub fn is_any_num(&self) -> bool {
        self.is_any_int() || self.is_any_real()
    }

    pub fn is_any_duration(&self) -> bool {
        matches!(self, IecType::Time | IecType::Ltime)
    }

    pub fn is_any_magnitude(&self) -> bool {
        self.is_any_num() || self.is_any_duration()
    }

    pub fn is_any_date(&self) -> bool {
        matches!(
            self,
            IecType::Date
                | IecType::Tod
                | IecType::Dt
                | IecType::Ldate
                | IecType::Ltod
                | IecType::Ldt
        )
    }

    pub fn is_any_string(&self) -> bool {
        matches!(
            self,
            IecType::StringType { .. } | IecType::WstringType { .. }
        )
    }

    pub fn is_any_chars(&self) -> bool {
        self.is_any_string() || matches!(self, IecType::Char | IecType::Wchar)
    }

    pub fn is_any_elementary(&self) -> bool {
        self.is_any_magnitude() || self.is_any_bit() || self.is_any_date() || self.is_any_chars()
    }

    /// Get the size in bits for numeric types.
    pub fn bit_size(&self) -> Option<u32> {
        match self {
            IecType::Bool => Some(1),
            IecType::Byte | IecType::Usint | IecType::Sint => Some(8),
            IecType::Word | IecType::Uint | IecType::Int => Some(16),
            IecType::Dword | IecType::Udint | IecType::Dint | IecType::Real => Some(32),
            IecType::Lword | IecType::Ulint | IecType::Lint | IecType::Lreal => Some(64),
            _ => None,
        }
    }

    /// Check if this type can be implicitly converted to another.
    pub fn can_implicit_convert_to(&self, target: &IecType) -> bool {
        if self == target {
            return true;
        }
        // Integer widening
        if self.is_any_int() && target.is_any_int() {
            if let (Some(s), Some(t)) = (self.bit_size(), target.bit_size()) {
                return s <= t
                    && (self.is_any_signed() == target.is_any_signed() || target.is_any_signed());
            }
        }
        // Int to Real
        if self.is_any_int() && target.is_any_real() {
            return true;
        }
        // REAL to LREAL
        if matches!(self, IecType::Real) && matches!(target, IecType::Lreal) {
            return true;
        }
        false
    }
}

impl fmt::Display for IecType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IecType::Bool => write!(f, "BOOL"),
            IecType::Byte => write!(f, "BYTE"),
            IecType::Word => write!(f, "WORD"),
            IecType::Dword => write!(f, "DWORD"),
            IecType::Lword => write!(f, "LWORD"),
            IecType::Sint => write!(f, "SINT"),
            IecType::Int => write!(f, "INT"),
            IecType::Dint => write!(f, "DINT"),
            IecType::Lint => write!(f, "LINT"),
            IecType::Usint => write!(f, "USINT"),
            IecType::Uint => write!(f, "UINT"),
            IecType::Udint => write!(f, "UDINT"),
            IecType::Ulint => write!(f, "ULINT"),
            IecType::Real => write!(f, "REAL"),
            IecType::Lreal => write!(f, "LREAL"),
            IecType::Time => write!(f, "TIME"),
            IecType::Ltime => write!(f, "LTIME"),
            IecType::Date => write!(f, "DATE"),
            IecType::Tod => write!(f, "TOD"),
            IecType::Dt => write!(f, "DT"),
            IecType::Ldate => write!(f, "LDATE"),
            IecType::Ltod => write!(f, "LTOD"),
            IecType::Ldt => write!(f, "LDT"),
            IecType::StringType { max_len: Some(n) } => write!(f, "STRING[{n}]"),
            IecType::StringType { max_len: None } => write!(f, "STRING"),
            IecType::WstringType { max_len: Some(n) } => write!(f, "WSTRING[{n}]"),
            IecType::WstringType { max_len: None } => write!(f, "WSTRING"),
            IecType::Char => write!(f, "CHAR"),
            IecType::Wchar => write!(f, "WCHAR"),
            IecType::Array { element_type, .. } => write!(f, "ARRAY OF {element_type}"),
            IecType::Struct { name, .. } => write!(f, "STRUCT {name}"),
            IecType::Enum { name, .. } => write!(f, "ENUM {name}"),
            IecType::Subrange { base_type, .. } => write!(f, "{base_type}"),
            IecType::Alias { name, .. } => write!(f, "{name}"),
            IecType::Pointer(base) => write!(f, "POINTER TO {base}"),
            IecType::FbInstance(name) => write!(f, "{name}"),
            IecType::Unresolved(name) => write!(f, "{name}"),
            IecType::Void => write!(f, "VOID"),
        }
    }
}

/// Resolve a type name string to an IecType.
pub fn resolve_type_name(name: &str) -> Option<IecType> {
    match name.to_uppercase().as_str() {
        "BOOL" => Some(IecType::Bool),
        "BYTE" => Some(IecType::Byte),
        "WORD" => Some(IecType::Word),
        "DWORD" => Some(IecType::Dword),
        "LWORD" => Some(IecType::Lword),
        "SINT" => Some(IecType::Sint),
        "INT" => Some(IecType::Int),
        "DINT" => Some(IecType::Dint),
        "LINT" => Some(IecType::Lint),
        "USINT" => Some(IecType::Usint),
        "UINT" => Some(IecType::Uint),
        "UDINT" => Some(IecType::Udint),
        "ULINT" => Some(IecType::Ulint),
        "REAL" => Some(IecType::Real),
        "LREAL" => Some(IecType::Lreal),
        "TIME" => Some(IecType::Time),
        "LTIME" => Some(IecType::Ltime),
        "DATE" => Some(IecType::Date),
        "TIME_OF_DAY" | "TOD" => Some(IecType::Tod),
        "DATE_AND_TIME" | "DT" => Some(IecType::Dt),
        "LDATE" => Some(IecType::Ldate),
        "LTOD" => Some(IecType::Ltod),
        "LDT" => Some(IecType::Ldt),
        "STRING" => Some(IecType::StringType { max_len: None }),
        "WSTRING" => Some(IecType::WstringType { max_len: None }),
        "CHAR" => Some(IecType::Char),
        "WCHAR" => Some(IecType::Wchar),
        _ => None,
    }
}

/// Symbol table for type checking.
#[derive(Debug, Clone)]
pub struct TypeRegistry {
    pub user_types: HashMap<String, IecType>,
}

impl TypeRegistry {
    pub fn new() -> Self {
        Self {
            user_types: HashMap::new(),
        }
    }

    /// Register a user type. The key is case-folded, because ST identifiers are
    /// case-insensitive — see [`Self::resolve`].
    pub fn register(&mut self, name: String, ty: IecType) {
        self.user_types.insert(name.to_uppercase(), ty);
    }

    /// Resolve a type name, ignoring case.
    ///
    /// IEC 61131-3 identifiers are case-insensitive (Annex A, 2.1.2), so `t : Ton;`,
    /// `f : myfb;` and `c : complex;` must all resolve. Matching the stored spelling
    /// exactly rejected idiomatic ST outright, and broke real corpora: OSCAT declares
    /// `TYPE COMPLEX` and then writes `complex` in FUNCTION CABS.
    ///
    /// The fold happens here rather than by registering every spelling a program might
    /// use — there is no finite set of those.
    pub fn resolve(&self, name: &str) -> Option<IecType> {
        resolve_type_name(name).or_else(|| self.user_types.get(&name.to_uppercase()).cloned())
    }
}
