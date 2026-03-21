// SPDX-License-Identifier: MPL-2.0

use crate::types::IecType;
use std::collections::HashMap;

/// Variable info stored in the scope.
#[derive(Debug, Clone)]
pub struct VarInfo {
    pub ty: IecType,
    pub is_constant: bool,
    pub is_input: bool,
    pub is_output: bool,
    pub is_in_out: bool,
}

/// A POU (program organization unit) definition.
#[derive(Debug, Clone)]
pub struct PouInfo {
    pub kind: PouKind,
    pub name: String,
    pub return_type: Option<IecType>,
    pub inputs: Vec<(String, IecType)>,
    pub outputs: Vec<(String, IecType)>,
    pub in_outs: Vec<(String, IecType)>,
    pub locals: Vec<(String, IecType)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PouKind {
    Program,
    Function,
    FunctionBlock,
    Method,
}

/// Nested scope for name resolution.
#[derive(Debug)]
pub struct Scope {
    pub variables: HashMap<String, VarInfo>,
    pub parent: Option<Box<Scope>>,
}

impl Scope {
    pub fn new() -> Self {
        Self {
            variables: HashMap::new(),
            parent: None,
        }
    }

    pub fn child(parent: Scope) -> Self {
        Self {
            variables: HashMap::new(),
            parent: Some(Box::new(parent)),
        }
    }

    pub fn define(&mut self, name: String, info: VarInfo) {
        self.variables.insert(name.to_uppercase(), info);
    }

    pub fn lookup(&self, name: &str) -> Option<&VarInfo> {
        let upper = name.to_uppercase();
        self.variables
            .get(&upper)
            .or_else(|| self.parent.as_ref().and_then(|p| p.lookup(&upper)))
    }
}

/// Global symbol table holding all POU definitions.
#[derive(Debug)]
pub struct SymbolTable {
    pub pous: HashMap<String, PouInfo>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self {
            pous: HashMap::new(),
        }
    }

    pub fn register_pou(&mut self, info: PouInfo) {
        self.pous.insert(info.name.to_uppercase(), info);
    }

    pub fn lookup_pou(&self, name: &str) -> Option<&PouInfo> {
        self.pous.get(&name.to_uppercase())
    }
}
