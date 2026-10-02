// SPDX-License-Identifier: MPL-2.0

//! Structured Text → the ladder model (IEC dialect): the drawable subset as
//! rungs, everything else as ST boxes, one rung per statement.
//!
//! | Statement | Rung |
//! |---|---|
//! | `x := <boolean expression>` | contacts (series for AND, parallel legs for OR, normally closed for NOT, De Morgan for NOT of AND/OR) + coil `x` |
//! | `IF c THEN x := TRUE; y := FALSE; END_IF` (only TRUE/FALSE assignments, no ELSIF/ELSE) | contacts for `c` + set / reset coils (parallel when several) |
//! | `IF c THEN RETURN; END_IF`, `RETURN;` | contacts for `c` + return |
//! | a comparison inside a condition, `a > b` | a compare box (GT, …) run by the rung, its result in a hidden BOOL `ld_cmp<n>`, and a contact on it |
//! | `fb(IN := x, PT := T#5s, Q => y)` of a standard FB (TON, TOF, TP, RTO, CTU, CTD, CTUD, R_TRIG, F_TRIG, SR, RS) | contacts for the power input + the FB box, other inputs as pin values; `v := fb.OUT` right after the call become output pins, and `x := fb.Q` a coil driven by the box |
//! | `fb(a := 1, b => c)` of another FB, named arguments | the box, taking no power |
//! | `x := a + b` (`-`, `*`, `/`, `MOD`) | an ADD / SUB / MUL / DIV / MOD box |
//! | any other assignment `x := e` | a MOVE box with `e` on its input |
//! | anything else (loops, CASE, other IFs, calls with positional arguments, EXIT, …) | an ST box holding the statement |
//!
//! Types come from the declarations: an assignment is drawn with contacts and
//! a coil only when the target and every operand are BOOL. Every conversion is
//! exact — the model lowered back to ST computes what the input did (the
//! tests check this by running both).

use crate::catalog;
use crate::model::*;
use plcc_st::ast::*;
use std::collections::HashMap;

/// Every PROGRAM, FUNCTION_BLOCK and FUNCTION of `unit` as a POU with its
/// body drawn as rungs. Other declarations are reported and left out.
pub fn from_unit(unit: &CompilationUnit) -> (Project, Vec<String>) {
    let mut notes = Vec::new();
    let mut ids = Ids::new();
    let mut project = Project {
        dialect: Dialect::Iec,
        ..Default::default()
    };
    // Globals are visible everywhere.
    let mut globals: HashMap<String, String> = HashMap::new();
    for d in &unit.declarations {
        if let Declaration::GlobalVarDecl(b) = d {
            for v in variables(std::slice::from_ref(b)) {
                globals.insert(v.name.to_ascii_uppercase(), v.data_type.clone());
                project.globals.push(v);
            }
        }
    }
    // Function blocks declared in the unit: their input/output names.
    let mut fbs: HashMap<String, Vec<(String, PinDir, String)>> = HashMap::new();
    for d in &unit.declarations {
        if let Declaration::FunctionBlock(f) = d {
            let pins = variables(&f.var_blocks)
                .into_iter()
                .filter_map(|v| {
                    let dir = match v.section {
                        VarSection::Input => PinDir::Input,
                        VarSection::Output => PinDir::Output,
                        VarSection::InOut => PinDir::InOut,
                        _ => return None,
                    };
                    Some((v.name, dir, v.data_type))
                })
                .collect();
            fbs.insert(f.name.name.to_ascii_uppercase(), pins);
        }
    }
    for d in &unit.declarations {
        let (name, kind, ret, blocks, body) = match d {
            Declaration::Program(p) => (&p.name, PouKind::Program, None, &p.var_blocks, &p.body),
            Declaration::FunctionBlock(f) => (
                &f.name,
                PouKind::FunctionBlock,
                None,
                &f.var_blocks,
                &f.body,
            ),
            Declaration::Function(f) => (
                &f.name,
                PouKind::Function,
                f.return_type.as_ref(),
                &f.var_blocks,
                &f.body,
            ),
            Declaration::GlobalVarDecl(_) => continue,
            other => {
                notes.push(format!(
                    "{} is not ladder: carried along as Structured Text",
                    decl_kind(other)
                ));
                project
                    .declarations
                    .push(plcc_st::printer::print_declaration(other));
                continue;
            }
        };
        // Methods, properties and actions: carried along as ST.
        let (methods, properties, actions) = match d {
            Declaration::Program(p) => (&p.methods[..], &p.properties[..], &p.actions[..]),
            Declaration::FunctionBlock(f) => (&f.methods[..], &f.properties[..], &f.actions[..]),
            _ => (&[][..], &[][..], &[][..]),
        };
        let members = if methods.is_empty() && properties.is_empty() && actions.is_empty() {
            String::new()
        } else {
            notes.push(format!(
                "{}: methods, properties and actions are not ladder: carried along as Structured Text",
                name.name
            ));
            members_text(methods, properties, actions)
        };
        let vars = variables(blocks);
        let mut types = globals.clone();
        for v in &vars {
            types.insert(v.name.to_ascii_uppercase(), v.data_type.clone());
        }
        if let Some(r) = ret {
            types.insert(
                name.name.to_ascii_uppercase(),
                plcc_st::printer::print_type_spec(r),
            );
        }
        let mut c = Conv {
            ids: &mut ids,
            types,
            fbs: &fbs,
            hidden: Vec::new(),
            ncmp: 0,
            in_function: kind == PouKind::Function,
        };
        let rungs = c.body(body);
        let mut variables = vars;
        variables.extend(c.hidden);
        let pou_id = ids.fresh();
        let routine_id = ids.fresh();
        project.pous.push(Pou {
            id: pou_id,
            name: name.name.clone(),
            kind,
            return_type: ret.map(plcc_st::printer::print_type_spec),
            variables,
            routines: vec![Routine {
                id: routine_id,
                name: name.name.clone(),
                rungs,
            }],
            members,
        });
    }
    (project, notes)
}

/// Methods, properties and actions as ST text (the inside of a POU).
fn members_text(
    methods: &[MethodDecl],
    properties: &[PropertyDecl],
    actions: &[ActionDecl],
) -> String {
    let holder = Declaration::FunctionBlock(FunctionBlockDecl {
        name: Ident::new("_", plcc_st::Span::empty()),
        extends: None,
        implements: Vec::new(),
        var_blocks: Vec::new(),
        methods: methods.to_vec(),
        properties: properties.to_vec(),
        actions: actions.to_vec(),
        body: Vec::new(),
        span: plcc_st::Span::empty(),
    });
    let text = plcc_st::printer::print_declaration(&holder);
    let lines: Vec<&str> = text.lines().collect();
    lines[1..lines.len().saturating_sub(1)].join("\n") + "\n"
}

fn decl_kind(d: &Declaration) -> &'static str {
    match d {
        Declaration::TypeDecl(_) => "a TYPE",
        Declaration::GlobalVarDecl(_) => "a VAR_GLOBAL block",
        Declaration::Configuration(_) => "a CONFIGURATION",
        Declaration::Class(_) => "a CLASS",
        Declaration::Interface(_) => "an INTERFACE",
        _ => "a declaration",
    }
}

/// The variables of VAR blocks.
pub fn variables(blocks: &[VarBlock]) -> Vec<Variable> {
    let mut out = Vec::new();
    for b in blocks {
        let section = match b.kind {
            VarBlockKind::VarInput => VarSection::Input,
            VarBlockKind::VarOutput => VarSection::Output,
            VarBlockKind::VarInOut => VarSection::InOut,
            VarBlockKind::VarExternal => VarSection::External,
            VarBlockKind::VarTemp => VarSection::Temp,
            VarBlockKind::VarGlobal => VarSection::Global,
            _ => VarSection::Local,
        };
        for d in &b.declarations {
            out.push(Variable {
                name: d.name.name.clone(),
                data_type: plcc_st::printer::print_type_spec(&d.type_spec),
                section,
                initial: d.initializer.as_ref().map(plcc_st::print_expression),
                address: d.at_address.as_ref().map(|a| a.repr.clone()),
                comment: None,
                constant: b.is_constant,
                retain: b.is_retain,
            });
        }
    }
    out
}

struct Conv<'a> {
    ids: &'a mut Ids,
    /// Upper-case name → declared type text.
    types: HashMap<String, String>,
    /// FBs declared in the unit: their inputs and outputs.
    fbs: &'a HashMap<String, Vec<(String, PinDir, String)>>,
    /// Hidden variables the rungs need (compare results).
    hidden: Vec<Variable>,
    ncmp: usize,
    in_function: bool,
}

fn text(e: &Expression) -> String {
    plcc_st::print_expression(e)
}

/// `a`, `a.b`, `a[1].c`, `a.3`: something a contact or coil can name.
fn is_path(e: &Expression) -> bool {
    match &e.kind {
        ExpressionKind::Identifier(_) => true,
        ExpressionKind::MemberAccess { object, .. } => is_path(object),
        ExpressionKind::ArrayIndex { array, indices } => {
            is_path(array) && indices.iter().all(|i| !has_call(i))
        }
        ExpressionKind::Parenthesized(x) => is_path(x),
        _ => false,
    }
}

fn has_call(e: &Expression) -> bool {
    match &e.kind {
        ExpressionKind::FunctionCall { .. } => true,
        ExpressionKind::BinaryOp { left, right, .. } => has_call(left) || has_call(right),
        ExpressionKind::UnaryOp { operand, .. } | ExpressionKind::Parenthesized(operand) => {
            has_call(operand)
        }
        ExpressionKind::MemberAccess { object, .. } => has_call(object),
        ExpressionKind::ArrayIndex { array, indices } => {
            has_call(array) || indices.iter().any(has_call)
        }
        ExpressionKind::Dereference(x) => has_call(x),
        _ => false,
    }
}

/// A compare operand that evaluates the same when the rung does not reach the
/// box: no calls (side effects), no division (a fault the ST would have
/// raised), no computed index (an out-of-bounds fault).
fn cmp_operand(e: &Expression) -> bool {
    match &e.kind {
        ExpressionKind::FunctionCall { .. } | ExpressionKind::Dereference(_) => false,
        ExpressionKind::BinaryOp { op, left, right } => {
            !matches!(op, BinaryOp::Div | BinaryOp::Mod) && cmp_operand(left) && cmp_operand(right)
        }
        ExpressionKind::UnaryOp { operand, .. } | ExpressionKind::Parenthesized(operand) => {
            cmp_operand(operand)
        }
        ExpressionKind::MemberAccess { object, .. } => cmp_operand(object),
        ExpressionKind::ArrayIndex { array, indices } => {
            cmp_operand(array)
                && indices
                    .iter()
                    .all(|i| matches!(i.kind, ExpressionKind::IntegerLiteral(_)))
        }
        _ => true,
    }
}

fn unparen(e: &Expression) -> &Expression {
    match &e.kind {
        ExpressionKind::Parenthesized(x) => unparen(x),
        _ => e,
    }
}

const STD_BOOL_OUTPUTS: &[&str] = &["Q", "Q1", "QU", "QD", "ENO"];

impl Conv<'_> {
    fn id(&mut self) -> Id {
        self.ids.fresh()
    }

    /// The declared type of a path's root, upper case, for plain names.
    fn type_of_path(&self, e: &Expression) -> Option<String> {
        match &unparen(e).kind {
            ExpressionKind::Identifier(i) => self
                .types
                .get(&i.name.to_ascii_uppercase())
                .map(|t| t.to_ascii_uppercase()),
            ExpressionKind::MemberAccess { object, member } => {
                // A bit of an integer, `x.3`.
                if member.name.chars().all(|c| c.is_ascii_digit()) {
                    return Some("BOOL".into());
                }
                let owner = self.type_of_path(object)?;
                if let Some(spec) = catalog::iec_spec(&owner)
                    && spec.instance
                {
                    let m = member.name.to_ascii_uppercase();
                    if STD_BOOL_OUTPUTS.contains(&m.as_str()) {
                        return Some("BOOL".into());
                    }
                    return spec
                        .pins
                        .iter()
                        .find(|p| p.name.eq_ignore_ascii_case(&m))
                        .map(|p| match p.name {
                            "IN" | "CU" | "CD" | "R" | "LD" | "CLK" | "S1" | "S" | "R1" => {
                                "BOOL".into()
                            }
                            "PT" | "ET" => "TIME".into(),
                            _ => "INT".into(),
                        });
                }
                if let Some(pins) = self.fbs.get(&owner) {
                    return pins
                        .iter()
                        .find(|(n, _, _)| n.eq_ignore_ascii_case(&member.name))
                        .map(|(_, _, t)| t.to_ascii_uppercase());
                }
                None
            }
            _ => None,
        }
    }

    fn is_bool_path(&self, e: &Expression) -> bool {
        is_path(e) && self.type_of_path(e).as_deref() == Some("BOOL")
    }

    /// Whether `e` can be drawn as contacts (and compare boxes).
    fn drawable_bool(&self, e: &Expression) -> bool {
        match &unparen(e).kind {
            ExpressionKind::BoolLiteral(_) => true,
            ExpressionKind::BinaryOp {
                op: BinaryOp::And | BinaryOp::Or,
                left,
                right,
            } => self.drawable_bool(left) && self.drawable_bool(right),
            ExpressionKind::UnaryOp {
                op: UnaryOp::Not,
                operand,
            } => self.drawable_bool(operand),
            ExpressionKind::BinaryOp {
                op:
                    BinaryOp::Equal
                    | BinaryOp::NotEqual
                    | BinaryOp::Less
                    | BinaryOp::LessEqual
                    | BinaryOp::Greater
                    | BinaryOp::GreaterEqual,
                left,
                right,
            } => cmp_operand(left) && cmp_operand(right),
            _ => self.is_bool_path(e),
        }
    }

    /// Contacts (and compare boxes) for a drawable boolean expression.
    /// `negate`: draw NOT e.
    fn contacts(&mut self, e: &Expression, negate: bool) -> Vec<Element> {
        let e = unparen(e);
        match &e.kind {
            ExpressionKind::BoolLiteral(b) => {
                let id = self.id();
                vec![Element::Contact(Contact {
                    id,
                    operand: if *b != negate { "TRUE" } else { "FALSE" }.into(),
                    ..Default::default()
                })]
            }
            ExpressionKind::UnaryOp {
                op: UnaryOp::Not,
                operand,
            } => self.contacts(operand, !negate),
            ExpressionKind::BinaryOp { op, left, right }
                if matches!(op, BinaryOp::And | BinaryOp::Or) =>
            {
                // AND is a series, OR parallel legs; NOT swaps them (De Morgan).
                let series = (*op == BinaryOp::And) != negate;
                if series {
                    let mut v = self.contacts(left, negate);
                    v.extend(self.contacts(right, negate));
                    v
                } else {
                    let mut legs = Vec::new();
                    self.or_legs(e, negate, &mut legs);
                    let id = self.id();
                    vec![Element::Branch(Branch {
                        id,
                        legs,
                        src: None,
                    })]
                }
            }
            ExpressionKind::BinaryOp { op, left, right } => {
                // A comparison: a box run by the rung writes the result into a
                // hidden BOOL, and a contact reads it.
                let name = match op {
                    BinaryOp::Equal => "EQ",
                    BinaryOp::NotEqual => "NE",
                    BinaryOp::Less => "LT",
                    BinaryOp::LessEqual => "LE",
                    BinaryOp::Greater => "GT",
                    _ => "GE",
                };
                self.ncmp += 1;
                while self.types.contains_key(&format!("LD_CMP{}", self.ncmp)) {
                    self.ncmp += 1;
                }
                let var = format!("ld_cmp{}", self.ncmp);
                self.types.insert(var.to_ascii_uppercase(), "BOOL".into());
                self.hidden.push(Variable {
                    name: var.clone(),
                    data_type: "BOOL".into(),
                    comment: Some(format!("result of {}", text(e))),
                    ..Default::default()
                });
                let (bid, cid) = (self.id(), self.id());
                vec![
                    Element::Block(Block {
                        id: bid,
                        name: name.into(),
                        pins: vec![
                            Pin {
                                name: "EN".into(),
                                ..Default::default()
                            },
                            Pin {
                                name: "IN1".into(),
                                value: Some(text(left)),
                                ..Default::default()
                            },
                            Pin {
                                name: "IN2".into(),
                                value: Some(text(right)),
                                ..Default::default()
                            },
                            Pin {
                                name: "ENO".into(),
                                dir: PinDir::Output,
                                ..Default::default()
                            },
                            Pin {
                                name: "OUT".into(),
                                dir: PinDir::Output,
                                value: Some(var.clone()),
                                ..Default::default()
                            },
                        ],
                        power_in: Some("EN".into()),
                        power_out: Some("ENO".into()),
                        ..Default::default()
                    }),
                    Element::Contact(Contact {
                        id: cid,
                        operand: var,
                        kind: if negate {
                            ContactKind::Nc
                        } else {
                            ContactKind::No
                        },
                        ..Default::default()
                    }),
                ]
            }
            _ => {
                let id = self.id();
                vec![Element::Contact(Contact {
                    id,
                    operand: text(e),
                    kind: if negate {
                        ContactKind::Nc
                    } else {
                        ContactKind::No
                    },
                    ..Default::default()
                })]
            }
        }
    }

    /// The legs of an OR (flattened), or of a negated AND.
    fn or_legs(&mut self, e: &Expression, negate: bool, legs: &mut Vec<Vec<Element>>) {
        let e = unparen(e);
        let joins = |op: &BinaryOp| {
            (*op == BinaryOp::Or) != negate && matches!(op, BinaryOp::And | BinaryOp::Or)
        };
        match &e.kind {
            ExpressionKind::BinaryOp { op, left, right } if joins(op) => {
                self.or_legs(left, negate, legs);
                self.or_legs(right, negate, legs);
            }
            _ => legs.push(self.contacts(e, negate)),
        }
    }

    fn body(&mut self, body: &[Statement]) -> Vec<Rung> {
        let mut rungs = Vec::new();
        let mut comment: Option<String> = None;
        let mut i = 0;
        while i < body.len() {
            let s = &body[i];
            i += 1;
            if let StatementKind::Comment(c) = &s.kind {
                comment = Some(c.clone());
                continue;
            }
            if matches!(s.kind, StatementKind::Empty) {
                continue;
            }
            let elements = match self.statement(s, &body[i..]) {
                Some((elements, used)) => {
                    i += used;
                    elements
                }
                None => {
                    let id = self.id();
                    vec![Element::St(StBox {
                        id,
                        code: plcc_st::print_statements(std::slice::from_ref(s), 0)
                            .trim_end()
                            .to_string(),
                        notes: Vec::new(),
                    })]
                }
            };
            let id = self.id();
            rungs.push(Rung {
                id,
                comment: comment.take(),
                label: None,
                elements,
                label_src: None,
                part_of: None,
            });
        }
        rungs
    }

    /// A statement as rung elements, and how many of the statements after it
    /// were absorbed; `None`: not drawable.
    fn statement(&mut self, s: &Statement, rest: &[Statement]) -> Option<(Vec<Element>, usize)> {
        match &s.kind {
            StatementKind::Assignment { target, value } => {
                if !is_path(target) {
                    return None;
                }
                let tt = self.type_of_path(target);
                if tt.as_deref() == Some("BOOL") && self.drawable_bool(value) {
                    let mut v = self.contacts(value, false);
                    let id = self.id();
                    v.push(Element::Coil(Coil {
                        id,
                        operand: text(target),
                        ..Default::default()
                    }));
                    return Some((v, 0));
                }
                if tt.is_none() || has_call(value) && !self.pure_call(value) {
                    return None;
                }
                Some((vec![self.math_box(target, value)], 0))
            }
            StatementKind::If {
                condition,
                then_body,
                elsif_branches,
                else_body,
            } if elsif_branches.is_empty() && else_body.is_none() => {
                if !self.drawable_bool(condition) || then_body.is_empty() {
                    return None;
                }
                // Only TRUE/FALSE into BOOLs, or a RETURN.
                let mut outs = Vec::new();
                for st in then_body {
                    match &st.kind {
                        StatementKind::Assignment { target, value }
                            if self.is_bool_path(target)
                                && matches!(
                                    unparen(value).kind,
                                    ExpressionKind::BoolLiteral(_)
                                ) =>
                        {
                            let set =
                                matches!(unparen(value).kind, ExpressionKind::BoolLiteral(true));
                            let id = self.id();
                            outs.push(Element::Coil(Coil {
                                id,
                                operand: text(target),
                                kind: if set { CoilKind::Set } else { CoilKind::Reset },
                                ..Default::default()
                            }));
                        }
                        StatementKind::Return { value: None } if then_body.len() == 1 => {
                            let id = self.id();
                            outs.push(Element::Return(Return { id, src: None }));
                        }
                        _ => return None,
                    }
                }
                let mut v = self.contacts(condition, false);
                if outs.len() == 1 {
                    v.extend(outs);
                } else {
                    let id = self.id();
                    v.push(Element::Branch(Branch {
                        id,
                        legs: outs.into_iter().map(|o| vec![o]).collect(),
                        src: None,
                    }));
                }
                Some((v, 0))
            }
            StatementKind::Return { value: None } => {
                let id = self.id();
                Some((vec![Element::Return(Return { id, src: None })], 0))
            }
            StatementKind::FunctionCall { callee, args } => self.fb_call(callee, args, rest),
            _ => None,
        }
    }

    /// A call of a function we know has no side effects (an IEC standard
    /// function spelled as an operator is already an operator).
    fn pure_call(&self, _e: &Expression) -> bool {
        false
    }

    /// `x := e` as a box: ADD/SUB/MUL/DIV/MOD for one arithmetic operator,
    /// else MOVE.
    fn math_box(&mut self, target: &Expression, value: &Expression) -> Element {
        let id = self.id();
        let (name, ins) = match &unparen(value).kind {
            ExpressionKind::BinaryOp { op, left, right }
                if matches!(
                    op,
                    BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod
                ) =>
            {
                let n = match op {
                    BinaryOp::Add => "ADD",
                    BinaryOp::Sub => "SUB",
                    BinaryOp::Mul => "MUL",
                    BinaryOp::Div => "DIV",
                    _ => "MOD",
                };
                (n, vec![("IN1", text(left)), ("IN2", text(right))])
            }
            _ => ("MOVE", vec![("IN", text(value))]),
        };
        let mut pins = vec![Pin {
            name: "EN".into(),
            ..Default::default()
        }];
        for (n, v) in ins {
            pins.push(Pin {
                name: n.into(),
                value: Some(v),
                ..Default::default()
            });
        }
        pins.push(Pin {
            name: "ENO".into(),
            dir: PinDir::Output,
            ..Default::default()
        });
        pins.push(Pin {
            name: "OUT".into(),
            dir: PinDir::Output,
            value: Some(text(target)),
            ..Default::default()
        });
        Element::Block(Block {
            id,
            name: name.into(),
            pins,
            power_in: Some("EN".into()),
            power_out: Some("ENO".into()),
            ..Default::default()
        })
    }

    /// `inst(IN := x, PT := t, Q => y)` of an FB instance with named
    /// arguments; the `v := inst.OUT` statements right after it are absorbed.
    fn fb_call(
        &mut self,
        callee: &Expression,
        args: &[CallArg],
        rest: &[Statement],
    ) -> Option<(Vec<Element>, usize)> {
        let ExpressionKind::Identifier(inst) = &unparen(callee).kind else {
            return None;
        };
        if args.iter().any(|a| a.name.is_none() || a.negated) {
            return None;
        }
        let ty = self.types.get(&inst.name.to_ascii_uppercase())?.clone();
        let ty_up = ty.to_ascii_uppercase();
        let spec = catalog::iec_spec(&ty_up).filter(|s| s.instance);
        let user = self.fbs.get(&ty_up);
        if spec.is_none() && user.is_none() {
            return None;
        }
        if self.in_function {
            return None;
        }
        let pin_names: Vec<(String, PinDir)> = match (&spec, user) {
            (Some(s), _) => s.pins.iter().map(|p| (p.name.to_string(), p.dir)).collect(),
            (None, Some(u)) => u.iter().map(|(n, d, _)| (n.clone(), *d)).collect(),
            _ => return None,
        };
        // Every argument must name a pin.
        for a in args {
            let n = a.name.as_ref()?;
            if !pin_names
                .iter()
                .any(|(p, _)| p.eq_ignore_ascii_case(&n.name))
            {
                return None;
            }
        }
        let arg = |name: &str, output: bool| {
            args.iter().find(|a| {
                a.is_output == output
                    && a.name
                        .as_ref()
                        .is_some_and(|n| n.name.eq_ignore_ascii_case(name))
            })
        };
        // The power pin: the standard FB's, when its argument can be drawn.
        let power_pin = spec
            .as_ref()
            .and_then(|s| s.power_in)
            .filter(|p| arg(p, false).is_some_and(|a| self.drawable_bool(&a.value)));
        let mut elements = Vec::new();
        if let Some(p) = power_pin {
            let a = arg(p, false)?;
            elements = self.contacts(&a.value, false);
        }
        let mut pins = Vec::new();
        for (n, dir) in &pin_names {
            let value = match dir {
                PinDir::Output => arg(n, true).map(|a| text(&a.value)),
                _ if Some(n.as_str()) == power_pin => None,
                _ => arg(n, false).map(|a| text(&a.value)),
            };
            pins.push(Pin {
                name: n.clone(),
                dir: *dir,
                value,
                ..Default::default()
            });
        }
        // `v := inst.OUT` right after the call: output pins; `x := inst.Q`
        // (the standard power output) drives a coil from the box.
        let mut used = 0;
        let mut coil = None;
        let power_out = spec.as_ref().and_then(|s| s.power_out);
        for st in rest {
            let StatementKind::Assignment { target, value } = &st.kind else {
                break;
            };
            let ExpressionKind::MemberAccess { object, member } = &unparen(value).kind else {
                break;
            };
            let ExpressionKind::Identifier(o) = &unparen(object).kind else {
                break;
            };
            if !o.name.eq_ignore_ascii_case(&inst.name) || !is_path(target) {
                break;
            }
            let Some(pin) = pins
                .iter_mut()
                .find(|p| p.dir == PinDir::Output && p.name.eq_ignore_ascii_case(&member.name))
            else {
                break;
            };
            let is_power_out = power_out.is_some_and(|po| po.eq_ignore_ascii_case(&member.name));
            if is_power_out && coil.is_none() && self.is_bool_path(target) && power_pin.is_some() {
                coil = Some(text(target));
            } else if pin.value.is_none() {
                pin.value = Some(text(target));
            } else {
                break;
            }
            used += 1;
        }
        let id = self.id();
        elements.push(Element::Block(Block {
            id,
            name: ty,
            instance: Some(inst.name.clone()),
            pins,
            power_in: power_pin.map(str::to_string),
            power_out: coil.as_ref().and(power_out.map(str::to_string)),
            ..Default::default()
        }));
        if let Some(c) = coil {
            let id = self.id();
            elements.push(Element::Coil(Coil {
                id,
                operand: c,
                ..Default::default()
            }));
        }
        Some((elements, used))
    }
}
