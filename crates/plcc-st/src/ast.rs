// SPDX-License-Identifier: MPL-2.0

use crate::span::Span;
use serde::{Deserialize, Serialize};

/// A complete compilation unit — one source file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompilationUnit {
    pub declarations: Vec<Declaration>,
    pub span: Span,
}

/// Top-level declarations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Declaration {
    Program(ProgramDecl),
    Function(FunctionDecl),
    FunctionBlock(FunctionBlockDecl),
    Class(ClassDecl),
    Interface(InterfaceDecl),
    TypeDecl(TypeDeclaration),
    GlobalVarDecl(VarBlock),
    Configuration(ConfigurationDecl),
}

// ── POUs ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgramDecl {
    pub name: Ident,
    pub var_blocks: Vec<VarBlock>,
    /// CODESYS/TwinCAT: a PROGRAM may have methods, properties and actions,
    /// called as `Prg.M()` (or `M()` inside it).
    #[serde(default)]
    pub methods: Vec<MethodDecl>,
    #[serde(default)]
    pub properties: Vec<PropertyDecl>,
    #[serde(default)]
    pub actions: Vec<ActionDecl>,
    pub body: Vec<Statement>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDecl {
    pub name: Ident,
    pub return_type: Option<TypeSpec>,
    pub var_blocks: Vec<VarBlock>,
    pub body: Vec<Statement>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionBlockDecl {
    pub name: Ident,
    pub extends: Option<Ident>,
    pub implements: Vec<Ident>,
    pub var_blocks: Vec<VarBlock>,
    pub methods: Vec<MethodDecl>,
    /// CODESYS/TwinCAT properties (`PROPERTY P : T` with GET/SET accessors).
    #[serde(default)]
    pub properties: Vec<PropertyDecl>,
    /// CODESYS/TwinCAT actions: parameterless bodies run in the instance's context.
    #[serde(default)]
    pub actions: Vec<ActionDecl>,
    pub body: Vec<Statement>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassDecl {
    pub name: Ident,
    pub access: Option<AccessModifier>,
    pub is_abstract: bool,
    pub is_final: bool,
    pub extends: Option<Ident>,
    pub implements: Vec<Ident>,
    pub var_blocks: Vec<VarBlock>,
    pub methods: Vec<MethodDecl>,
    #[serde(default)]
    pub properties: Vec<PropertyDecl>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterfaceDecl {
    pub name: Ident,
    pub extends: Vec<Ident>,
    pub methods: Vec<MethodDecl>,
    #[serde(default)]
    pub properties: Vec<PropertyDecl>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MethodDecl {
    pub name: Ident,
    pub access: Option<AccessModifier>,
    pub is_override: bool,
    pub is_abstract: bool,
    pub is_final: bool,
    pub return_type: Option<TypeSpec>,
    pub var_blocks: Vec<VarBlock>,
    pub body: Vec<Statement>,
    pub span: Span,
}

/// A CODESYS/TwinCAT `PROPERTY`: a typed member read through its GET accessor
/// and written through its SET accessor. Inside GET the property's name is the
/// return value; inside SET it is the value being assigned.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropertyDecl {
    pub name: Ident,
    pub access: Option<AccessModifier>,
    pub is_abstract: bool,
    pub is_final: bool,
    pub type_spec: TypeSpec,
    pub get: Option<PropertyAccessor>,
    pub set: Option<PropertyAccessor>,
    pub span: Span,
}

/// The GET or SET half of a property: local variables and a body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropertyAccessor {
    pub var_blocks: Vec<VarBlock>,
    pub body: Vec<Statement>,
    pub span: Span,
}

/// A CODESYS/TwinCAT `ACTION`: a named, parameterless statement list of a
/// FUNCTION_BLOCK, called as `inst.Action()` (or `Action()` inside the FB).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionDecl {
    pub name: Ident,
    pub body: Vec<Statement>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AccessModifier {
    Public,
    Private,
    Protected,
    Internal,
}

// ── Variables ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VarBlock {
    pub kind: VarBlockKind,
    /// The name of the global variable list a VAR_GLOBAL block belongs to
    /// (TwinCAT/CODESYS GVL objects), so `GVL.x` resolves. `None` in plain ST.
    #[serde(default)]
    pub list_name: Option<Ident>,
    pub is_constant: bool,
    pub is_retain: bool,
    pub is_non_retain: bool,
    pub declarations: Vec<VarDecl>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum VarBlockKind {
    Var,
    VarInput,
    VarOutput,
    VarInOut,
    VarGlobal,
    VarExternal,
    VarTemp,
    VarAccess,
    VarConfig,
    /// CODESYS `VAR_INST` in a METHOD: stored in the instance, so it keeps its
    /// value between calls of the method.
    VarInst,
    /// CODESYS `VAR_STAT` in a FUNCTION_BLOCK, FUNCTION or METHOD: static
    /// storage shared by all instances and calls.
    VarStat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VarDecl {
    pub name: Ident,
    pub type_spec: TypeSpec,
    pub at_address: Option<DirectVariable>,
    pub edge: Option<EdgeKind>,
    pub initializer: Option<Expression>,
    /// CODESYS declaration-site FB_init arguments: `fb : FB_X(1, THIS^);`
    /// passes them to the instance's `FB_init` method when it is initialized.
    #[serde(default)]
    pub init_args: Vec<CallArg>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum EdgeKind {
    Rising,
    Falling,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectVariable {
    pub repr: String,
    pub span: Span,
}

// ── Types ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeSpec {
    pub kind: TypeSpecKind,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TypeSpecKind {
    /// Elementary type name or user-defined type name.
    Named(Ident),
    /// STRING[length] or WSTRING[length].
    StringType {
        wide: bool,
        length: Option<Box<Expression>>,
    },
    /// ARRAY[lo..hi, ...] OF base
    Array {
        ranges: Vec<SubrangeSpec>,
        base: Box<TypeSpec>,
    },
    /// A variable-length array, `ARRAY[*, *] OF base` (IEC 61131-3 ed. 3,
    /// VAR_IN_OUT only): the bounds come from the argument.
    VarLengthArray {
        dimensions: usize,
        base: Box<TypeSpec>,
    },
    /// POINTER TO base or REF_TO base
    Pointer(Box<TypeSpec>),
    /// CODESYS `REFERENCE TO base`: stored as an address, but every use of the
    /// variable means the referenced value; `r REF= x` rebinds it.
    Reference(Box<TypeSpec>),
    /// Subrange: INT(0..100)
    Subrange {
        base: Ident,
        low: Box<Expression>,
        high: Box<Expression>,
    },
    /// Inline struct
    Struct(Vec<StructField>),
    /// Inline enum
    Enum(EnumSpec),
    /// Inline union
    Union(Vec<StructField>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubrangeSpec {
    pub low: Expression,
    pub high: Expression,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructField {
    pub name: Ident,
    pub type_spec: TypeSpec,
    pub initializer: Option<Expression>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnumSpec {
    pub base_type: Option<Ident>,
    pub values: Vec<EnumValue>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnumValue {
    pub name: Ident,
    pub value: Option<Expression>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeDeclaration {
    pub name: Ident,
    /// CODESYS `TYPE D EXTENDS B : STRUCT ...`: the base structure, whose
    /// fields come first.
    #[serde(default)]
    pub extends: Option<Ident>,
    /// `{attribute '...'}` pragmas written before the declaration, by name
    /// (lowercase): `to_string`, `qualified_only`, `strict`, ...
    #[serde(default)]
    pub attributes: Vec<String>,
    pub type_spec: TypeSpec,
    pub initializer: Option<Expression>,
    pub span: Span,
}

// ── Statements ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Statement {
    pub kind: StatementKind,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StatementKind {
    Assignment {
        target: Expression,
        value: Expression,
    },
    FunctionCall {
        callee: Expression,
        args: Vec<CallArg>,
    },
    If {
        condition: Expression,
        then_body: Vec<Statement>,
        elsif_branches: Vec<ElsifBranch>,
        else_body: Option<Vec<Statement>>,
    },
    Case {
        selector: Expression,
        branches: Vec<CaseBranch>,
        else_body: Option<Vec<Statement>>,
    },
    For {
        /// The control variable: a name, or (CODESYS / TwinCAT) any integer
        /// location such as `idx[2]`.
        variable: Expression,
        from: Expression,
        to: Expression,
        by: Option<Expression>,
        body: Vec<Statement>,
    },
    While {
        condition: Expression,
        body: Vec<Statement>,
    },
    Repeat {
        body: Vec<Statement>,
        until: Expression,
    },
    Exit,
    Continue,
    Return {
        value: Option<Expression>,
    },
    /// Empty statement (bare semicolon).
    Empty,
    /// A comment kept as a statement, so it survives into printed ST. Never
    /// produced by the parser (source comments are skipped by the lexer); the
    /// ladder lowerings add one before each rung when asked to (`plcc convert`).
    /// Does nothing.
    Comment(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElsifBranch {
    pub condition: Expression,
    pub body: Vec<Statement>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseBranch {
    pub labels: Vec<CaseLabel>,
    pub body: Vec<Statement>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CaseLabel {
    Value(Expression),
    Range(Expression, Expression),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallArg {
    pub name: Option<Ident>,
    pub value: Expression,
    pub is_output: bool,
    /// `NOT Q => x`: the output is inverted on its way to `x` (IEC 61131-3 §6.6.1.4).
    /// Only meaningful with `is_output`.
    #[serde(default)]
    pub negated: bool,
    pub span: Span,
}

// ── Expressions ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Expression {
    pub kind: ExpressionKind,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExpressionKind {
    IntegerLiteral(i128),
    RealLiteral(f64),
    StringLiteral(String),
    WstringLiteral(String),
    BoolLiteral(bool),
    TimeLiteral(String),
    DateLiteral(String),
    TodLiteral(String),
    DtLiteral(String),
    /// Typed literal: TYPE#value, e.g. INT#5
    TypedLiteral {
        type_name: Ident,
        value: Box<Expression>,
    },
    Identifier(Ident),
    DirectVariable(String),
    BinaryOp {
        op: BinaryOp,
        left: Box<Expression>,
        right: Box<Expression>,
    },
    UnaryOp {
        op: UnaryOp,
        operand: Box<Expression>,
    },
    FunctionCall {
        callee: Box<Expression>,
        args: Vec<CallArg>,
    },
    MemberAccess {
        object: Box<Expression>,
        member: Ident,
    },
    ArrayIndex {
        array: Box<Expression>,
        indices: Vec<Expression>,
    },
    Dereference(Box<Expression>),
    Parenthesized(Box<Expression>),
    /// An array aggregate initializer: `[10, 20, 30]`, `[3(0), 2(7)]`, or the
    /// bracket-less `10, 20, 30` that IEC 61131-3 also allows after `:=`.
    ///
    /// Only ever appears as a variable/field initializer — it has no address and no
    /// scalar value, so it is not a general expression.
    ArrayInitializer(Vec<ArrayInitElement>),
    /// A structure initializer: `(x := 1, y := (a := 2))` (IEC 61131-3 §6.4.4.6,
    /// `struct_init`). Fields not named keep their declared default. Like
    /// `ArrayInitializer`, only ever an initial value.
    StructInitializer(Vec<StructInitField>),
}

/// One `name := value` of a structure initializer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructInitField {
    pub name: Ident,
    pub value: Expression,
    pub span: Span,
}

/// One entry of an array aggregate initializer.
///
/// `repeat` carries IEC 61131-3's repetition syntax: `3(0)` is three copies of `0`.
/// `None` means a single value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArrayInitElement {
    pub repeat: Option<Box<Expression>>,
    pub value: Box<Expression>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Power,
    And,
    Or,
    Xor,
    /// CODESYS `AND_THEN`: BOOL only, right operand evaluated only if the left
    /// one is TRUE.
    AndThen,
    /// CODESYS `OR_ELSE`: BOOL only, right operand evaluated only if the left
    /// one is FALSE.
    OrElse,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

// ── Configuration ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigurationDecl {
    pub name: Ident,
    pub global_vars: Vec<VarBlock>,
    pub resources: Vec<ResourceDecl>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceDecl {
    pub name: Ident,
    pub on: Option<Ident>,
    pub global_vars: Vec<VarBlock>,
    pub tasks: Vec<TaskDecl>,
    pub program_configs: Vec<ProgramConfig>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDecl {
    pub name: Ident,
    pub properties: Vec<(Ident, Expression)>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgramConfig {
    pub name: Ident,
    pub task: Option<Ident>,
    pub program_type: Ident,
    /// `PROGRAM p WITH t : Main (inp := g_a, outp => g_b);` — input connections
    /// (`:=`) are copied in before each scan, output connections (`=>`) out after.
    #[serde(default)]
    pub connections: Vec<CallArg>,
    pub span: Span,
}

// ── Common ──

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

impl Ident {
    pub fn new(name: impl Into<String>, span: Span) -> Self {
        Self {
            name: name.into(),
            span,
        }
    }
}
