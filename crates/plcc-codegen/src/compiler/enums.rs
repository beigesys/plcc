// SPDX-License-Identifier: MPL-2.0

//! Enumerated values: `Idle`, `Mode#Idle` (IEC 61131-3) and `Mode.Idle` (CODESYS).
//!
//! An enumerator is a constant of its enumeration's base type (INT unless the
//! declaration names another). A bare name is accepted when exactly one enumeration
//! (or several with the same value) defines it, as CODESYS does for types without
//! `{attribute 'qualified_only'}`; an ambiguous bare name is an error. A variable of
//! the same name takes precedence over a bare enumerator.

use super::*;

/// Every enumerator of every enumeration in the unit.
#[derive(Default)]
pub(super) struct EnumTable {
    /// Uppercase enumerator name → (uppercase type name, value, the enum type).
    /// Anonymous (inline) enumerations have an empty type name.
    by_value: HashMap<String, Vec<(String, i64, IecType)>>,
    /// Uppercase type name → the enum type.
    types: HashMap<String, IecType>,
}

impl<'ctx> Compiler<'ctx> {
    /// Record the enumerators of `ty` (declared as `type_name`, or anonymous).
    pub(super) fn register_enum(&mut self, type_name: &str, ty: &IecType) {
        let IecType::Enum { values, .. } = ty else {
            return;
        };
        let key = type_name.to_uppercase();
        if !key.is_empty() {
            self.enums.types.insert(key.clone(), ty.clone());
        }
        for (name, v) in values {
            let entry = self.enums.by_value.entry(name.to_uppercase()).or_default();
            if !entry.iter().any(|(t, val, _)| *t == key && val == v) {
                entry.push((key.clone(), *v, ty.clone()));
            }
        }
    }

    fn qualified_enum(&self, type_name: &str, member: &str) -> Option<(i64, IecType)> {
        let ty = self.enums.types.get(&type_name.to_uppercase())?;
        let IecType::Enum { values, .. } = ty else {
            return None;
        };
        values
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(member))
            .map(|(_, v)| (*v, ty.clone()))
    }

    /// The enumerated value `expr` denotes, if it is one, with its enum type.
    pub(super) fn enum_constant_of(
        &self,
        expr: &Expression,
    ) -> Result<Option<(i64, IecType)>, CodegenError> {
        match &expr.kind {
            ExpressionKind::Identifier(id) => {
                let upper = id.name.to_uppercase();
                if self.variables.contains_key(&upper) {
                    return Ok(None);
                }
                let Some(candidates) = self.enums.by_value.get(&upper) else {
                    return Ok(None);
                };
                let (_, v, ty) = &candidates[0];
                if candidates.iter().any(|(_, w, _)| w != v) {
                    let types: Vec<&str> = candidates.iter().map(|(t, _, _)| t.as_str()).collect();
                    return Err(CodegenError::UnsupportedType(format!(
                        "`{}` is an enumerator of several types ({}) with different values; \
                         qualify it, e.g. `{}#{}`",
                        id.name,
                        types.join(", "),
                        types[0],
                        id.name
                    )));
                }
                Ok(Some((*v, ty.clone())))
            }
            ExpressionKind::TypedLiteral { type_name, value } => {
                let ExpressionKind::Identifier(member) = &value.kind else {
                    return Ok(None);
                };
                if !self.enums.types.contains_key(&type_name.name.to_uppercase()) {
                    return Ok(None);
                }
                self.qualified_enum(&type_name.name, &member.name)
                    .map(Some)
                    .ok_or_else(|| {
                        CodegenError::UndefinedVariable(format!(
                            "`{}` is not an enumerator of `{}`",
                            member.name, type_name.name
                        ))
                    })
            }
            // CODESYS `Mode.Idle`, when `Mode` is a type and not a variable.
            ExpressionKind::MemberAccess { object, member } => {
                let ExpressionKind::Identifier(t) = &object.kind else {
                    return Ok(None);
                };
                if self.variables.contains_key(&t.name.to_uppercase())
                    || !self.enums.types.contains_key(&t.name.to_uppercase())
                {
                    return Ok(None);
                }
                self.qualified_enum(&t.name, &member.name)
                    .map(Some)
                    .ok_or_else(|| {
                        CodegenError::UndefinedVariable(format!(
                            "`{}` is not an enumerator of `{}`",
                            member.name, t.name
                        ))
                    })
            }
            _ => Ok(None),
        }
    }

    /// The LLVM constant for an enumerated value of type `ty`.
    pub(super) fn enum_value_const(&self, v: i64, ty: &IecType) -> BasicValueEnum<'ctx> {
        match self.iec_to_llvm_type(ty) {
            BasicTypeEnum::IntType(it) => it.const_int(v as u64, true).into(),
            _ => self.context.i16_type().const_int(v as u64, true).into(),
        }
    }
}
