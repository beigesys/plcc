// SPDX-License-Identifier: MPL-2.0

//! Logix strings.
//!
//! A Logix string is a structure `LEN : DINT; DATA : SINT[n]` (STRING has 82
//! characters; a string-family UDT has its own). The ASCII string instructions
//! (1756-RM003 "ASCII String Instructions", "ASCII Conversion Instructions")
//! work on any string type, so plcc generates one small ST helper per
//! combination of string types an instruction is used with
//! (`lx__concat__LX_STRING__LX_STRING__Str40`), sized exactly, and adds them
//! to the project. Characters are 1-based positions, as in the manual.
//!
//! Errors the manual lists as minor faults (a result longer than the
//! destination, a start position outside the string) count a minor fault
//! (`lx__minor()`); a result that does not fit is truncated.

use crate::types::{StructKind, Ty, TypeEnv};
use std::cell::RefCell;
use std::collections::BTreeMap;

#[derive(Default)]
pub(crate) struct Helpers {
    funcs: RefCell<BTreeMap<String, String>>,
}

pub(crate) fn cap(env: &TypeEnv, t: &Ty) -> Option<u32> {
    let Ty::Struct(i) = t else { return None };
    let d = env.get(*i);
    if d.kind != StructKind::Str {
        return None;
    }
    d.field("DATA").and_then(|f| match &f.ty {
        Ty::Array(_, dims) => dims.first().copied(),
        _ => None,
    })
}

fn st(env: &TypeEnv, t: &Ty) -> String {
    env.st(t)
}

impl Helpers {
    pub fn names(&self) -> Vec<String> {
        self.funcs.borrow().keys().cloned().collect()
    }

    /// Helpers other than `names`.
    pub fn source_except(&self, names: &[String]) -> String {
        self.funcs
            .borrow()
            .iter()
            .filter(|(k, _)| !names.contains(k))
            .map(|(_, v)| v.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn is_empty(&self) -> bool {
        self.funcs.borrow().is_empty()
    }

    /// All generated helper functions, in name order.
    pub fn source(&self) -> String {
        self.funcs
            .borrow()
            .values()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn add(&self, name: &str, body: impl FnOnce() -> String) -> String {
        let mut f = self.funcs.borrow_mut();
        if !f.contains_key(name) {
            f.insert(name.to_string(), body());
        }
        name.to_string()
    }

    /// Copy (MOV/MOVE of strings, ST `a := b` between string types): LEN
    /// and the characters that fit; a longer source is truncated with S:V
    /// (1756-RM003 MOVE, "String operands").
    pub fn copy(&self, env: &TypeEnv, from: &Ty, to: &Ty) -> Option<String> {
        let (cs, cd) = (cap(env, from)?, cap(env, to)?);
        let (a, b) = (st(env, from), st(env, to));
        Some(self.add(&format!("lx__scopy__{a}__{b}"), || {
            format!(
                "FUNCTION lx__scopy__{a}__{b} : BOOL\nVAR_IN_OUT s : {a}; d : {b}; END_VAR\nVAR n : DINT; i : DINT; END_VAR\n    \
                 n := s.LEN;\n    IF n < 0 THEN n := 0; END_IF;\n    IF n > {cs} THEN n := {cs}; END_IF;\n    \
                 IF n > {cd} THEN n := {cd}; lx__overflow(); END_IF;\n    \
                 FOR i := 0 TO n - 1 DO d.DATA[i] := s.DATA[i]; END_FOR;\n    d.LEN := n;\n    lx__scopy__{a}__{b} := TRUE;\nEND_FUNCTION\n"
            )
        }))
    }

    /// CONCAT(A, B, Dest): A's characters, then B's (1756-RM003 CONCAT).
    pub fn concat(&self, env: &TypeEnv, ta: &Ty, tb: &Ty, td: &Ty) -> Option<String> {
        let (ca, cb, cd) = (cap(env, ta)?, cap(env, tb)?, cap(env, td)?);
        let (a, b, d) = (st(env, ta), st(env, tb), st(env, td));
        let buf = ca + cb;
        Some(self.add(&format!("lx__concat__{a}__{b}__{d}"), || {
            format!(
                "FUNCTION lx__concat__{a}__{b}__{d} : BOOL\nVAR_IN_OUT sa : {a}; sb : {b}; d : {d}; END_VAR\n\
                 VAR buf : ARRAY[0..{bm}] OF SINT; n : DINT; la : DINT; lb : DINT; i : DINT; END_VAR\n    \
                 la := LIMIT(0, sa.LEN, {ca}); lb := LIMIT(0, sb.LEN, {cb});\n    \
                 FOR i := 0 TO la - 1 DO buf[i] := sa.DATA[i]; END_FOR;\n    \
                 FOR i := 0 TO lb - 1 DO buf[la + i] := sb.DATA[i]; END_FOR;\n    \
                 n := la + lb;\n    IF n > {cd} THEN n := {cd}; lx__minor(); END_IF;\n    \
                 FOR i := 0 TO n - 1 DO d.DATA[i] := buf[i]; END_FOR;\n    d.LEN := n;\n    lx__concat__{a}__{b}__{d} := TRUE;\nEND_FUNCTION\n",
                bm = buf.max(1) - 1,
            )
        }))
    }

    /// MID(Source, Qty, Start, Dest): Qty characters from position Start.
    pub fn mid(&self, env: &TypeEnv, ts: &Ty, td: &Ty) -> Option<String> {
        let (cs, cd) = (cap(env, ts)?, cap(env, td)?);
        let (s, d) = (st(env, ts), st(env, td));
        Some(self.add(&format!("lx__mid__{s}__{d}"), || {
            format!(
                "FUNCTION lx__mid__{s}__{d} : BOOL\nVAR_IN_OUT src : {s}; d : {d}; END_VAR\nVAR_INPUT qty : DINT; start : DINT; END_VAR\n\
                 VAR buf : ARRAY[0..{bm}] OF SINT; n : DINT; l : DINT; i : DINT; END_VAR\n    \
                 l := LIMIT(0, src.LEN, {cs});\n    \
                 IF start < 1 OR start > l OR qty < 0 THEN lx__minor(); RETURN; END_IF;\n    \
                 n := qty;\n    IF start - 1 + n > l THEN n := l - start + 1; END_IF;\n    \
                 FOR i := 0 TO n - 1 DO buf[i] := src.DATA[start - 1 + i]; END_FOR;\n    \
                 IF n > {cd} THEN n := {cd}; lx__minor(); END_IF;\n    \
                 FOR i := 0 TO n - 1 DO d.DATA[i] := buf[i]; END_FOR;\n    d.LEN := n;\n    lx__mid__{s}__{d} := TRUE;\nEND_FUNCTION\n",
                bm = cs.max(1) - 1,
            )
        }))
    }

    /// DELETE(Source, Qty, Start, Dest): Source without Qty characters from
    /// position Start.
    pub fn delete(&self, env: &TypeEnv, ts: &Ty, td: &Ty) -> Option<String> {
        let (cs, cd) = (cap(env, ts)?, cap(env, td)?);
        let (s, d) = (st(env, ts), st(env, td));
        Some(self.add(&format!("lx__delete__{s}__{d}"), || {
            format!(
                "FUNCTION lx__delete__{s}__{d} : BOOL\nVAR_IN_OUT src : {s}; d : {d}; END_VAR\nVAR_INPUT qty : DINT; start : DINT; END_VAR\n\
                 VAR buf : ARRAY[0..{bm}] OF SINT; n : DINT; l : DINT; i : DINT; q : DINT; END_VAR\n    \
                 l := LIMIT(0, src.LEN, {cs});\n    \
                 IF start < 1 OR start > l OR qty < 0 THEN lx__minor(); RETURN; END_IF;\n    \
                 q := qty;\n    IF start - 1 + q > l THEN q := l - start + 1; END_IF;\n    n := 0;\n    \
                 FOR i := 0 TO l - 1 DO\n        IF i < start - 1 OR i >= start - 1 + q THEN buf[n] := src.DATA[i]; n := n + 1; END_IF;\n    END_FOR;\n    \
                 IF n > {cd} THEN n := {cd}; lx__minor(); END_IF;\n    \
                 FOR i := 0 TO n - 1 DO d.DATA[i] := buf[i]; END_FOR;\n    d.LEN := n;\n    lx__delete__{s}__{d} := TRUE;\nEND_FUNCTION\n",
                bm = cs.max(1) - 1,
            )
        }))
    }

    /// INSERT(A, B, Start, Dest): B's characters inserted into A at position
    /// Start.
    pub fn insert(&self, env: &TypeEnv, ta: &Ty, tb: &Ty, td: &Ty) -> Option<String> {
        let (ca, cb, cd) = (cap(env, ta)?, cap(env, tb)?, cap(env, td)?);
        let (a, b, d) = (st(env, ta), st(env, tb), st(env, td));
        Some(self.add(&format!("lx__insert__{a}__{b}__{d}"), || {
            format!(
                "FUNCTION lx__insert__{a}__{b}__{d} : BOOL\nVAR_IN_OUT sa : {a}; sb : {b}; d : {d}; END_VAR\nVAR_INPUT start : DINT; END_VAR\n\
                 VAR buf : ARRAY[0..{bm}] OF SINT; n : DINT; la : DINT; lb : DINT; i : DINT; END_VAR\n    \
                 la := LIMIT(0, sa.LEN, {ca}); lb := LIMIT(0, sb.LEN, {cb});\n    \
                 IF start < 1 OR start > la + 1 THEN lx__minor(); RETURN; END_IF;\n    n := 0;\n    \
                 FOR i := 0 TO start - 2 DO buf[n] := sa.DATA[i]; n := n + 1; END_FOR;\n    \
                 FOR i := 0 TO lb - 1 DO buf[n] := sb.DATA[i]; n := n + 1; END_FOR;\n    \
                 FOR i := start - 1 TO la - 1 DO buf[n] := sa.DATA[i]; n := n + 1; END_FOR;\n    \
                 IF n > {cd} THEN n := {cd}; lx__minor(); END_IF;\n    \
                 FOR i := 0 TO n - 1 DO d.DATA[i] := buf[i]; END_FOR;\n    d.LEN := n;\n    lx__insert__{a}__{b}__{d} := TRUE;\nEND_FUNCTION\n",
                bm = (ca + cb).max(1) - 1,
            )
        }))
    }

    /// FIND(Source, Search, Start, Result): position (1-based) of Search in
    /// Source at or after Start, or 0.
    pub fn find(&self, env: &TypeEnv, ts: &Ty, tf: &Ty) -> Option<String> {
        let (cs, cf) = (cap(env, ts)?, cap(env, tf)?);
        let (s, f) = (st(env, ts), st(env, tf));
        Some(self.add(&format!("lx__find__{s}__{f}"), || {
            format!(
                "FUNCTION lx__find__{s}__{f} : DINT\nVAR_IN_OUT src : {s}; search : {f}; END_VAR\nVAR_INPUT start : DINT; END_VAR\n\
                 VAR l : DINT; m : DINT; i : DINT; j : DINT; ok : BOOL; END_VAR\n    \
                 l := LIMIT(0, src.LEN, {cs}); m := LIMIT(0, search.LEN, {cf});\n    lx__find__{s}__{f} := 0;\n    \
                 IF start < 1 OR start > l THEN lx__minor(); RETURN; END_IF;\n    \
                 FOR i := start - 1 TO l - m DO\n        ok := TRUE;\n        \
                 FOR j := 0 TO m - 1 DO\n            IF src.DATA[i + j] <> search.DATA[j] THEN ok := FALSE; EXIT; END_IF;\n        END_FOR;\n        \
                 IF ok THEN lx__find__{s}__{f} := i + 1; RETURN; END_IF;\n    END_FOR;\nEND_FUNCTION\n"
            )
        }))
    }

    /// Compare two strings: -1, 0, 1 by character codes, a prefix first
    /// ("sorted as in a telephone directory", 1756-RM003 EQ/LT/GT on strings).
    pub fn compare(&self, env: &TypeEnv, ta: &Ty, tb: &Ty) -> Option<String> {
        let (ca, cb) = (cap(env, ta)?, cap(env, tb)?);
        let (a, b) = (st(env, ta), st(env, tb));
        Some(self.add(&format!("lx__scmp__{a}__{b}"), || {
            format!(
                "FUNCTION lx__scmp__{a}__{b} : DINT\nVAR_IN_OUT sa : {a}; sb : {b}; END_VAR\n\
                 VAR la : DINT; lb : DINT; i : DINT; x : DINT; y : DINT; END_VAR\n    \
                 la := LIMIT(0, sa.LEN, {ca}); lb := LIMIT(0, sb.LEN, {cb});\n    \
                 FOR i := 0 TO MIN(la, lb) - 1 DO\n        \
                 x := SINT_TO_DINT(sa.DATA[i]) AND 16#FF; y := SINT_TO_DINT(sb.DATA[i]) AND 16#FF;\n        \
                 IF x < y THEN lx__scmp__{a}__{b} := -1; RETURN; END_IF;\n        \
                 IF x > y THEN lx__scmp__{a}__{b} := 1; RETURN; END_IF;\n    END_FOR;\n    \
                 IF la < lb THEN lx__scmp__{a}__{b} := -1; ELSIF la > lb THEN lx__scmp__{a}__{b} := 1; ELSE lx__scmp__{a}__{b} := 0; END_IF;\nEND_FUNCTION\n"
            )
        }))
    }

    /// UPPER / LOWER (ASCII letters only).
    pub fn case(&self, env: &TypeEnv, ts: &Ty, td: &Ty, upper: bool) -> Option<String> {
        let (cs, cd) = (cap(env, ts)?, cap(env, td)?);
        let (s, d) = (st(env, ts), st(env, td));
        let (lo, hi, delta) = if upper { (97, 122, -32) } else { (65, 90, 32) };
        let fname = if upper { "upper" } else { "lower" };
        Some(self.add(&format!("lx__{fname}__{s}__{d}"), || {
            format!(
                "FUNCTION lx__{fname}__{s}__{d} : BOOL\nVAR_IN_OUT src : {s}; d : {d}; END_VAR\nVAR n : DINT; i : DINT; c : SINT; END_VAR\n    \
                 n := LIMIT(0, src.LEN, {cs});\n    IF n > {cd} THEN n := {cd}; lx__minor(); END_IF;\n    \
                 FOR i := 0 TO n - 1 DO\n        c := src.DATA[i];\n        \
                 IF c >= {lo} AND c <= {hi} THEN c := c + ({delta}); END_IF;\n        d.DATA[i] := c;\n    END_FOR;\n    \
                 d.LEN := n;\n    lx__{fname}__{s}__{d} := TRUE;\nEND_FUNCTION\n"
            )
        }))
    }

    /// DTOS(Source, Dest): the decimal ASCII representation.
    pub fn dtos(&self, env: &TypeEnv, td: &Ty) -> Option<String> {
        let cd = cap(env, td)?;
        let d = st(env, td);
        Some(self.add(&format!("lx__dtos__{d}"), || {
            format!(
                "FUNCTION lx__dtos__{d} : BOOL\nVAR_INPUT v : LINT; END_VAR\nVAR_IN_OUT d : {d}; END_VAR\n\
                 VAR buf : ARRAY[0..20] OF SINT; n : DINT; i : DINT; x : LINT; neg : BOOL; END_VAR\n    \
                 x := v; neg := x < 0; n := 0;\n    \
                 REPEAT\n        buf[n] := LINT_TO_SINT(ABS(x MOD 10) + 48); n := n + 1; x := x / 10;\n    UNTIL x = 0 END_REPEAT;\n    \
                 IF neg THEN buf[n] := 45; n := n + 1; END_IF;\n    \
                 IF n > {cd} THEN lx__minor(); RETURN; END_IF;\n    \
                 FOR i := 0 TO n - 1 DO d.DATA[i] := buf[n - 1 - i]; END_FOR;\n    d.LEN := n;\n    lx__dtos__{d} := TRUE;\nEND_FUNCTION\n"
            )
        }))
    }

    /// STOD(Source, Dest): the leading (optionally signed) decimal integer;
    /// conversion stops at the first character that is not a digit.
    pub fn stod(&self, env: &TypeEnv, ts: &Ty) -> Option<String> {
        let cs = cap(env, ts)?;
        let s = st(env, ts);
        Some(self.add(&format!("lx__stod__{s}"), || {
            format!(
                "FUNCTION lx__stod__{s} : LINT\nVAR_IN_OUT src : {s}; END_VAR\n\
                 VAR l : DINT; i : DINT; c : SINT; x : LINT; neg : BOOL; started : BOOL; END_VAR\n    \
                 l := LIMIT(0, src.LEN, {cs}); x := 0;\n    \
                 FOR i := 0 TO l - 1 DO\n        c := src.DATA[i];\n        \
                 IF NOT started AND (c = 32 OR c = 43) THEN\n            ;\n        \
                 ELSIF NOT started AND c = 45 THEN\n            neg := TRUE; started := TRUE;\n        \
                 ELSIF c >= 48 AND c <= 57 THEN\n            x := x * 10 + SINT_TO_LINT(c - 48); started := TRUE;\n        \
                 ELSE\n            EXIT;\n        END_IF;\n    END_FOR;\n    \
                 IF neg THEN x := -x; END_IF;\n    lx__stod__{s} := x;\nEND_FUNCTION\n"
            )
        }))
    }
}
