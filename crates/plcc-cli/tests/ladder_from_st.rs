// SPDX-License-Identifier: MPL-2.0

//! Structured Text → ladder (`plcc_ladder::from_st`): the ladder model of a
//! program, lowered back to ST, runs exactly like the program (JIT,
//! differential over randomized inputs) — on the ST fixtures and on a
//! generated corpus of programs made of the drawable statements.

mod jit;

use plcc_ladder::model::*;
use plcc_st::ast::*;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Names assigned anywhere in a statement list.
fn assigned(stmts: &[Statement], out: &mut Vec<String>) {
    for s in stmts {
        match &s.kind {
            StatementKind::Assignment { target, .. } => {
                out.push(plcc_st::print_expression(target));
            }
            StatementKind::FunctionCall { args, .. } => {
                for a in args {
                    if a.is_output {
                        out.push(plcc_st::print_expression(&a.value));
                    }
                }
            }
            StatementKind::If {
                then_body,
                elsif_branches,
                else_body,
                ..
            } => {
                assigned(then_body, out);
                for b in elsif_branches {
                    assigned(&b.body, out);
                }
                if let Some(e) = else_body {
                    assigned(e, out);
                }
            }
            StatementKind::Case {
                branches,
                else_body,
                ..
            } => {
                for b in branches {
                    assigned(&b.body, out);
                }
                if let Some(e) = else_body {
                    assigned(e, out);
                }
            }
            StatementKind::For { body, variable, .. } => {
                out.push(plcc_st::print_expression(variable));
                assigned(body, out);
            }
            StatementKind::While { body, .. } | StatementKind::Repeat { body, .. } => {
                assigned(body, out)
            }
            _ => {}
        }
    }
}

/// BOOL and integer variables of the PROGRAMs that nothing assigns: the
/// inputs to randomize.
fn inputs(unit: &CompilationUnit) -> (Vec<String>, Vec<String>) {
    let (mut bools, mut ints) = (Vec::new(), Vec::new());
    for d in &unit.declarations {
        let (blocks, body) = match d {
            Declaration::Program(p) => (&p.var_blocks, &p.body),
            _ => continue,
        };
        let mut written = Vec::new();
        assigned(body, &mut written);
        for b in blocks {
            if matches!(b.kind, VarBlockKind::VarExternal | VarBlockKind::VarTemp) || b.is_constant
            {
                continue;
            }
            for v in &b.declarations {
                if written.iter().any(|w| w.eq_ignore_ascii_case(&v.name.name)) {
                    continue;
                }
                let ty = plcc_st::printer::print_type_spec(&v.type_spec).to_ascii_uppercase();
                match ty.as_str() {
                    "BOOL" => bools.push(v.name.name.clone()),
                    "INT" | "DINT" | "SINT" | "LINT" => ints.push(v.name.name.clone()),
                    _ => {}
                }
            }
        }
    }
    (bools, ints)
}

/// Convert, lower back, run both; `Err` on a difference.
fn check(name: &str, src: &str, scans: usize, seed: u64) -> Result<(usize, usize, usize), String> {
    let (unit, errs) = plcc_st::parse(src);
    if !errs.is_empty() {
        return Err(format!("{name}: does not parse"));
    }
    let (model, _notes) = plcc_ladder::from_st::from_unit(&unit);
    let (back, errs) = plcc_ladder::to_unit(&model, false);
    if !errs.is_empty() {
        return Err(format!("{name}: the model does not lower: {errs:?}"));
    }
    let (mut rungs, mut boxes) = (0, 0);
    for p in &model.pous {
        for r in &p.routines {
            for g in &r.rungs {
                rungs += 1;
                if matches!(g.elements.as_slice(), [Element::St(_)]) {
                    boxes += 1;
                }
            }
        }
    }
    let ctx_a = inkwell::context::Context::create();
    let ctx_b = inkwell::context::Context::create();
    let a = jit::load(&ctx_a, "st", &jit::with_libs(unit.clone(), false))?;
    let st = plcc_st::print_unit(&back);
    let b = jit::load(&ctx_b, "ladder", &jit::with_libs(back, false))
        .map_err(|e| format!("{name}: {e}\n{st}"))?;
    let (bools, ints) = inputs(&unit);
    let opts = jit::Diff {
        ints,
        ..Default::default()
    };
    jit::differential_with(&a, &b, &bools, scans, seed, 20, &opts)
        .map_err(|e| format!("{name}: {e}\n--- ladder model as ST:\n{st}"))?;
    Ok((rungs, rungs - boxes, boxes))
}

#[test]
fn st_fixtures_convert_exactly() {
    let _clock = jit::clock();
    let mut report = Vec::new();
    let mut failures = Vec::new();
    for dir in ["tests/fixtures/programs", "tests/fixtures/codegen"] {
        let mut files: Vec<_> = std::fs::read_dir(root().join(dir))
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "st"))
            .collect();
        files.sort();
        for f in files {
            let src = std::fs::read_to_string(&f).unwrap();
            let name = f.file_name().unwrap().to_string_lossy().into_owned();
            // Only programs the compiler takes as they are.
            let (unit, errs) = plcc_st::parse(&src);
            let ctx = inkwell::context::Context::create();
            if !errs.is_empty() || jit::load(&ctx, "probe", &jit::with_libs(unit, false)).is_err() {
                report.push(format!("{name}: skipped (does not compile on its own)"));
                continue;
            }
            match check(&name, &src, 150, 17) {
                Ok((r, drawn, boxes)) => report.push(format!(
                    "{name}: {r} rungs, {drawn} drawn, {boxes} ST boxes"
                )),
                Err(e) => failures.push(e),
            }
        }
    }
    eprintln!("{}", report.join("\n"));
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

// ── A generated corpus ──

struct Gen(jit::Rng);

impl Gen {
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[self.0.below(xs.len() as u64) as usize]
    }

    fn bexpr(&mut self, depth: u32) -> String {
        let leaf = |g: &mut Gen| -> String {
            match g.0.below(10) {
                0 => format!("n > {}", g.0.below(8) as i64 - 3),
                1 => format!("m <= n + {}", g.0.below(3)),
                2 => "t1.Q".into(),
                3 => "TRUE".into(),
                _ => g.pick(&["a", "b", "c", "d", "e", "x", "y"]).to_string(),
            }
        };
        if depth == 0 || self.0.below(3) == 0 {
            return leaf(self);
        }
        match self.0.below(5) {
            0 => format!("NOT ({})", self.bexpr(depth - 1)),
            1 | 2 => format!("({} AND {})", self.bexpr(depth - 1), self.bexpr(depth - 1)),
            _ => format!("({} OR {})", self.bexpr(depth - 1), self.bexpr(depth - 1)),
        }
    }

    fn statement(&mut self) -> String {
        match self.0.below(12) {
            0..=3 => format!("{} := {};", self.pick(&["x", "y", "z", "w"]), self.bexpr(3)),
            4 => format!(
                "IF {} THEN {} := TRUE; END_IF;",
                self.bexpr(2),
                self.pick(&["x", "y", "w"])
            ),
            5 => format!(
                "IF {} THEN {} := FALSE; {} := TRUE; END_IF;",
                self.bexpr(2),
                self.pick(&["x", "y"]),
                self.pick(&["z", "w"])
            ),
            6 => format!(
                "t1(IN := {}, PT := T#40ms);\nz := t1.Q;\nel := t1.ET;",
                self.bexpr(2)
            ),
            7 => format!(
                "c1(CU := {}, R := {}, PV := 4);\nw := c1.Q;\ncount := c1.CV;",
                self.bexpr(1),
                self.pick(&["d", "e"])
            ),
            8 => format!("n := n + {};", self.0.below(3)),
            9 => format!("m := {} * 2;", self.pick(&["n", "m", "count"])),
            10 => format!("IF {} THEN n := 0; END_IF;", self.bexpr(1)),
            _ => "FOR i := 1 TO 2 DO m := m + i; END_FOR;".into(),
        }
    }

    fn program(&mut self, k: usize) -> String {
        let mut body = String::new();
        for _ in 0..(4 + self.0.below(6)) {
            body.push_str("    ");
            body.push_str(&self.statement());
            body.push('\n');
        }
        format!(
            "PROGRAM Gen{k}\nVAR\n    a, b, c, d, e : BOOL;\n    x, y, z, w : BOOL;\n    n, m, count, i : INT;\n    el : TIME;\n    t1 : TON;\n    c1 : CTU;\nEND_VAR\n{body}END_PROGRAM\n"
        )
    }
}

#[test]
fn generated_programs_convert_exactly() {
    let _clock = jit::clock();
    let mut g = Gen(jit::Rng::new(2026));
    let (mut rungs, mut drawn) = (0, 0);
    let mut failures = Vec::new();
    let n = 60;
    for k in 0..n {
        let src = g.program(k);
        match check(&format!("Gen{k}"), &src, 200, 100 + k as u64) {
            Ok((r, d, _)) => {
                rungs += r;
                drawn += d;
            }
            Err(e) => failures.push(format!("{e}\n--- source:\n{src}")),
        }
    }
    eprintln!("generated corpus: {n} programs, {rungs} rungs, {drawn} drawn as ladder");
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    assert!(drawn * 2 > rungs, "most generated statements are drawable");
}
