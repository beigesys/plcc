// SPDX-License-Identifier: MPL-2.0

//! Tag values → ST initializers.
//!
//! A tag's `<Data Format="Decorated">` (v17+, 1756-RM014 "Data formats") is
//! read when present; otherwise `<Data Format="L5K">`, the bracketed value list
//! (`[0,5000,0]` for a TIMER: control word, PRE, ACC). Zero values are left out,
//! since every variable starts at zero anyway.

use crate::types::{Elem, StructKind, Ty, TypeEnv};
use crate::xml::{self, XNode};

/// One slot of a UDT's L5K value list: a real member, or a hidden SINT host
/// whose bits feed the BOOL members packed into it.
#[derive(Clone, Debug)]
pub(crate) enum L5kSlot {
    Field(usize),
    Host(Vec<(u32, usize)>),
}

/// Integer value of a decorated/L5K atomic literal (any radix, ASCII).
pub(crate) fn parse_int(v: &str) -> Option<i128> {
    let v = v.trim();
    if let Some(body) = v.strip_prefix('\'').and_then(|b| b.strip_suffix('\'')) {
        // ASCII radix: characters, most significant first, `$hh` escapes.
        let bytes = decode_string(body);
        let mut n: i128 = 0;
        for b in bytes {
            n = (n << 8) | b as i128;
        }
        return Some(n);
    }
    let (neg, v) = match v.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, v),
    };
    let v = v.replace('_', "");
    let n = if let Some((radix, digits)) = v.split_once('#') {
        i128::from_str_radix(digits, radix.parse().ok()?).ok()?
    } else {
        v.parse::<i128>().ok()?
    };
    Some(if neg { -n } else { n })
}

pub(crate) fn parse_real(v: &str) -> Option<f64> {
    let t = v.trim();
    if t.contains('$') {
        return Some(if t.starts_with('-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        });
    }
    if t.contains('#') {
        // 1.#QNAN and friends.
        return Some(f64::NAN);
    }
    t.parse().ok()
}

/// `$`-escapes of a Logix string literal body.
pub(crate) fn decode_string(body: &str) -> Vec<u8> {
    let b = body.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'$' && i + 1 < b.len() {
            let n = b[i + 1];
            if i + 2 < b.len() && n.is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit() {
                out.push(u8::from_str_radix(&body[i + 1..i + 3], 16).unwrap_or(0));
                i += 3;
                continue;
            }
            out.push(match n {
                b'L' | b'l' | b'N' | b'n' => b'\n',
                b'R' | b'r' => b'\r',
                b'T' | b't' => b'\t',
                b'P' | b'p' => 0x0c,
                other => other,
            });
            i += 2;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

fn wrap(e: Elem, n: i128) -> i128 {
    let bits = e.bits();
    if bits >= 64 && !e.signed() {
        return n & 0xFFFF_FFFF_FFFF_FFFF;
    }
    let m = 1i128 << bits;
    let mut x = n.rem_euclid(m);
    if e.signed() && x >= m / 2 {
        x -= m;
    }
    x
}

/// An atomic value as an ST literal, or `None` for zero / unrepresentable.
pub(crate) fn atomic(e: Elem, v: &str) -> Option<String> {
    match e {
        Elem::Bool => {
            let n = parse_int(v)?;
            (n != 0).then(|| "TRUE".to_string())
        }
        e if e.is_int() => {
            let n = wrap(e, parse_int(v)?);
            (n != 0).then(|| {
                if n < 0 {
                    format!("({n})")
                } else {
                    n.to_string()
                }
            })
        }
        Elem::Real | Elem::Lreal => {
            let f = parse_real(v)?;
            if f == 0.0 || !f.is_finite() {
                return None;
            }
            let s = format!("{f:?}");
            let s = if s.contains(['.', 'e', 'E']) {
                s
            } else {
                format!("{s}.0")
            };
            Some(if f < 0.0 { format!("({s})") } else { s })
        }
        _ => None,
    }
}

fn string_init(env: &TypeEnv, ty: &Ty, text: &[u8]) -> Option<String> {
    let Ty::Struct(i) = ty else { return None };
    let def = env.get(*i);
    let cap = def
        .fields
        .iter()
        .find(|f| f.logix.eq_ignore_ascii_case("DATA"))
        .and_then(|f| match &f.ty {
            Ty::Array(_, d) => d.first().copied(),
            _ => None,
        })
        .unwrap_or(82) as usize;
    let text = &text[..text.len().min(cap)];
    if text.is_empty() {
        return None;
    }
    let bytes: Vec<String> = text.iter().map(|&b| (b as i8).to_string()).collect();
    Some(format!(
        "(LEN := {}, DATA := [{}])",
        text.len(),
        bytes.join(", ")
    ))
}

/// A string tag's `<Data Format="String">` text.
pub(crate) fn decorated_string(env: &TypeEnv, ty: &Ty, body: &str) -> Option<String> {
    string_init(env, ty, &decode_string(body))
}

// ── Decorated ──

pub(crate) fn decorated(env: &TypeEnv, ty: &Ty, data: XNode) -> Option<String> {
    let node = xml::elements(data).next()?;
    value_node(env, ty, node)
}

fn value_node(env: &TypeEnv, ty: &Ty, n: XNode) -> Option<String> {
    match (xml::name(n), ty) {
        ("DataValue", Ty::Elem(e))
        | ("DataValueMember", Ty::Elem(e))
        | ("Element", Ty::Elem(e)) => atomic(*e, xml::attr(n, "Value")?),
        ("DataValueMember", t) | ("DataValue", t) if crate::scope::is_string(env, t) => {
            let raw = n.text().or(xml::attr(n, "Value")).unwrap_or("");
            let body = raw.trim().trim_start_matches('\'').trim_end_matches('\'');
            string_init(env, t, &decode_string(body))
        }
        ("Array", Ty::Array(elem, dims)) | ("ArrayMember", Ty::Array(elem, dims)) => {
            let total: usize = dims.iter().map(|d| *d as usize).product();
            let mut vals: Vec<Option<String>> = vec![None; total];
            for el in xml::children(n, "Element") {
                let Some(idx) = xml::attr(el, "Index").and_then(|i| flat_index(i, dims)) else {
                    continue;
                };
                if idx >= total {
                    continue;
                }
                let v = match elem.as_ref() {
                    Ty::Elem(e) => xml::attr(el, "Value").and_then(|v| atomic(*e, v)),
                    t => xml::elements(el).next().and_then(|c| value_node(env, t, c)),
                };
                vals[idx] = v;
            }
            array_init(&vals, elem, env)
        }
        ("Structure", Ty::Struct(i))
        | ("StructureMember", Ty::Struct(i))
        | ("Element", Ty::Struct(i)) => {
            let n = if xml::name(n) == "Element" {
                xml::elements(n).next()?
            } else {
                n
            };
            let def = env.get(*i);
            if def.kind == StructKind::Str {
                // LEN + DATA members; DATA holds the text.
                let data = xml::elements(n).find(|m| {
                    xml::attr(*m, "Name").is_some_and(|x| x.eq_ignore_ascii_case("DATA"))
                })?;
                let raw = data.text().or(xml::attr(data, "Value")).unwrap_or("");
                let body = raw.trim().trim_start_matches('\'').trim_end_matches('\'');
                return string_init(env, &Ty::Struct(*i), &decode_string(body));
            }
            let mut parts = Vec::new();
            for m in xml::elements(n) {
                let Some(name) = xml::attr(m, "Name") else {
                    continue;
                };
                let Some(f) = def.field(name) else { continue };
                if let Some(v) = value_node(env, &f.ty, m) {
                    parts.push(format!("{} := {v}", f.st));
                }
            }
            (!parts.is_empty()).then(|| format!("({})", parts.join(", ")))
        }
        _ => None,
    }
}

fn flat_index(idx: &str, dims: &[u32]) -> Option<usize> {
    let inner = idx.trim().trim_start_matches('[').trim_end_matches(']');
    let parts: Vec<usize> = inner
        .split(',')
        .map(|p| p.trim().parse().ok())
        .collect::<Option<_>>()?;
    if parts.len() != dims.len() {
        return None;
    }
    let mut flat = 0usize;
    for (p, d) in parts.iter().zip(dims) {
        flat = flat * *d as usize + p;
    }
    Some(flat)
}

fn zero(ty: &Ty, env: &TypeEnv) -> String {
    match ty {
        Ty::Elem(Elem::Bool) => "FALSE".into(),
        Ty::Elem(e) if e.is_real() => "0.0".into(),
        Ty::Elem(_) => "0".into(),
        Ty::Struct(i) => {
            // An empty aggregate is not valid ST; name one field.
            match env.get(*i).fields.first() {
                Some(f) => format!("({} := {})", f.st, zero(&f.ty, env)),
                None => "()".into(),
            }
        }
        Ty::Array(e, dims) => {
            let total: u32 = dims.iter().product();
            format!("[{}({})]", total, zero(e, env))
        }
    }
}

fn array_init(vals: &[Option<String>], elem: &Ty, env: &TypeEnv) -> Option<String> {
    let last = vals.iter().rposition(|v| v.is_some())?;
    let items: Vec<String> = vals[..=last]
        .iter()
        .map(|v| v.clone().unwrap_or_else(|| zero(elem, env)))
        .collect();
    Some(format!("[{}]", items.join(", ")))
}

// ── L5K ──

#[derive(Debug, Clone)]
enum L5k {
    List(Vec<L5k>),
    Atom(String),
}

fn parse_l5k(s: &str) -> Option<L5k> {
    let b = s.as_bytes();
    let mut i = 0;
    let v = l5k_value(s, b, &mut i)?;
    Some(v)
}

fn l5k_value(s: &str, b: &[u8], i: &mut usize) -> Option<L5k> {
    while *i < b.len() && b[*i].is_ascii_whitespace() {
        *i += 1;
    }
    if *i >= b.len() {
        return None;
    }
    if b[*i] == b'[' {
        *i += 1;
        let mut items = Vec::new();
        loop {
            while *i < b.len() && b[*i].is_ascii_whitespace() {
                *i += 1;
            }
            if *i >= b.len() {
                return None;
            }
            if b[*i] == b']' {
                *i += 1;
                break;
            }
            items.push(l5k_value(s, b, i)?);
            while *i < b.len() && b[*i].is_ascii_whitespace() {
                *i += 1;
            }
            if *i < b.len() && b[*i] == b',' {
                *i += 1;
            }
        }
        return Some(L5k::List(items));
    }
    let start = *i;
    if b[*i] == b'\'' {
        *i += 1;
        while *i < b.len() && b[*i] != b'\'' {
            if b[*i] == b'$' {
                *i += 1;
            }
            *i += 1;
        }
        *i = (*i + 1).min(b.len());
        return Some(L5k::Atom(s[start..*i].to_string()));
    }
    while *i < b.len() && !matches!(b[*i], b',' | b']') && !b[*i].is_ascii_whitespace() {
        *i += 1;
    }
    Some(L5k::Atom(s[start..*i].to_string()))
}

pub(crate) fn l5k(
    env: &TypeEnv,
    ty: &Ty,
    text: &str,
    layouts: &[Option<Vec<L5kSlot>>],
) -> Option<String> {
    let v = parse_l5k(text)?;
    l5k_node(env, ty, &v, layouts)
}

fn flatten(v: &L5k, out: &mut Vec<L5k>) {
    match v {
        L5k::List(items)
            if items.iter().all(|x| matches!(x, L5k::List(_))) && !items.is_empty() =>
        {
            for x in items {
                out.push(x.clone());
            }
        }
        L5k::List(items) => out.extend(items.iter().cloned()),
        a => out.push(a.clone()),
    }
}

fn l5k_node(env: &TypeEnv, ty: &Ty, v: &L5k, layouts: &[Option<Vec<L5kSlot>>]) -> Option<String> {
    match (ty, v) {
        (Ty::Elem(e), L5k::Atom(a)) => atomic(*e, a),
        (Ty::Array(elem, dims), L5k::List(items)) => {
            let total: usize = dims.iter().map(|d| *d as usize).product();
            let mut flat = Vec::new();
            if matches!(elem.as_ref(), Ty::Elem(_)) {
                fn all_atoms(v: &L5k, out: &mut Vec<L5k>) {
                    match v {
                        L5k::List(xs) => xs.iter().for_each(|x| all_atoms(x, out)),
                        a => out.push(a.clone()),
                    }
                }
                items.iter().for_each(|x| all_atoms(x, &mut flat));
            } else {
                flatten(v, &mut flat);
            }
            let vals: Vec<Option<String>> = flat
                .iter()
                .take(total)
                .map(|x| l5k_node(env, elem, x, layouts))
                .collect();
            array_init(&vals, elem, env)
        }
        (Ty::Struct(i), L5k::List(items)) => {
            let def = env.get(*i);
            let bitset = |word: &L5k, bits: &[(&str, u32)]| -> Vec<String> {
                let n = match word {
                    L5k::Atom(a) => parse_int(a).unwrap_or(0),
                    _ => 0,
                };
                bits.iter()
                    .filter(|(_, b)| (n >> b) & 1 == 1)
                    .map(|(f, _)| format!("{f} := TRUE"))
                    .collect()
            };
            let int = |x: Option<&L5k>, f: &str| -> Option<String> {
                match x? {
                    L5k::Atom(a) => atomic(Elem::Dint, a).map(|v| format!("{f} := {v}")),
                    _ => None,
                }
            };
            let mut parts: Vec<String> = Vec::new();
            match (def.kind, def.logix.to_ascii_uppercase().as_str()) {
                (StructKind::Builtin, "TIMER") => {
                    parts.extend(bitset(
                        items.first()?,
                        &[
                            ("EN", 31),
                            ("TT", 30),
                            ("DN", 29),
                            ("FS", 28),
                            ("LS", 27),
                            ("OV", 26),
                            ("ER", 25),
                        ],
                    ));
                    parts.extend(int(items.get(1), "PRE"));
                    parts.extend(int(items.get(2), "ACC"));
                }
                (StructKind::Builtin, "COUNTER") => {
                    parts.extend(bitset(
                        items.first()?,
                        &[("CU", 31), ("CD", 30), ("DN", 29), ("OV", 28), ("UN", 27)],
                    ));
                    parts.extend(int(items.get(1), "PRE"));
                    parts.extend(int(items.get(2), "ACC"));
                }
                (StructKind::Builtin, "CONTROL") => {
                    parts.extend(bitset(
                        items.first()?,
                        &[
                            ("EN", 31),
                            ("EU", 30),
                            ("DN", 29),
                            ("EM", 28),
                            ("ER", 27),
                            ("UL", 26),
                            ("IN", 25),
                            ("FD", 24),
                        ],
                    ));
                    parts.extend(int(items.get(1), "LEN"));
                    parts.extend(int(items.get(2), "POS"));
                }
                (StructKind::Str, _) => {
                    let text = items.iter().find_map(|x| match x {
                        L5k::Atom(a) if a.starts_with('\'') => Some(a.clone()),
                        _ => None,
                    })?;
                    let len = items
                        .first()
                        .and_then(|x| match x {
                            L5k::Atom(a) => parse_int(a),
                            _ => None,
                        })
                        .unwrap_or(0)
                        .max(0) as usize;
                    let body = text.trim_matches('\'');
                    let mut bytes = decode_string(body);
                    bytes.truncate(len);
                    return string_init(env, ty, &bytes);
                }
                (StructKind::Udt, _) => {
                    let layout = layouts.get(*i)?.as_ref()?;
                    for (slot, item) in layout.iter().zip(items) {
                        match slot {
                            L5kSlot::Field(fi) => {
                                let f = &def.fields[*fi];
                                if let Some(v) = l5k_node(env, &f.ty, item, layouts) {
                                    parts.push(format!("{} := {v}", f.st));
                                }
                            }
                            L5kSlot::Host(bits) => {
                                let n = match item {
                                    L5k::Atom(a) => parse_int(a).unwrap_or(0),
                                    _ => 0,
                                };
                                for (b, fi) in bits {
                                    if (n >> b) & 1 == 1 {
                                        parts.push(format!("{} := TRUE", def.fields[*fi].st));
                                    }
                                }
                            }
                        }
                    }
                }
                _ => return None,
            }
            (!parts.is_empty()).then(|| format!("({})", parts.join(", ")))
        }
        (t, L5k::Atom(a)) if crate::scope::is_string(env, t) && a.starts_with('\'') => {
            string_init(env, t, &decode_string(a.trim_matches('\'')))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atoms_in_every_radix() {
        assert_eq!(atomic(Elem::Dint, "16#0000_0005"), Some("5".into()));
        assert_eq!(atomic(Elem::Int, "16#ffff"), Some("(-1)".into()));
        assert_eq!(atomic(Elem::Dint, "0"), None);
        assert_eq!(atomic(Elem::Bool, "1"), Some("TRUE".into()));
        assert_eq!(atomic(Elem::Real, "2.50000000e+000"), Some("2.5".into()));
        assert_eq!(atomic(Elem::Dint, "'$00$00$00A'"), Some("65".into()));
    }

    #[test]
    fn l5k_timer_and_array() {
        let env = TypeEnv::new();
        let timer = env.resolve("TIMER").unwrap();
        assert_eq!(
            l5k(&env, &timer, "[-1610612736,5000,12]", &[]),
            Some("(EN := TRUE, DN := TRUE, PRE := 5000, ACC := 12)".into())
        );
        let arr = Ty::Array(Box::new(Ty::Elem(Elem::Int)), vec![4]);
        assert_eq!(l5k(&env, &arr, "[2,3,0,0]", &[]), Some("[2, 3]".into()));
    }
}
