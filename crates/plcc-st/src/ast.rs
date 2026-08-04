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
    pub span: Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterfaceDecl {
    pub name: Ident,
    pub extends: Vec<Ident>,
    pub methods: Vec<MethodDecl>,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VarDecl {
    pub name: Ident,
    pub type_spec: TypeSpec,
    pub at_address: Option<DirectVariable>,
    pub edge: Option<EdgeKind>,
    pub initializer: Option<Expression>,
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
    /// POINTER TO base or REF_TO base
    Pointer(Box<TypeSpec>),
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
        variable: Ident,
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
