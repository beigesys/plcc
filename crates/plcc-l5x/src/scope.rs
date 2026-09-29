// SPDX-License-Identifier: MPL-2.0

//! Tag scopes and the translation of Logix operands and expressions to ST.
//!
//! A reference resolves innermost-first: Add-On Instruction parameters and local
//! tags (an AOI sees nothing else), else program tags, then controller tags and
//! module tags. Alias tags are resolved to their target when declared, so every
//! use of an alias is a use of the target (1756-RM014 "AliasFor").
//!
//! Arithmetic follows 1756-RM003 "Data conversions": integers are promoted and
//! computed at 64 bits (so an overflow of the destination can be detected and
//! reported through S:V), REAL stays REAL, anything mixed with LREAL is LREAL.
//! The final value is stored through a `lx__put_*` prelude function, which
//! rounds REAL→integer half to even, keeps the low bits of an integer that does
//! not fit, and sets S:V / S:Z / S:N.

use crate::error::L5xError;
use crate::operand::{BinOp, LExpr, LKind, Seg, TagPath, UnOp};
use crate::types::{Elem, StructKind, Ty, TypeEnv};
use plcc_st::Span;
use std::collections::HashMap;
use std::ops::Range;

#[derive(Clone, Debug)]
pub(crate) struct Sym {
    /// ST expression for the tag (a variable name, or an alias target path).
    pub st: String,
    pub ty: Option<Ty>,
    /// Logix type name when plcc cannot model it (for diagnostics).
    pub unknown_type: Option<String>,
    #[allow(dead_code)] // where the tag is declared, for future diagnostics
    pub decl: Span,
}

#[derive(Default)]
pub(crate) struct Scope {
    pub map: HashMap<String, Sym>,
}

impl Scope {
    pub fn insert(&mut self, logix: &str, sym: Sym) -> bool {
        self.map.insert(logix.to_ascii_lowercase(), sym).is_none()
    }

    pub fn get(&self, logix: &str) -> Option<&Sym> {
        self.map.get(&logix.to_ascii_lowercase())
    }
}

/// Where an operand's text lives, to turn its byte ranges into L5X spans.
pub(crate) trait SpanOf {
    fn span_of(&self, r: Range<usize>) -> Span;
}

impl SpanOf for crate::xml::Text {
    fn span_of(&self, r: Range<usize>) -> Span {
        self.span(r)
    }
}

/// Value domain of a translated expression.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Dom {
    Bool,
    /// Integers, computed as LINT.
    Int,
    Real,
    LReal,
}

#[derive(Clone, Debug)]
pub(crate) struct Val {
    pub st: String,
    pub dom: Dom,
    /// The Logix type of the operand before promotion, when it is one tag or a
    /// literal (drives the compute type of math instructions).
    pub ty: Ty,
}

pub(crate) struct Ctx<'x> {
    pub env: &'x TypeEnv,
    /// Innermost first.
    pub layers: Vec<&'x Scope>,
    /// Lower-case program name → ST name of its global instance.
    pub programs: &'x HashMap<String, (String, usize)>,
    /// Program scopes, for `Program:Name.Tag`.
    pub program_scopes: &'x [Scope],
}

pub(crate) fn dom_of(t: &Ty) -> Option<Dom> {
    match t.elem()? {
        Elem::Bool => Some(Dom::Bool),
        Elem::Real => Some(Dom::Real),
        Elem::Lreal => Some(Dom::LReal),
        e if e.is_int() => Some(Dom::Int),
        _ => None,
    }
}

/// Convert an ST value between domains.
pub(crate) fn conv(v: &Val, to: Dom) -> String {
    let from = v.dom;
    if from == to {
        return v.st.clone();
    }
    let f = match from {
        Dom::Bool => "BOOL",
        Dom::Int => "LINT",
        Dom::Real => "REAL",
        Dom::LReal => "LREAL",
    };
    let t = match to {
        Dom::Bool => return format!("({} <> 0)", v.st),
        Dom::Int => "LINT",
        Dom::Real => "REAL",
        Dom::LReal => "LREAL",
    };
    format!("{f}_TO_{t}({})", v.st)
}

fn real_lit(v: f64) -> String {
    if v.is_infinite() {
        return if v > 0.0 {
            "lx__inf()".into()
        } else {
            "(-lx__inf())".into()
        };
    }
    let s = format!("{v:?}");
    if s.contains(['.', 'e', 'E']) {
        s
    } else {
        format!("{s}.0")
    }
}

impl Ctx<'_> {
    pub fn lookup(&self, name: &str) -> Option<&Sym> {
        self.layers.iter().find_map(|s| s.get(name))
    }

    /// Resolve a tag path to its ST text and Logix type.
    pub fn path(&self, p: &TagPath, sp: &dyn SpanOf) -> Result<(String, Ty), L5xError> {
        let base_span = sp.span_of(p.base_span.clone());
        let mut segs = p.segs.as_slice();
        let (mut st, mut ty) = if let Some(flag) = status_flag(&p.base) {
            (flag.to_string(), Ty::Elem(Elem::Bool))
        } else if let Some(prog) = p
            .base
            .strip_prefix("Program:")
            .or_else(|| p.base.strip_prefix("program:"))
        {
            // `Program:Other.Tag`: another program's tag, through its instance.
            let Some((inst, idx)) = self.programs.get(&prog.to_ascii_lowercase()) else {
                return Err(L5xError::new(
                    format!("unknown program `{prog}`"),
                    base_span,
                ));
            };
            let Some(Seg::Member(m, msp)) = segs.first() else {
                return Err(L5xError::new(
                    format!("`{}` must be followed by `.tag`", p.base),
                    base_span,
                ));
            };
            let Some(sym) = self.program_scopes[*idx].get(m) else {
                return Err(L5xError::new(
                    format!("program `{prog}` has no tag `{m}`"),
                    sp.span_of(msp.clone()),
                ));
            };
            let ty = sym
                .ty
                .clone()
                .ok_or_else(|| unknown(sym, sp.span_of(msp.clone())))?;
            segs = &segs[1..];
            (format!("{inst}.{}", sym.st), ty)
        } else {
            let Some(sym) = self.lookup(&p.base) else {
                return Err(L5xError::new(
                    format!("unknown tag `{}`", p.base),
                    base_span,
                ));
            };
            let ty = sym.ty.clone().ok_or_else(|| unknown(sym, base_span))?;
            (sym.st.clone(), ty)
        };
        for seg in segs {
            match seg {
                Seg::Member(m, msp) => {
                    let msp = sp.span_of(msp.clone());
                    match &ty {
                        Ty::Struct(i) => {
                            let def = self.env.get(*i);
                            let Some(f) = def.field(m) else {
                                return Err(L5xError::new(
                                    format!("`{}` has no member `{m}`", def.logix),
                                    msp,
                                ));
                            };
                            st = format!("{st}.{}", f.st);
                            ty = f.ty.clone();
                        }
                        other => {
                            return Err(L5xError::new(
                                format!("`{m}`: a {} has no members", self.env.logix(other)),
                                msp,
                            ));
                        }
                    }
                }
                Seg::Bit(b, bsp) => {
                    let bsp = sp.span_of(bsp.clone());
                    match ty.elem() {
                        Some(e) if e.is_int() => {
                            if *b >= e.bits() {
                                return Err(L5xError::new(
                                    format!("bit {b} is out of range for a {}", e.st()),
                                    bsp,
                                ));
                            }
                            st = format!("{st}.{b}");
                            ty = Ty::Elem(Elem::Bool);
                        }
                        _ => {
                            return Err(L5xError::new(
                                format!(
                                    "`.{b}`: bit access needs an integer, not a {}",
                                    self.env.logix(&ty)
                                ),
                                bsp,
                            ));
                        }
                    }
                }
                Seg::IndirectBit(e) => {
                    let Some(el) = ty.elem().filter(|e| e.is_int()) else {
                        return Err(L5xError::new(
                            "indirect bit access `.[n]` needs an integer",
                            sp.span_of(e.span.clone()),
                        ));
                    };
                    let idx = self.value(e, sp)?;
                    let whole = if el == Elem::Lint {
                        st
                    } else {
                        format!("{}_TO_LINT({st})", el.st())
                    };
                    st = format!("lx__getbit({whole}, {})", conv(&idx, Dom::Int));
                    ty = Ty::Elem(Elem::Bool);
                }
                Seg::Index(ix, isp) => {
                    let Ty::Array(elem, dims) = &ty else {
                        return Err(L5xError::new(
                            format!("`[...]`: a {} is not an array", self.env.logix(&ty)),
                            sp.span_of(isp.clone()),
                        ));
                    };
                    if ix.len() != dims.len() {
                        return Err(L5xError::new(
                            format!(
                                "the array has {} dimension(s), not {}",
                                dims.len(),
                                ix.len()
                            ),
                            sp.span_of(isp.clone()),
                        ));
                    }
                    let mut parts = Vec::new();
                    for e in ix {
                        let v = self.value(e, sp)?;
                        if v.dom != Dom::Int {
                            return Err(L5xError::new(
                                "an array subscript must be an integer",
                                sp.span_of(e.span.clone()),
                            ));
                        }
                        parts.push(match &e.kind {
                            LKind::Int(n) => n.to_string(),
                            // A DINT (or narrower) tag indexes directly.
                            LKind::Path(ip)
                                if v.ty.elem().is_some_and(|el| el.is_int() && el.bits() <= 32) =>
                            {
                                self.path(ip, sp)?.0
                            }
                            _ => format!("LINT_TO_DINT({})", v.st),
                        });
                    }
                    st = format!("{st}[{}]", parts.join(", "));
                    ty = (**elem).clone();
                }
            }
        }
        Ok((st, ty))
    }

    /// Translate an expression into its value domain.
    pub fn value(&self, e: &LExpr, sp: &dyn SpanOf) -> Result<Val, L5xError> {
        let at = |r: &Range<usize>| sp.span_of(r.clone());
        Ok(match &e.kind {
            LKind::Int(v) => {
                let ty = if i32::try_from(*v).is_ok() {
                    Elem::Dint
                } else {
                    Elem::Lint
                };
                let st = if *v < 0 {
                    format!("({v})")
                } else {
                    v.to_string()
                };
                Val {
                    st,
                    dom: Dom::Int,
                    ty: Ty::Elem(ty),
                }
            }
            LKind::Real(v) => Val {
                st: if *v < 0.0 {
                    format!("({})", real_lit(*v))
                } else {
                    real_lit(*v)
                },
                dom: Dom::Real,
                ty: Ty::Elem(Elem::Real),
            },
            LKind::Str(_) => {
                return Err(L5xError::new(
                    "a string literal is not supported here",
                    at(&e.span),
                ));
            }
            LKind::Unset => {
                return Err(L5xError::new("operand `?` has no value", at(&e.span)));
            }
            LKind::Path(p) => {
                let (st, ty) = self.path(p, sp)?;
                let Some(dom) = dom_of(&ty) else {
                    return Err(L5xError::new(
                        format!(
                            "`{}` is a {}, not a number or BOOL",
                            p.base,
                            self.env.logix(&ty)
                        ),
                        at(&e.span),
                    ));
                };
                let st = match ty.elem() {
                    Some(el) if el.is_int() && el != Elem::Lint => {
                        format!("{}_TO_LINT({st})", el.st())
                    }
                    _ => st,
                };
                Val { st, dom, ty }
            }
            LKind::Unary(op, inner) => {
                let v = self.value(inner, sp)?;
                match op {
                    UnOp::Neg => {
                        let dom = if v.dom == Dom::Bool { Dom::Int } else { v.dom };
                        Val {
                            st: format!("(-{})", conv(&v, dom)),
                            dom,
                            ty: v.ty,
                        }
                    }
                    UnOp::Not | UnOp::LNot => {
                        if v.dom == Dom::Bool || *op == UnOp::LNot {
                            Val {
                                st: format!("(NOT {})", conv(&v, Dom::Bool)),
                                dom: Dom::Bool,
                                ty: Ty::Elem(Elem::Bool),
                            }
                        } else {
                            Val {
                                st: format!("(NOT {})", conv(&v, Dom::Int)),
                                dom: Dom::Int,
                                ty: v.ty,
                            }
                        }
                    }
                }
            }
            LKind::Binary(op, a, b) => {
                let a = self.value(a, sp)?;
                let b = self.value(b, sp)?;
                self.binary(*op, &a, &b)
            }
            LKind::Call(name, args) => {
                let arg = |i: usize| -> Result<Val, L5xError> {
                    match args.get(i) {
                        Some(x) => self.value(x, sp),
                        None => Err(L5xError::new(
                            format!("{name} needs an argument"),
                            at(&e.span),
                        )),
                    }
                };
                let x = arg(0)?;
                let fdom = |v: &Val| {
                    if v.dom == Dom::LReal {
                        Dom::LReal
                    } else {
                        Dom::Real
                    }
                };
                let float1 = |f: &str, v: &Val| -> Val {
                    let d = fdom(v);
                    let inner = format!("{f}({})", conv(v, Dom::LReal));
                    Val {
                        st: if d == Dom::Real {
                            format!("LREAL_TO_REAL({inner})")
                        } else {
                            inner
                        },
                        dom: d,
                        ty: Ty::Elem(if d == Dom::Real {
                            Elem::Real
                        } else {
                            Elem::Lreal
                        }),
                    }
                };
                match name.as_str() {
                    "ABS" => {
                        let d = if x.dom == Dom::Bool { Dom::Int } else { x.dom };
                        Val {
                            st: format!("ABS({})", conv(&x, d)),
                            dom: d,
                            ty: x.ty,
                        }
                    }
                    "SQR" | "SQRT" => float1("SQRT", &x),
                    "SIN" => float1("SIN", &x),
                    "COS" => float1("COS", &x),
                    "TAN" => float1("TAN", &x),
                    "ASN" | "ASIN" => float1("ASIN", &x),
                    "ACS" | "ACOS" => float1("ACOS", &x),
                    "ATN" | "ATAN" => float1("ATAN", &x),
                    "LN" => float1("LN", &x),
                    "LOG" => float1("LOG", &x),
                    "DEG" => float1("lx__deg", &x),
                    "RAD" => float1("lx__rad", &x),
                    "TRN" | "TRUNC" => {
                        if x.dom == Dom::Int {
                            x
                        } else {
                            float1("lx__trn", &x)
                        }
                    }
                    "FRD" | "BCD_TO" => Val {
                        st: format!("lx__frd({})", conv(&x, Dom::Int)),
                        dom: Dom::Int,
                        ty: Ty::Elem(Elem::Dint),
                    },
                    "TOD" | "TO_BCD" => Val {
                        st: format!("lx__tod({})", conv(&x, Dom::Int)),
                        dom: Dom::Int,
                        ty: Ty::Elem(Elem::Dint),
                    },
                    _ => {
                        return Err(L5xError::new(
                            format!("function `{name}` is not supported in an expression"),
                            at(&e.span),
                        ));
                    }
                }
            }
        })
    }

    pub fn binary(&self, op: BinOp, a: &Val, b: &Val) -> Val {
        let num = |d: Dom| if d == Dom::Bool { Dom::Int } else { d };
        let wide = |x: Dom, y: Dom| -> Dom {
            let (x, y) = (num(x), num(y));
            if x == Dom::LReal || y == Dom::LReal {
                Dom::LReal
            } else if x == Dom::Real || y == Dom::Real {
                Dom::Real
            } else {
                Dom::Int
            }
        };
        let ty_of = |d: Dom| {
            Ty::Elem(match d {
                Dom::Bool => Elem::Bool,
                Dom::Int => Elem::Lint,
                Dom::Real => Elem::Real,
                Dom::LReal => Elem::Lreal,
            })
        };
        let arith = |sym: &str| {
            let d = wide(a.dom, b.dom);
            Val {
                st: format!("({} {sym} {})", conv(a, d), conv(b, d)),
                dom: d,
                ty: ty_of(d),
            }
        };
        match op {
            BinOp::Add => arith("+"),
            BinOp::Sub => arith("-"),
            BinOp::Mul => arith("*"),
            BinOp::Div | BinOp::Mod => {
                let d = wide(a.dom, b.dom);
                let f = match (op, d) {
                    (BinOp::Div, Dom::Int) => "lx__div_i",
                    (BinOp::Div, Dom::Real) => "lx__div_f",
                    (BinOp::Div, _) => "lx__div_r",
                    (_, Dom::Int) => "lx__mod_i",
                    (_, Dom::Real) => "lx__mod_f",
                    _ => "lx__mod_r",
                };
                Val {
                    st: format!("{f}({}, {})", conv(a, d), conv(b, d)),
                    dom: d,
                    ty: ty_of(d),
                }
            }
            BinOp::Pow => Val {
                st: format!("lx__pow({}, {})", conv(a, Dom::LReal), conv(b, Dom::LReal)),
                dom: Dom::LReal,
                ty: ty_of(Dom::LReal),
            },
            BinOp::And | BinOp::Or | BinOp::Xor => {
                let sym = match op {
                    BinOp::And => "AND",
                    BinOp::Or => "OR",
                    _ => "XOR",
                };
                if a.dom == Dom::Bool && b.dom == Dom::Bool {
                    Val {
                        st: format!("({} {sym} {})", a.st, b.st),
                        dom: Dom::Bool,
                        ty: ty_of(Dom::Bool),
                    }
                } else {
                    Val {
                        st: format!("({} {sym} {})", conv(a, Dom::Int), conv(b, Dom::Int)),
                        dom: Dom::Int,
                        ty: ty_of(Dom::Int),
                    }
                }
            }
            BinOp::LAnd | BinOp::LOr | BinOp::LXor => {
                let sym = match op {
                    BinOp::LAnd => "AND",
                    BinOp::LOr => "OR",
                    _ => "XOR",
                };
                Val {
                    st: format!("({} {sym} {})", conv(a, Dom::Bool), conv(b, Dom::Bool)),
                    dom: Dom::Bool,
                    ty: ty_of(Dom::Bool),
                }
            }
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let sym = match op {
                    BinOp::Eq => "=",
                    BinOp::Ne => "<>",
                    BinOp::Lt => "<",
                    BinOp::Le => "<=",
                    BinOp::Gt => ">",
                    _ => ">=",
                };
                let d = if a.dom == Dom::Bool && b.dom == Dom::Bool {
                    Dom::Bool
                } else {
                    wide(a.dom, b.dom)
                };
                Val {
                    st: format!("({} {sym} {})", conv(a, d), conv(b, d)),
                    dom: Dom::Bool,
                    ty: ty_of(Dom::Bool),
                }
            }
        }
    }

    /// A writable destination: a tag path (no indirect bits).
    pub fn dest(&self, e: &LExpr, sp: &dyn SpanOf) -> Result<(String, Ty), L5xError> {
        match &e.kind {
            LKind::Path(p) => {
                if p.segs.iter().any(|s| matches!(s, Seg::IndirectBit(_))) {
                    return self.indirect_bit_dest(p, sp);
                }
                if status_flag(&p.base).is_some() && !p.base.eq_ignore_ascii_case("S:FS") {
                    // S:V etc. may be written with OTE/OTL (1756-RM003 "Math
                    // status flags": "set S:V with an OTE or OTL instruction").
                }
                self.path(p, sp)
            }
            _ => Err(L5xError::new(
                "the destination must be a tag",
                sp.span_of(e.span.clone()),
            )),
        }
    }

    fn indirect_bit_dest(&self, _p: &TagPath, _sp: &dyn SpanOf) -> Result<(String, Ty), L5xError> {
        Err(L5xError::new(
            "writing an indirect bit (`tag.[n]`) is not supported yet",
            _sp.span_of(_p.base_span.clone()),
        ))
    }
}

fn unknown(sym: &Sym, at: Span) -> L5xError {
    L5xError::new(
        format!(
            "tag has data type `{}`, which plcc cannot model",
            sym.unknown_type.as_deref().unwrap_or("?")
        ),
        at,
    )
    .with_help("module-defined and most predefined Logix types other than TIMER, COUNTER, CONTROL and STRING are not supported yet")
}

/// Controller status keywords (1756-RM003 "Math status flags").
pub(crate) fn status_flag(base: &str) -> Option<&'static str> {
    let up = base.to_ascii_uppercase();
    Some(match up.as_str() {
        "S:FS" => "lx__S_FS",
        "S:V" => "lx__S_V",
        "S:Z" => "lx__S_Z",
        "S:N" => "lx__S_N",
        "S:C" => "lx__S_C",
        "S:MINOR" => "lx__S_MINOR",
        _ => return None,
    })
}

/// Whether a type is a Logix string (STRING or a string-family UDT).
pub(crate) fn is_string(env: &TypeEnv, t: &Ty) -> bool {
    matches!(t, Ty::Struct(i) if env.get(*i).kind == StructKind::Str)
}
