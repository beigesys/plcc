// SPDX-License-Identifier: MPL-2.0

//! `--io-map`: binding Logix I/O to plcc's process image.
//!
//! On a Logix controller, I/O lives in module tags (`Local:1:I.Data.3`) that
//! the controller updates asynchronously; on plcc hardware it lives in the
//! `%I` / `%Q` / `%M` process image (docs/process-image.md). The map is a TOML
//! table from a Logix tag path to an IEC direct address:
//!
//! ```toml
//! "Local:1:I.Data.0" = "%IX0.0"   # module tag member → Opta input I1
//! "Start"            = "%IX0.1"   # a controller tag (base or alias)
//! "Level"            = "%IW1"     # analog, raw
//! "Local:2:O.Data.0" = "%QX0.0"   # → relay 1
//! ```
//!
//! Entries may also sit under `[io]`. Each entry gets a hidden located
//! variable (`lx__io<n> AT %IX0.0`) and is copied: `%I` into the tag before
//! each program runs, the tag into `%Q` after it, and `%M` both ways. Logix
//! updates I/O asynchronously to the scan; copying at program boundaries is
//! one of the behaviours that allows.

use crate::emit::Out;
use crate::error::L5xError;
use crate::operand::{self, LKind};
use crate::scope::Ctx;
use crate::types::Elem;
use plcc_st::Span;

#[derive(Clone, Debug)]
struct Entry {
    tag: String,
    addr: String,
}

#[derive(Default, Clone, Debug)]
pub struct IoMap {
    entries: Vec<Entry>,
    /// Resolved copies: (ST path, hidden variable, direction).
    copies: Vec<(String, String, char)>,
    /// Hidden located variables: (name, address, type).
    hidden: Vec<(String, String, String)>,
    /// Where map diagnostics and declarations point (the <Controller> tag).
    at: std::ops::Range<usize>,
}

fn addr_ok(addr: &str) -> Option<(char, char)> {
    let a = addr.trim().to_ascii_uppercase();
    let rest = a.strip_prefix('%')?;
    let mut ch = rest.chars();
    let area = ch.next()?;
    if !matches!(area, 'I' | 'Q' | 'M') {
        return None;
    }
    let size = ch.clone().next()?;
    let size = if size.is_ascii_digit() { 'X' } else { size };
    if !matches!(size, 'X' | 'B' | 'W' | 'D' | 'L') {
        return None;
    }
    Some((area, size))
}

fn size_fits(size: char, e: Elem) -> bool {
    match size {
        'X' => e == Elem::Bool,
        'B' => matches!(e, Elem::Sint | Elem::Usint),
        'W' => matches!(e, Elem::Int | Elem::Uint),
        'D' => matches!(e, Elem::Dint | Elem::Udint | Elem::Real),
        'L' => matches!(e, Elem::Lint | Elem::Ulint | Elem::Lreal),
        _ => false,
    }
}

impl IoMap {
    /// Parse the TOML text of an I/O map.
    pub fn parse(text: &str) -> Result<IoMap, String> {
        let table: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut entries = Vec::new();
        let add = |t: &toml::Table, entries: &mut Vec<Entry>| -> Result<(), String> {
            for (k, v) in t {
                match v {
                    toml::Value::String(a) => {
                        if addr_ok(a).is_none() {
                            return Err(format!(
                                "`{k}` = `{a}`: not a process-image address (%IX0.0, %QW3, %MD10, ...)"
                            ));
                        }
                        entries.push(Entry {
                            tag: k.clone(),
                            addr: a.trim().to_string(),
                        });
                    }
                    toml::Value::Table(_) => {}
                    other => {
                        return Err(format!(
                            "`{k}`: expected an address string, found a {}",
                            other.type_str()
                        ));
                    }
                }
            }
            Ok(())
        };
        add(&table, &mut entries)?;
        for (k, v) in &table {
            if let toml::Value::Table(t) = v {
                if k != "io" {
                    return Err(format!(
                        "unknown section [{k}] (entries go at the top level or under [io])"
                    ));
                }
                add(t, &mut entries)?;
            }
        }
        Ok(IoMap {
            entries,
            ..IoMap::default()
        })
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Resolve every entry against the controller scope. Must run after the
    /// controller tags are known and before they are declared.
    pub(crate) fn resolve(&mut self, ctx: &Ctx, at: Span) -> Vec<L5xError> {
        let mut errors = Vec::new();
        self.copies.clear();
        self.hidden.clear();
        self.at = at.start..at.end;
        for (n, e) in self.entries.iter().enumerate() {
            let fail = |m: String| L5xError::new(format!("--io-map `{}`: {m}", e.tag), at);
            let Some((area, size)) = addr_ok(&e.addr) else {
                continue;
            };
            let parsed = match operand::parse_expr(&e.tag, 0..e.tag.len()) {
                Ok(p) => p,
                Err(err) => {
                    errors.push(fail(err.message));
                    continue;
                }
            };
            let LKind::Path(p) = &parsed.kind else {
                errors.push(fail("not a tag path".into()));
                continue;
            };
            let (st, ty) = match ctx.path(p, &AtSpan(at)) {
                Ok(x) => x,
                Err(err) => {
                    errors.push(fail(err.message));
                    continue;
                }
            };
            let Some(el) = ty.elem() else {
                errors.push(fail(format!(
                    "a {} cannot be mapped to one address",
                    ctx.env.logix(&ty)
                )));
                continue;
            };
            if !size_fits(size, el) {
                errors.push(fail(format!("a {} does not fit {}", el.st(), e.addr)));
                continue;
            }
            let hidden = format!("lx__io{n}");
            self.hidden
                .push((hidden.clone(), e.addr.clone(), el.st().to_string()));
            self.copies.push((st, hidden, area));
        }
        errors
    }

    /// Declarations of the hidden located variables (inside VAR_GLOBAL).
    pub(crate) fn hidden_decls(&self) -> Out {
        let mut o = Out::new();
        // Each declaration needs a span of its own (plcc treats equal spans as
        // one `a, b AT ...` list); they all point into the <Controller> tag.
        for (i, (name, addr, ty)) in self.hidden.iter().enumerate() {
            let sp = Span::new(self.at.start + i, self.at.start + i + 1);
            o.m(&format!("    {name} AT {addr} : {ty};\n"), sp);
        }
        o
    }

    pub(crate) fn copy_in(&self, _ctx: &Ctx) -> Out {
        let mut o = Out::new();
        for (st, hidden, area) in &self.copies {
            if matches!(area, 'I' | 'M') {
                o.s(st).s(" := ").s(hidden).s(";\n");
            }
        }
        o
    }

    pub(crate) fn copy_out(&self, _ctx: &Ctx) -> Out {
        let mut o = Out::new();
        for (st, hidden, area) in &self.copies {
            if matches!(area, 'Q' | 'M') {
                o.s(hidden).s(" := ").s(st).s(";\n");
            }
        }
        o
    }
}

/// Operand spans of a map entry all point at one place in the L5X.
struct AtSpan(Span);

impl crate::scope::SpanOf for AtSpan {
    fn span_of(&self, _r: std::ops::Range<usize>) -> Span {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flat_and_sectioned_maps() {
        let m =
            IoMap::parse("\"Local:1:I.Data.0\" = \"%IX0.0\"\n[io]\nMotor = \"%QX0.1\"\n").unwrap();
        assert_eq!(m.entries.len(), 2);
        assert!(IoMap::parse("x = \"Q0.1\"").is_err());
        assert!(IoMap::parse("[inputs]\nx = \"%IX0.0\"").is_err());
    }
}
