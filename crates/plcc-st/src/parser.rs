// SPDX-License-Identifier: MPL-2.0
#![allow(unused_assignments)]

use crate::ast::*;
use crate::span::Span;
use crate::token::Token;
use logos::Logos;
use miette::Diagnostic;
use thiserror::Error;

#[derive(Debug, Clone, Error, Diagnostic)]
pub enum ParseError {
    #[error("unexpected token: found {found}, expected {expected}")]
    UnexpectedToken {
        found: String,
        expected: String,
        #[label("here")]
        span: miette::SourceSpan,
    },

    #[error("unexpected end of input, expected {expected}")]
    UnexpectedEof {
        expected: String,
        #[label("here")]
        span: miette::SourceSpan,
    },

    #[error("{message}")]
    General {
        message: String,
        #[label("{message}")]
        span: miette::SourceSpan,
    },
}

struct TokenStream {
    tokens: Vec<(Token, Span)>,
    pos: usize,
    source_len: usize,
}

impl TokenStream {
    fn new(source: &str) -> (Self, Vec<ParseError>) {
        let mut tokens = Vec::new();
        let mut errors = Vec::new();
        let lexer = Token::lexer(source);
        for (result, range) in lexer.spanned() {
            let span = Span::from(range.clone());
            match result {
                Ok(tok) => tokens.push((tok, span)),
                Err(()) => {
                    errors.push(ParseError::General {
                        message: format!(
                            "unexpected character: {:?}",
                            &source[range.start..range.end]
                        ),
                        span: span.into(),
                    });
                }
            }
        }
        (
            TokenStream {
                tokens,
                pos: 0,
                source_len: source.len(),
            },
            errors,
        )
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos).map(|(t, _)| t)
    }

    fn peek_span(&self) -> Span {
        self.tokens
            .get(self.pos)
            .map(|(_, s)| *s)
            .unwrap_or(Span::new(self.source_len, self.source_len))
    }

    fn advance(&mut self) -> Option<(Token, Span)> {
        if self.pos < self.tokens.len() {
            let item = self.tokens[self.pos].clone();
            self.pos += 1;
            Some(item)
        } else {
            None
        }
    }

    fn expect(&mut self, expected: &Token, errors: &mut Vec<ParseError>) -> Option<Span> {
        if self.peek() == Some(expected) {
            Some(self.advance().unwrap().1)
        } else {
            let span = self.peek_span();
            let found = self
                .peek()
                .map(|t| format!("{t}"))
                .unwrap_or("end of input".into());
            errors.push(ParseError::UnexpectedToken {
                found,
                expected: format!("{expected}"),
                span: span.into(),
            });
            None
        }
    }

    fn at(&self, token: &Token) -> bool {
        self.peek() == Some(token)
    }

    fn eat(&mut self, token: &Token) -> Option<Span> {
        if self.at(token) {
            Some(self.advance().unwrap().1)
        } else {
            None
        }
    }

    fn slice<'s>(&self, source: &'s str, span: &Span) -> &'s str {
        &source[span.start..span.end]
    }
}

pub struct Parser<'s> {
    ts: TokenStream,
    source: &'s str,
    pub errors: Vec<ParseError>,
}

impl<'s> Parser<'s> {
    pub fn new(source: &'s str) -> Self {
        let (ts, errors) = TokenStream::new(source);
        Parser { ts, source, errors }
    }

    pub fn parse(mut self) -> (CompilationUnit, Vec<ParseError>) {
        let start = Span::new(0, 0);
        let mut declarations = Vec::new();

        while self.ts.peek().is_some() {
            match self.parse_declaration() {
                Some(decl) => declarations.push(decl),
                None => {
                    // Error recovery: skip token
                    if self.ts.advance().is_none() {
                        break;
                    }
                }
            }
        }

        let end_span = if let Some(last) = declarations.last() {
            match last {
                Declaration::Program(d) => d.span,
                Declaration::Function(d) => d.span,
                Declaration::FunctionBlock(d) => d.span,
                Declaration::Class(d) => d.span,
                Declaration::Interface(d) => d.span,
                Declaration::TypeDecl(d) => d.span,
                Declaration::GlobalVarDecl(d) => d.span,
                Declaration::Configuration(d) => d.span,
            }
        } else {
            start
        };

        let unit = CompilationUnit {
            declarations,
            span: start.merge(end_span),
        };
        let errors = self.errors;
        (unit, errors)
    }

    fn parse_declaration(&mut self) -> Option<Declaration> {
        match self.ts.peek()? {
            Token::Program => Some(Declaration::Program(self.parse_program())),
            Token::Function => {
                // Distinguish FUNCTION from FUNCTION_BLOCK by peeking — but logos
                // already handles this as separate tokens.
                Some(Declaration::Function(self.parse_function()))
            }
            Token::FunctionBlock => {
                Some(Declaration::FunctionBlock(self.parse_function_block()))
            }
            Token::Class | Token::Abstract | Token::Final => {
                Some(Declaration::Class(self.parse_class()))
            }
            Token::Interface => Some(Declaration::Interface(self.parse_interface())),
            Token::Type => self.parse_type_decl_block(),
            Token::VarGlobal => {
                Some(Declaration::GlobalVarDecl(self.parse_var_block()))
            }
            Token::Configuration => {
                Some(Declaration::Configuration(self.parse_configuration()))
            }
            _ => {
                let span = self.ts.peek_span();
                self.errors.push(ParseError::General {
                    message: format!(
                        "expected declaration, found {}",
                        self.ts.peek().map(|t| format!("{t}")).unwrap_or_default()
                    ),
                    span: span.into(),
                });
                None
            }
        }
    }

    // ── PROGRAM ──

    fn parse_program(&mut self) -> ProgramDecl {
        let start = self.ts.advance().unwrap().1; // consume PROGRAM
        let name = self.expect_ident();
        let mut var_blocks = Vec::new();
        let mut body = Vec::new();

        loop {
            match self.ts.peek() {
                Some(Token::EndProgram) | None => break,
                Some(t) if Self::is_var_block_start(t) => {
                    var_blocks.push(self.parse_var_block());
                }
                _ => {
                    body = self.parse_statement_list(&[Token::EndProgram]);
                    break;
                }
            }
        }

        let end = self
            .ts
            .expect(&Token::EndProgram, &mut self.errors)
            .unwrap_or(self.ts.peek_span());
        // Optional trailing semicolon
        self.ts.eat(&Token::Semicolon);

        ProgramDecl {
            name,
            var_blocks,
            body,
            span: start.merge(end),
        }
    }

    // ── FUNCTION ──

    fn parse_function(&mut self) -> FunctionDecl {
        let start = self.ts.advance().unwrap().1; // consume FUNCTION
        let name = self.expect_ident();

        let return_type = if self.ts.eat(&Token::Colon).is_some() {
            Some(self.parse_type_spec())
        } else {
            None
        };

        let mut var_blocks = Vec::new();
        let mut body = Vec::new();

        loop {
            match self.ts.peek() {
                Some(Token::EndFunction) | None => break,
                Some(t) if Self::is_var_block_start(t) => {
                    var_blocks.push(self.parse_var_block());
                }
                _ => {
                    body = self.parse_statement_list(&[Token::EndFunction]);
                    break;
                }
            }
        }

        let end = self
            .ts
            .expect(&Token::EndFunction, &mut self.errors)
            .unwrap_or(self.ts.peek_span());
        self.ts.eat(&Token::Semicolon);

        FunctionDecl {
            name,
            return_type,
            var_blocks,
            body,
            span: start.merge(end),
        }
    }

    // ── FUNCTION_BLOCK ──

    fn parse_function_block(&mut self) -> FunctionBlockDecl {
        let start = self.ts.advance().unwrap().1; // consume FUNCTION_BLOCK
        let name = self.expect_ident();

        let extends = if self.ts.eat(&Token::Extends).is_some() {
            Some(self.expect_ident())
        } else {
            None
        };

        let implements = if self.ts.eat(&Token::Implements).is_some() {
            self.parse_ident_list()
        } else {
            Vec::new()
        };

        let mut var_blocks = Vec::new();
        let mut methods = Vec::new();
        let mut body = Vec::new();

        loop {
            match self.ts.peek() {
                Some(Token::EndFunctionBlock) | None => break,
                Some(t) if Self::is_var_block_start(t) => {
                    var_blocks.push(self.parse_var_block());
                }
                Some(Token::Method) => {
                    methods.push(self.parse_method());
                }
                _ => {
                    body = self.parse_statement_list(&[Token::EndFunctionBlock, Token::Method]);
                    // Don't break — there might be methods after the body
                    if !matches!(self.ts.peek(), Some(Token::Method)) {
                        break;
                    }
                }
            }
        }

        let end = self
            .ts
            .expect(&Token::EndFunctionBlock, &mut self.errors)
            .unwrap_or(self.ts.peek_span());
        self.ts.eat(&Token::Semicolon);

        FunctionBlockDecl {
            name,
            extends,
            implements,
            var_blocks,
            methods,
            body,
            span: start.merge(end),
        }
    }

    // ── CLASS ──

    fn parse_class(&mut self) -> ClassDecl {
        let start = self.ts.peek_span();
        let is_abstract = self.ts.eat(&Token::Abstract).is_some();
        let is_final = if !is_abstract {
            self.ts.eat(&Token::Final).is_some()
        } else {
            false
        };
        self.ts.expect(&Token::Class, &mut self.errors);
        let name = self.expect_ident();

        let extends = if self.ts.eat(&Token::Extends).is_some() {
            Some(self.expect_ident())
        } else {
            None
        };

        let implements = if self.ts.eat(&Token::Implements).is_some() {
            self.parse_ident_list()
        } else {
            Vec::new()
        };

        let mut var_blocks = Vec::new();
        let mut methods = Vec::new();

        loop {
            match self.ts.peek() {
                Some(Token::EndClass) | None => break,
                Some(t) if Self::is_var_block_start(t) => {
                    var_blocks.push(self.parse_var_block());
                }
                Some(Token::Method)
                | Some(Token::Public)
                | Some(Token::Private)
                | Some(Token::Protected)
                | Some(Token::Internal)
                | Some(Token::Override)
                | Some(Token::Abstract)
                | Some(Token::Final) => {
                    methods.push(self.parse_method());
                }
                _ => {
                    // Skip unexpected token with error recovery
                    let span = self.ts.peek_span();
                    self.errors.push(ParseError::General {
                        message: "expected variable block or method in class".into(),
                        span: span.into(),
                    });
                    self.ts.advance();
                }
            }
        }

        let end = self
            .ts
            .expect(&Token::EndClass, &mut self.errors)
            .unwrap_or(self.ts.peek_span());
        self.ts.eat(&Token::Semicolon);

        ClassDecl {
            name,
            access: None,
            is_abstract,
            is_final,
            extends,
            implements,
            var_blocks,
            methods,
            span: start.merge(end),
        }
    }

    // ── INTERFACE ──

    fn parse_interface(&mut self) -> InterfaceDecl {
        let start = self.ts.advance().unwrap().1; // consume INTERFACE
        let name = self.expect_ident();

        let extends = if self.ts.eat(&Token::Extends).is_some() {
            self.parse_ident_list()
        } else {
            Vec::new()
        };

        let mut methods = Vec::new();

        loop {
            match self.ts.peek() {
                Some(Token::EndInterface) | None => break,
                Some(Token::Method)
                | Some(Token::Public)
                | Some(Token::Private)
                | Some(Token::Protected) => {
                    methods.push(self.parse_method());
                }
                _ => {
                    let span = self.ts.peek_span();
                    self.errors.push(ParseError::General {
                        message: "expected method declaration in interface".into(),
                        span: span.into(),
                    });
                    self.ts.advance();
                }
            }
        }

        let end = self
            .ts
            .expect(&Token::EndInterface, &mut self.errors)
            .unwrap_or(self.ts.peek_span());
        self.ts.eat(&Token::Semicolon);

        InterfaceDecl {
            name,
            extends,
            methods,
            span: start.merge(end),
        }
    }

    // ── METHOD ──

    fn parse_method(&mut self) -> MethodDecl {
        let start = self.ts.peek_span();

        // Optional access modifier
        let access = self.try_parse_access_modifier();
        let is_override = self.ts.eat(&Token::Override).is_some();
        let is_abstract = self.ts.eat(&Token::Abstract).is_some();
        let is_final = self.ts.eat(&Token::Final).is_some();

        self.ts.expect(&Token::Method, &mut self.errors);
        let name = self.expect_ident();

        let return_type = if self.ts.eat(&Token::Colon).is_some() {
            Some(self.parse_type_spec())
        } else {
            None
        };

        let mut var_blocks = Vec::new();
        let mut body = Vec::new();

        loop {
            match self.ts.peek() {
                Some(Token::EndMethod) | None => break,
                Some(t) if Self::is_var_block_start(t) => {
                    var_blocks.push(self.parse_var_block());
                }
                _ => {
                    body = self.parse_statement_list(&[Token::EndMethod]);
                    break;
                }
            }
        }

        let end = self
            .ts
            .expect(&Token::EndMethod, &mut self.errors)
            .unwrap_or(self.ts.peek_span());
        self.ts.eat(&Token::Semicolon);

        MethodDecl {
            name,
            access,
            is_override,
            is_abstract,
            is_final,
            return_type,
            var_blocks,
            body,
            span: start.merge(end),
        }
    }

    fn try_parse_access_modifier(&mut self) -> Option<AccessModifier> {
        match self.ts.peek()? {
            Token::Public => {
                self.ts.advance();
                Some(AccessModifier::Public)
            }
            Token::Private => {
                self.ts.advance();
                Some(AccessModifier::Private)
            }
            Token::Protected => {
                self.ts.advance();
                Some(AccessModifier::Protected)
            }
            Token::Internal => {
                self.ts.advance();
                Some(AccessModifier::Internal)
            }
            _ => None,
        }
    }

    // ── TYPE declarations ──

    fn parse_type_decl_block(&mut self) -> Option<Declaration> {
        let _start = self.ts.advance().unwrap().1; // consume TYPE

        // Parse all type declarations in this TYPE..END_TYPE block
        let mut first_decl = None;
        while !self.ts.at(&Token::EndType) && self.ts.peek().is_some() {
            let name = self.expect_ident();
            self.ts.expect(&Token::Colon, &mut self.errors);
            let type_spec = self.parse_type_spec();
            let initializer = if self.ts.eat(&Token::Assign).is_some() {
                Some(self.parse_expression())
            } else {
                None
            };
            self.ts.eat(&Token::Semicolon);

            let span = name.span.merge(type_spec.span);
            let decl = TypeDeclaration {
                name,
                type_spec,
                initializer,
                span,
            };

            if first_decl.is_none() {
                first_decl = Some(decl);
            }
            // TODO: return multiple declarations from a single TYPE block
        }

        self.ts.expect(&Token::EndType, &mut self.errors);
        self.ts.eat(&Token::Semicolon);
        first_decl.map(Declaration::TypeDecl)
    }

    // ── Variable blocks ──

    fn is_var_block_start(token: &Token) -> bool {
        matches!(
            token,
            Token::Var
                | Token::VarInput
                | Token::VarOutput
                | Token::VarInOut
                | Token::VarGlobal
                | Token::VarExternal
                | Token::VarTemp
                | Token::VarAccess
                | Token::VarConfig
        )
    }

    fn parse_var_block(&mut self) -> VarBlock {
        let (kind, start) = match self.ts.advance() {
            Some((Token::Var, s)) => (VarBlockKind::Var, s),
            Some((Token::VarInput, s)) => (VarBlockKind::VarInput, s),
            Some((Token::VarOutput, s)) => (VarBlockKind::VarOutput, s),
            Some((Token::VarInOut, s)) => (VarBlockKind::VarInOut, s),
            Some((Token::VarGlobal, s)) => (VarBlockKind::VarGlobal, s),
            Some((Token::VarExternal, s)) => (VarBlockKind::VarExternal, s),
            Some((Token::VarTemp, s)) => (VarBlockKind::VarTemp, s),
            Some((Token::VarAccess, s)) => (VarBlockKind::VarAccess, s),
            Some((Token::VarConfig, s)) => (VarBlockKind::VarConfig, s),
            _ => {
                let s = self.ts.peek_span();
                return VarBlock {
                    kind: VarBlockKind::Var,
                    is_constant: false,
                    is_retain: false,
                    is_non_retain: false,
                    declarations: Vec::new(),
                    span: s,
                };
            }
        };

        let is_constant = self.ts.eat(&Token::Constant).is_some();
        let is_retain = self.ts.eat(&Token::Retain).is_some();
        let is_non_retain = if !is_retain {
            self.ts.eat(&Token::NonRetain).is_some()
        } else {
            false
        };

        let mut declarations = Vec::new();
        while !self.ts.at(&Token::EndVar) && self.ts.peek().is_some() {
            let pos_before = self.ts.pos;
            self.parse_var_decl_list(&mut declarations);
            // Safety: prevent infinite loop
            if self.ts.pos == pos_before {
                self.ts.advance();
            }
        }

        let end = self
            .ts
            .expect(&Token::EndVar, &mut self.errors)
            .unwrap_or(self.ts.peek_span());

        VarBlock {
            kind,
            is_constant,
            is_retain,
            is_non_retain,
            declarations,
            span: start.merge(end),
        }
    }

    /// Parse one or more variable declarations (handles `a, b, c : INT := 0;`).
    fn parse_var_decl_list(&mut self, out: &mut Vec<VarDecl>) {
        // Collect names (comma-separated)
        let mut names = vec![self.expect_ident()];
        while self.ts.eat(&Token::Comma).is_some() {
            names.push(self.expect_ident());
        }

        // Optional AT address (only valid for single-name declarations)
        let at_address = if self.ts.eat(&Token::At).is_some() {
            if matches!(self.ts.peek(), Some(Token::DirectVariable)) {
                let (_, span) = self.ts.advance().unwrap();
                let repr = self.ts.slice(self.source, &span).to_string();
                Some(DirectVariable { repr, span })
            } else {
                None
            }
        } else {
            None
        };

        self.ts.expect(&Token::Colon, &mut self.errors);

        let type_spec = self.parse_type_spec();

        // Optional edge qualifier
        let edge = if self.ts.eat(&Token::REdge).is_some() {
            Some(EdgeKind::Rising)
        } else if self.ts.eat(&Token::FEdge).is_some() {
            Some(EdgeKind::Falling)
        } else {
            None
        };

        let initializer = if self.ts.eat(&Token::Assign).is_some() {
            let first = self.parse_expression();
            // Consume array initializer list: := val1, val2, val3, ...;
            while self.ts.eat(&Token::Comma).is_some() {
                // Skip additional initializer values
                if self.ts.at(&Token::Semicolon) || self.ts.peek().is_none() {
                    break;
                }
                self.parse_expression();
            }
            Some(first)
        } else {
            None
        };

        let end = self.ts.eat(&Token::Semicolon).unwrap_or(self.ts.peek_span());

        // Emit one VarDecl per name, sharing the same type/initializer
        for name in names {
            let start = name.span;
            out.push(VarDecl {
                name,
                type_spec: type_spec.clone(),
                at_address: at_address.clone(),
                edge,
                initializer: initializer.clone(),
                span: start.merge(end),
            });
        }
    }

    // ── Type specs ──

    fn parse_type_spec(&mut self) -> TypeSpec {
        let _start = self.ts.peek_span();

        match self.ts.peek() {
            Some(Token::Array) => self.parse_array_type(),
            Some(Token::Struct) => self.parse_struct_type(),
            Some(Token::Union) => self.parse_union_type(),
            Some(Token::StringType) | Some(Token::WstringType) => self.parse_string_type(),
            Some(Token::Pointer) | Some(Token::RefTo) | Some(Token::Reference) => {
                self.parse_pointer_type()
            }
            Some(Token::LParen) => {
                // Enum spec: (val1, val2, ...)
                self.parse_enum_type_spec(None)
            }
            _ => {
                // Named type, possibly followed by subrange or enum
                let ident = self.expect_ident();
                if self.ts.at(&Token::LParen) {
                    // Could be enum or subrange: MyType(0..10) or (val1, val2)
                    self.parse_enum_or_subrange(ident)
                } else {
                    let span = ident.span;
                    TypeSpec {
                        kind: TypeSpecKind::Named(ident),
                        span,
                    }
                }
            }
        }
    }

    fn parse_array_type(&mut self) -> TypeSpec {
        let start = self.ts.advance().unwrap().1; // consume ARRAY
        self.ts.expect(&Token::LBracket, &mut self.errors);

        let mut ranges = Vec::new();
        loop {
            let low = self.parse_expression();
            self.ts.expect(&Token::DotDot, &mut self.errors);
            let high = self.parse_expression();
            let span = low.span.merge(high.span);
            ranges.push(SubrangeSpec { low, high, span });
            if self.ts.eat(&Token::Comma).is_none() {
                break;
            }
        }

        self.ts.expect(&Token::RBracket, &mut self.errors);
        self.ts.expect(&Token::Of, &mut self.errors);

        let base = self.parse_type_spec();
        let end = base.span;

        TypeSpec {
            kind: TypeSpecKind::Array {
                ranges,
                base: Box::new(base),
            },
            span: start.merge(end),
        }
    }

    fn parse_struct_type(&mut self) -> TypeSpec {
        let start = self.ts.advance().unwrap().1; // consume STRUCT
        let mut fields = Vec::new();

        while !self.ts.at(&Token::EndStruct) && self.ts.peek().is_some() {
            let pos_before = self.ts.pos;
            let name = self.expect_ident();
            self.ts.expect(&Token::Colon, &mut self.errors);
            let type_spec = self.parse_type_spec();
            let initializer = if self.ts.eat(&Token::Assign).is_some() {
                let first = self.parse_expression();
                // Consume array initializer list: := val1, val2, val3, ...;
                while self.ts.eat(&Token::Comma).is_some() {
                    if self.ts.at(&Token::Semicolon) || self.ts.peek().is_none() {
                        break;
                    }
                    self.parse_expression();
                }
                Some(first)
            } else {
                None
            };
            let end_span = self.ts.eat(&Token::Semicolon).unwrap_or(self.ts.peek_span());
            // Safety guard: prevent infinite loop
            if self.ts.pos == pos_before {
                self.ts.advance();
                continue;
            }
            let span = name.span.merge(end_span);
            fields.push(StructField {
                name,
                type_spec,
                initializer,
                span,
            });
        }

        let end = self
            .ts
            .expect(&Token::EndStruct, &mut self.errors)
            .unwrap_or(self.ts.peek_span());

        TypeSpec {
            kind: TypeSpecKind::Struct(fields),
            span: start.merge(end),
        }
    }

    fn parse_union_type(&mut self) -> TypeSpec {
        let start = self.ts.advance().unwrap().1; // consume UNION
        let mut fields = Vec::new();

        while !self.ts.at(&Token::EndUnion) && self.ts.peek().is_some() {
            let name = self.expect_ident();
            self.ts.expect(&Token::Colon, &mut self.errors);
            let type_spec = self.parse_type_spec();
            let end_span = self.ts.eat(&Token::Semicolon).unwrap_or(self.ts.peek_span());
            let span = name.span.merge(end_span);
            fields.push(StructField {
                name,
                type_spec,
                initializer: None,
                span,
            });
        }

        let end = self
            .ts
            .expect(&Token::EndUnion, &mut self.errors)
            .unwrap_or(self.ts.peek_span());

        TypeSpec {
            kind: TypeSpecKind::Union(fields),
            span: start.merge(end),
        }
    }

    fn parse_pointer_type(&mut self) -> TypeSpec {
        let start = self.ts.advance().unwrap().1; // consume POINTER/REF_TO/REFERENCE
        // POINTER TO <type> or REF_TO <type> or REFERENCE TO <type>
        self.ts.eat(&Token::To); // consume optional TO
        let base = self.parse_type_spec();
        let end = base.span;
        TypeSpec {
            kind: TypeSpecKind::Pointer(Box::new(base)),
            span: start.merge(end),
        }
    }

    fn parse_string_type(&mut self) -> TypeSpec {
        let (tok, start) = self.ts.advance().unwrap();
        let wide = matches!(tok, Token::WstringType);

        let length = if self.ts.eat(&Token::LBracket).is_some() {
            let expr = self.parse_expression();
            self.ts.expect(&Token::RBracket, &mut self.errors);
            Some(Box::new(expr))
        } else if self.ts.eat(&Token::LParen).is_some() {
            // CODESYS/OSCAT style: STRING(length)
            let expr = self.parse_expression();
            self.ts.expect(&Token::RParen, &mut self.errors);
            Some(Box::new(expr))
        } else {
            None
        };

        let end = length
            .as_ref()
            .map(|e| e.span)
            .unwrap_or(start);

        TypeSpec {
            kind: TypeSpecKind::StringType { wide, length },
            span: start.merge(end),
        }
    }

    fn parse_enum_type_spec(&mut self, _base: Option<Ident>) -> TypeSpec {
        let start = self.ts.advance().unwrap().1; // consume (
        let mut values = Vec::new();

        loop {
            let name = self.expect_ident();
            let value = if self.ts.eat(&Token::Assign).is_some() {
                Some(self.parse_expression())
            } else {
                None
            };
            let span = name.span;
            values.push(EnumValue { name, value, span });
            if self.ts.eat(&Token::Comma).is_none() {
                break;
            }
        }

        let end = self
            .ts
            .expect(&Token::RParen, &mut self.errors)
            .unwrap_or(self.ts.peek_span());

        TypeSpec {
            kind: TypeSpecKind::Enum(EnumSpec {
                base_type: None,
                values,
                span: start.merge(end),
            }),
            span: start.merge(end),
        }
    }

    fn parse_enum_or_subrange(&mut self, base: Ident) -> TypeSpec {
        // Look ahead: if we see (ident, or (ident) or (ident :=, it's enum
        // If we see (expr .. expr), it's subrange
        let start = base.span;
        self.ts.advance(); // consume (

        // Try to detect subrange: first token could be number or ident, then ..
        let checkpoint = self.ts.pos;
        let first_expr = self.parse_expression();
        if self.ts.at(&Token::DotDot) {
            // Subrange
            self.ts.advance(); // consume ..
            let high = self.parse_expression();
            let end = self
                .ts
                .expect(&Token::RParen, &mut self.errors)
                .unwrap_or(self.ts.peek_span());
            return TypeSpec {
                kind: TypeSpecKind::Subrange {
                    base,
                    low: Box::new(first_expr),
                    high: Box::new(high),
                },
                span: start.merge(end),
            };
        }

        // It's an enum — backtrack and reparse as enum values
        self.ts.pos = checkpoint;
        let mut values = Vec::new();
        loop {
            let name = self.expect_ident();
            let value = if self.ts.eat(&Token::Assign).is_some() {
                Some(self.parse_expression())
            } else {
                None
            };
            let span = name.span;
            values.push(EnumValue { name, value, span });
            if self.ts.eat(&Token::Comma).is_none() {
                break;
            }
        }

        let end = self
            .ts
            .expect(&Token::RParen, &mut self.errors)
            .unwrap_or(self.ts.peek_span());

        TypeSpec {
            kind: TypeSpecKind::Enum(EnumSpec {
                base_type: Some(base),
                values,
                span: start.merge(end),
            }),
            span: start.merge(end),
        }
    }

    // ── Statements ──

    fn parse_statement_list(&mut self, terminators: &[Token]) -> Vec<Statement> {
        let mut stmts = Vec::new();
        while let Some(tok) = self.ts.peek() {
            if terminators.contains(tok) {
                break;
            }
            // Also break on var blocks or other structural tokens
            if Self::is_var_block_start(tok) {
                break;
            }
            let pos_before = self.ts.pos;
            match self.parse_statement() {
                Some(stmt) => stmts.push(stmt),
                None => {
                    // Error recovery: skip only if we haven't advanced
                    if self.ts.pos == pos_before {
                        if self.ts.advance().is_none() {
                            break;
                        }
                    }
                }
            }
            // Safety: if we haven't advanced at all, force advance to prevent infinite loop
            if self.ts.pos == pos_before {
                if self.ts.advance().is_none() {
                    break;
                }
            }
        }
        stmts
    }

    fn parse_statement(&mut self) -> Option<Statement> {
        let start = self.ts.peek_span();

        match self.ts.peek()? {
            Token::Semicolon => {
                let span = self.ts.advance().unwrap().1;
                Some(Statement {
                    kind: StatementKind::Empty,
                    span,
                })
            }
            Token::If => Some(self.parse_if_statement()),
            Token::Case => Some(self.parse_case_statement()),
            Token::For => Some(self.parse_for_statement()),
            Token::While => Some(self.parse_while_statement()),
            Token::Repeat => Some(self.parse_repeat_statement()),
            Token::Exit => {
                self.ts.advance();
                self.ts.eat(&Token::Semicolon);
                Some(Statement {
                    kind: StatementKind::Exit,
                    span: start,
                })
            }
            Token::Continue => {
                self.ts.advance();
                self.ts.eat(&Token::Semicolon);
                Some(Statement {
                    kind: StatementKind::Continue,
                    span: start,
                })
            }
            Token::Return => {
                self.ts.advance();
                let value = if !self.ts.at(&Token::Semicolon) && self.ts.peek().is_some() {
                    // Check if the next token could be start of an expression
                    match self.ts.peek() {
                        Some(Token::EndFunction)
                        | Some(Token::EndFunctionBlock)
                        | Some(Token::EndMethod)
                        | Some(Token::EndProgram) => None,
                        _ => Some(self.parse_expression()),
                    }
                } else {
                    None
                };
                self.ts.eat(&Token::Semicolon);
                Some(Statement {
                    kind: StatementKind::Return { value },
                    span: start,
                })
            }
            _ => {
                // Assignment or function call
                let expr = self.parse_expression();

                if self.ts.eat(&Token::Assign).is_some() {
                    // Assignment
                    let value = self.parse_expression();
                    let end = self.ts.eat(&Token::Semicolon).unwrap_or(self.ts.peek_span());
                    Some(Statement {
                        kind: StatementKind::Assignment {
                            target: expr,
                            value,
                        },
                        span: start.merge(end),
                    })
                } else {
                    // Expression statement (function call or bare expression)
                    let end = self.ts.eat(&Token::Semicolon).unwrap_or(self.ts.peek_span());
                    match expr.kind {
                        ExpressionKind::FunctionCall { callee, args } => Some(Statement {
                            kind: StatementKind::FunctionCall {
                                callee: *callee,
                                args,
                            },
                            span: start.merge(end),
                        }),
                        _ => {
                            // Bare expression — emit as function call with no args
                            // (common in ST for side-effect expressions)
                            Some(Statement {
                                kind: StatementKind::FunctionCall {
                                    callee: expr,
                                    args: Vec::new(),
                                },
                                span: start.merge(end),
                            })
                        }
                    }
                }
            }
        }
    }

    fn parse_if_statement(&mut self) -> Statement {
        let start = self.ts.advance().unwrap().1; // consume IF
        let condition = self.parse_expression();
        self.ts.expect(&Token::Then, &mut self.errors);

        let then_body = self.parse_statement_list(&[Token::Elsif, Token::Else, Token::EndIf]);

        let mut elsif_branches = Vec::new();
        while self.ts.eat(&Token::Elsif).is_some() {
            let elsif_start = self.ts.peek_span();
            let cond = self.parse_expression();
            self.ts.expect(&Token::Then, &mut self.errors);
            let body = self.parse_statement_list(&[Token::Elsif, Token::Else, Token::EndIf]);
            let span = elsif_start.merge(
                body.last()
                    .map(|s| s.span)
                    .unwrap_or(elsif_start),
            );
            elsif_branches.push(ElsifBranch {
                condition: cond,
                body,
                span,
            });
        }

        let else_body = if self.ts.eat(&Token::Else).is_some() {
            Some(self.parse_statement_list(&[Token::EndIf]))
        } else {
            None
        };

        let end = self
            .ts
            .expect(&Token::EndIf, &mut self.errors)
            .unwrap_or(self.ts.peek_span());
        self.ts.eat(&Token::Semicolon);

        Statement {
            kind: StatementKind::If {
                condition,
                then_body,
                elsif_branches,
                else_body,
            },
            span: start.merge(end),
        }
    }

    fn parse_case_statement(&mut self) -> Statement {
        let start = self.ts.advance().unwrap().1; // consume CASE
        let selector = self.parse_expression();
        self.ts.expect(&Token::Of, &mut self.errors);

        let mut branches = Vec::new();
        let mut else_body = None;

        loop {
            match self.ts.peek() {
                Some(Token::EndCase) | None => break,
                Some(Token::Else) => {
                    self.ts.advance();
                    else_body = Some(self.parse_statement_list(&[Token::EndCase]));
                    break;
                }
                _ => {
                    let branch_start = self.ts.peek_span();
                    let mut labels = Vec::new();
                    loop {
                        let expr = self.parse_expression();
                        if self.ts.eat(&Token::DotDot).is_some() {
                            let high = self.parse_expression();
                            labels.push(CaseLabel::Range(expr, high));
                        } else {
                            labels.push(CaseLabel::Value(expr));
                        }
                        if self.ts.eat(&Token::Comma).is_none() {
                            break;
                        }
                    }
                    self.ts.expect(&Token::Colon, &mut self.errors);
                    let body = self.parse_case_branch_body();
                    let span = branch_start.merge(
                        body.last().map(|s| s.span).unwrap_or(branch_start),
                    );
                    branches.push(CaseBranch {
                        labels,
                        body,
                        span,
                    });
                }
            }
        }

        let end = self
            .ts
            .expect(&Token::EndCase, &mut self.errors)
            .unwrap_or(self.ts.peek_span());
        self.ts.eat(&Token::Semicolon);

        Statement {
            kind: StatementKind::Case {
                selector,
                branches,
                else_body,
            },
            span: start.merge(end),
        }
    }

    /// Check if current position looks like a case label start:
    /// integer/identifier followed by : or , (but not :=)
    fn is_case_label_start(&self) -> bool {
        let tok = match self.ts.tokens.get(self.ts.pos) {
            Some((t, _)) => t,
            None => return false,
        };
        // Case labels are integers, identifiers, or enum values
        let is_label_token = matches!(
            tok,
            Token::IntegerLiteral(_) | Token::Identifier | Token::True | Token::False
        );
        if !is_label_token {
            return false;
        }
        // Look at what follows: must be :, ,, or ..
        match self.ts.tokens.get(self.ts.pos + 1) {
            Some((Token::Colon, _)) => true,
            Some((Token::Comma, _)) => true,
            Some((Token::DotDot, _)) => true,
            _ => false,
        }
    }

    fn parse_case_branch_body(&mut self) -> Vec<Statement> {
        let mut stmts = Vec::new();
        while let Some(tok) = self.ts.peek() {
            if matches!(tok, Token::EndCase | Token::Else) {
                break;
            }
            if self.is_case_label_start() {
                break;
            }
            if Self::is_var_block_start(tok) {
                break;
            }
            match self.parse_statement() {
                Some(stmt) => stmts.push(stmt),
                None => {
                    if self.ts.advance().is_none() {
                        break;
                    }
                }
            }
        }
        stmts
    }

    fn parse_for_statement(&mut self) -> Statement {
        let start = self.ts.advance().unwrap().1; // consume FOR
        let variable = self.expect_ident();
        self.ts.expect(&Token::Assign, &mut self.errors);
        let from = self.parse_expression();
        self.ts.expect(&Token::To, &mut self.errors);
        let to = self.parse_expression();
        let by = if self.ts.eat(&Token::By).is_some() {
            Some(self.parse_expression())
        } else {
            None
        };
        self.ts.expect(&Token::Do, &mut self.errors);

        let body = self.parse_statement_list(&[Token::EndFor]);

        let end = self
            .ts
            .expect(&Token::EndFor, &mut self.errors)
            .unwrap_or(self.ts.peek_span());
        self.ts.eat(&Token::Semicolon);

        Statement {
            kind: StatementKind::For {
                variable,
                from,
                to,
                by,
                body,
            },
            span: start.merge(end),
        }
    }

    fn parse_while_statement(&mut self) -> Statement {
        let start = self.ts.advance().unwrap().1; // consume WHILE
        let condition = self.parse_expression();
        self.ts.expect(&Token::Do, &mut self.errors);
        let body = self.parse_statement_list(&[Token::EndWhile]);
        let end = self
            .ts
            .expect(&Token::EndWhile, &mut self.errors)
            .unwrap_or(self.ts.peek_span());
        self.ts.eat(&Token::Semicolon);

        Statement {
            kind: StatementKind::While { condition, body },
            span: start.merge(end),
        }
    }

    fn parse_repeat_statement(&mut self) -> Statement {
        let start = self.ts.advance().unwrap().1; // consume REPEAT
        let body = self.parse_statement_list(&[Token::Until]);
        self.ts.expect(&Token::Until, &mut self.errors);
        let until = self.parse_expression();
        let end = self
            .ts
            .expect(&Token::EndRepeat, &mut self.errors)
            .unwrap_or(self.ts.peek_span());
        self.ts.eat(&Token::Semicolon);

        Statement {
            kind: StatementKind::Repeat { body, until },
            span: start.merge(end),
        }
    }

    // ── Expressions (Pratt parser) ──

    fn parse_expression(&mut self) -> Expression {
        self.parse_or_expression()
    }

    fn parse_or_expression(&mut self) -> Expression {
        let mut left = self.parse_xor_expression();
        while self.ts.at(&Token::Or) {
            self.ts.advance();
            let right = self.parse_xor_expression();
            let span = left.span.merge(right.span);
            left = Expression {
                kind: ExpressionKind::BinaryOp {
                    op: BinaryOp::Or,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            };
        }
        left
    }

    fn parse_xor_expression(&mut self) -> Expression {
        let mut left = self.parse_and_expression();
        while self.ts.at(&Token::Xor) {
            self.ts.advance();
            let right = self.parse_and_expression();
            let span = left.span.merge(right.span);
            left = Expression {
                kind: ExpressionKind::BinaryOp {
                    op: BinaryOp::Xor,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            };
        }
        left
    }

    fn parse_and_expression(&mut self) -> Expression {
        let mut left = self.parse_comparison();
        while self.ts.at(&Token::And) || self.ts.at(&Token::Ampersand) {
            self.ts.advance();
            let right = self.parse_comparison();
            let span = left.span.merge(right.span);
            left = Expression {
                kind: ExpressionKind::BinaryOp {
                    op: BinaryOp::And,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            };
        }
        left
    }

    fn parse_comparison(&mut self) -> Expression {
        let mut left = self.parse_addition();
        loop {
            let op = match self.ts.peek() {
                Some(Token::Equal) => BinaryOp::Equal,
                Some(Token::NotEqual) => BinaryOp::NotEqual,
                Some(Token::Less) => BinaryOp::Less,
                Some(Token::LessEqual) => BinaryOp::LessEqual,
                Some(Token::Greater) => BinaryOp::Greater,
                Some(Token::GreaterEqual) => BinaryOp::GreaterEqual,
                _ => break,
            };
            self.ts.advance();
            let right = self.parse_addition();
            let span = left.span.merge(right.span);
            left = Expression {
                kind: ExpressionKind::BinaryOp {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            };
        }
        left
    }

    fn parse_addition(&mut self) -> Expression {
        let mut left = self.parse_multiplication();
        loop {
            let op = match self.ts.peek() {
                Some(Token::Plus) => BinaryOp::Add,
                Some(Token::Minus) => BinaryOp::Sub,
                _ => break,
            };
            self.ts.advance();
            let right = self.parse_multiplication();
            let span = left.span.merge(right.span);
            left = Expression {
                kind: ExpressionKind::BinaryOp {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            };
        }
        left
    }

    fn parse_multiplication(&mut self) -> Expression {
        let mut left = self.parse_power();
        loop {
            let op = match self.ts.peek() {
                Some(Token::Star) => BinaryOp::Mul,
                Some(Token::Slash) => BinaryOp::Div,
                Some(Token::Mod) => BinaryOp::Mod,
                _ => break,
            };
            self.ts.advance();
            let right = self.parse_power();
            let span = left.span.merge(right.span);
            left = Expression {
                kind: ExpressionKind::BinaryOp {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            };
        }
        left
    }

    fn parse_power(&mut self) -> Expression {
        let base = self.parse_unary();
        if self.ts.at(&Token::Power) {
            self.ts.advance();
            let exp = self.parse_unary(); // right-associative
            let span = base.span.merge(exp.span);
            Expression {
                kind: ExpressionKind::BinaryOp {
                    op: BinaryOp::Power,
                    left: Box::new(base),
                    right: Box::new(exp),
                },
                span,
            }
        } else {
            base
        }
    }

    fn parse_unary(&mut self) -> Expression {
        match self.ts.peek() {
            Some(Token::Not) => {
                let start = self.ts.advance().unwrap().1;
                let operand = self.parse_unary();
                let span = start.merge(operand.span);
                Expression {
                    kind: ExpressionKind::UnaryOp {
                        op: UnaryOp::Not,
                        operand: Box::new(operand),
                    },
                    span,
                }
            }
            Some(Token::Minus) => {
                let start = self.ts.advance().unwrap().1;
                let operand = self.parse_unary();
                let span = start.merge(operand.span);
                Expression {
                    kind: ExpressionKind::UnaryOp {
                        op: UnaryOp::Neg,
                        operand: Box::new(operand),
                    },
                    span,
                }
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Expression {
        let mut expr = self.parse_primary();

        loop {
            match self.ts.peek() {
                Some(Token::Dot) => {
                    self.ts.advance();
                    // Handle both identifier member access and bit access (e.g. IN.0)
                    if matches!(self.ts.peek(), Some(Token::IntegerLiteral(_))) {
                        let (Token::IntegerLiteral(v), int_span) = self.ts.advance().unwrap()
                        else {
                            unreachable!()
                        };
                        let member = Ident::new(v.to_string(), int_span);
                        let span = expr.span.merge(int_span);
                        expr = Expression {
                            kind: ExpressionKind::MemberAccess {
                                object: Box::new(expr),
                                member,
                            },
                            span,
                        };
                    } else {
                        let member = self.expect_ident();
                        let span = expr.span.merge(member.span);
                        expr = Expression {
                            kind: ExpressionKind::MemberAccess {
                                object: Box::new(expr),
                                member,
                            },
                            span,
                        };
                    }
                }
                Some(Token::LBracket) => {
                    self.ts.advance();
                    let mut indices = Vec::new();
                    loop {
                        indices.push(self.parse_expression());
                        if self.ts.eat(&Token::Comma).is_none() {
                            break;
                        }
                    }
                    let end = self
                        .ts
                        .expect(&Token::RBracket, &mut self.errors)
                        .unwrap_or(self.ts.peek_span());
                    let span = expr.span.merge(end);
                    expr = Expression {
                        kind: ExpressionKind::ArrayIndex {
                            array: Box::new(expr),
                            indices,
                        },
                        span,
                    };
                }
                Some(Token::LParen) => {
                    self.ts.advance();
                    let args = self.parse_call_args();
                    let end = self
                        .ts
                        .expect(&Token::RParen, &mut self.errors)
                        .unwrap_or(self.ts.peek_span());
                    let span = expr.span.merge(end);
                    expr = Expression {
                        kind: ExpressionKind::FunctionCall {
                            callee: Box::new(expr),
                            args,
                        },
                        span,
                    };
                }
                Some(Token::Caret) => {
                    let end = self.ts.advance().unwrap().1;
                    let span = expr.span.merge(end);
                    expr = Expression {
                        kind: ExpressionKind::Dereference(Box::new(expr)),
                        span,
                    };
                }
                _ => break,
            }
        }

        expr
    }

    fn parse_primary(&mut self) -> Expression {
        let _span = self.ts.peek_span();

        match self.ts.peek() {
            Some(Token::IntegerLiteral(_)) => {
                let (Token::IntegerLiteral(v), span) = self.ts.advance().unwrap() else {
                    unreachable!()
                };
                Expression {
                    kind: ExpressionKind::IntegerLiteral(v),
                    span,
                }
            }
            Some(Token::RealLiteral(_)) => {
                let (Token::RealLiteral(v), span) = self.ts.advance().unwrap() else {
                    unreachable!()
                };
                Expression {
                    kind: ExpressionKind::RealLiteral(v),
                    span,
                }
            }
            Some(Token::StringLiteral) => {
                let (_, span) = self.ts.advance().unwrap();
                let raw = self.ts.slice(self.source, &span);
                // Strip surrounding quotes
                let inner = &raw[1..raw.len() - 1];
                Expression {
                    kind: ExpressionKind::StringLiteral(inner.to_string()),
                    span,
                }
            }
            Some(Token::WstringLiteral) => {
                let (_, span) = self.ts.advance().unwrap();
                let raw = self.ts.slice(self.source, &span);
                let inner = &raw[1..raw.len() - 1];
                Expression {
                    kind: ExpressionKind::WstringLiteral(inner.to_string()),
                    span,
                }
            }
            Some(Token::True) => {
                let (_, span) = self.ts.advance().unwrap();
                Expression {
                    kind: ExpressionKind::BoolLiteral(true),
                    span,
                }
            }
            Some(Token::False) => {
                let (_, span) = self.ts.advance().unwrap();
                Expression {
                    kind: ExpressionKind::BoolLiteral(false),
                    span,
                }
            }
            Some(Token::TimeLiteral) => {
                let (_, span) = self.ts.advance().unwrap();
                let raw = self.ts.slice(self.source, &span).to_string();
                Expression {
                    kind: ExpressionKind::TimeLiteral(raw),
                    span,
                }
            }
            Some(Token::DateLiteral) => {
                let (_, span) = self.ts.advance().unwrap();
                let raw = self.ts.slice(self.source, &span).to_string();
                Expression {
                    kind: ExpressionKind::DateLiteral(raw),
                    span,
                }
            }
            Some(Token::TodLiteral) => {
                let (_, span) = self.ts.advance().unwrap();
                let raw = self.ts.slice(self.source, &span).to_string();
                Expression {
                    kind: ExpressionKind::TodLiteral(raw),
                    span,
                }
            }
            Some(Token::DtLiteral) => {
                let (_, span) = self.ts.advance().unwrap();
                let raw = self.ts.slice(self.source, &span).to_string();
                Expression {
                    kind: ExpressionKind::DtLiteral(raw),
                    span,
                }
            }
            Some(Token::DirectVariable) => {
                let (_, span) = self.ts.advance().unwrap();
                let raw = self.ts.slice(self.source, &span).to_string();
                Expression {
                    kind: ExpressionKind::DirectVariable(raw),
                    span,
                }
            }
            Some(Token::LParen) => {
                let start = self.ts.advance().unwrap().1;
                let inner = self.parse_expression();
                let end = self
                    .ts
                    .expect(&Token::RParen, &mut self.errors)
                    .unwrap_or(self.ts.peek_span());
                Expression {
                    kind: ExpressionKind::Parenthesized(Box::new(inner)),
                    span: start.merge(end),
                }
            }
            Some(t) if Self::is_ident_like(t) => {
                let ident = self.expect_ident();
                // Check for typed literal: TYPE#value
                if self.ts.at(&Token::Hash) {
                    self.ts.advance(); // consume #
                    let value = self.parse_primary();
                    let span = ident.span.merge(value.span);
                    Expression {
                        kind: ExpressionKind::TypedLiteral {
                            type_name: ident.clone(),
                            value: Box::new(value),
                        },
                        span,
                    }
                } else {
                    let span = ident.span;
                    Expression {
                        kind: ExpressionKind::Identifier(ident),
                        span,
                    }
                }
            }
            // Handle type keywords used as typed literals (INT#5, REAL#3.14, etc.)
            // (This is now largely covered by is_ident_like above, but keep for Hash handling)
            Some(t) if Self::is_type_keyword(t) && !Self::is_ident_like(t) => {
                let (tok, tok_span) = self.ts.advance().unwrap();
                let type_name = Ident::new(Self::type_keyword_name(&tok), tok_span);
                if self.ts.at(&Token::Hash) {
                    self.ts.advance(); // consume #
                    let value = self.parse_primary();
                    let span = tok_span.merge(value.span);
                    Expression {
                        kind: ExpressionKind::TypedLiteral {
                            type_name,
                            value: Box::new(value),
                        },
                        span,
                    }
                } else {
                    // Just a type name used as identifier
                    let span = type_name.span;
                    Expression {
                        kind: ExpressionKind::Identifier(type_name),
                        span,
                    }
                }
            }
            _ => {
                let span = self.ts.peek_span();
                let found = self
                    .ts
                    .peek()
                    .map(|t| format!("{t}"))
                    .unwrap_or("end of input".into());
                self.errors.push(ParseError::UnexpectedToken {
                    found,
                    expected: "expression".into(),
                    span: span.into(),
                });
                // Return a dummy expression for error recovery
                Expression {
                    kind: ExpressionKind::IntegerLiteral(0),
                    span,
                }
            }
        }
    }

    fn parse_call_args(&mut self) -> Vec<CallArg> {
        let mut args = Vec::new();
        if self.ts.at(&Token::RParen) {
            return args;
        }

        loop {
            let start = self.ts.peek_span();
            // Check for named argument: name := value or name => value
            let checkpoint = self.ts.pos;
            if matches!(self.ts.peek(), Some(Token::Identifier)) {
                let ident = self.expect_ident();
                if self.ts.eat(&Token::Assign).is_some() {
                    let value = self.parse_expression();
                    let span = start.merge(value.span);
                    args.push(CallArg {
                        name: Some(ident),
                        value,
                        is_output: false,
                        span,
                    });
                    if self.ts.eat(&Token::Comma).is_none() {
                        break;
                    }
                    continue;
                } else if self.ts.eat(&Token::OutputAssign).is_some() {
                    let value = self.parse_expression();
                    let span = start.merge(value.span);
                    args.push(CallArg {
                        name: Some(ident),
                        value,
                        is_output: true,
                        span,
                    });
                    if self.ts.eat(&Token::Comma).is_none() {
                        break;
                    }
                    continue;
                }
                // Not a named argument — backtrack
                self.ts.pos = checkpoint;
            }

            let value = self.parse_expression();
            let span = start.merge(value.span);
            args.push(CallArg {
                name: None,
                value,
                is_output: false,
                span,
            });
            if self.ts.eat(&Token::Comma).is_none() {
                break;
            }
        }

        args
    }

    fn is_type_keyword(token: &Token) -> bool {
        matches!(
            token,
            Token::Bool
                | Token::Byte
                | Token::Word
                | Token::Dword
                | Token::Lword
                | Token::Sint
                | Token::Int
                | Token::Dint
                | Token::Lint
                | Token::Usint
                | Token::Uint
                | Token::Udint
                | Token::Ulint
                | Token::Real
                | Token::Lreal
                | Token::Time
                | Token::Ltime
                | Token::Date
                | Token::Dt
                | Token::Tod
                | Token::Char
                | Token::Wchar
        )
    }

    fn type_keyword_name(token: &Token) -> &'static str {
        match token {
            Token::Bool => "BOOL",
            Token::Byte => "BYTE",
            Token::Word => "WORD",
            Token::Dword => "DWORD",
            Token::Lword => "LWORD",
            Token::Sint => "SINT",
            Token::Int => "INT",
            Token::Dint => "DINT",
            Token::Lint => "LINT",
            Token::Usint => "USINT",
            Token::Uint => "UINT",
            Token::Udint => "UDINT",
            Token::Ulint => "ULINT",
            Token::Real => "REAL",
            Token::Lreal => "LREAL",
            Token::Time => "TIME",
            Token::Ltime => "LTIME",
            Token::Date => "DATE",
            Token::Dt => "DT",
            Token::Tod => "TOD",
            Token::Char => "CHAR",
            Token::Wchar => "WCHAR",
            _ => "UNKNOWN",
        }
    }

    // ── Configuration ──

    fn parse_configuration(&mut self) -> ConfigurationDecl {
        let start = self.ts.advance().unwrap().1; // consume CONFIGURATION
        let name = self.expect_ident();

        let mut global_vars = Vec::new();
        let mut resources = Vec::new();

        loop {
            match self.ts.peek() {
                Some(Token::EndConfiguration) | None => break,
                Some(Token::VarGlobal) => {
                    global_vars.push(self.parse_var_block());
                }
                Some(Token::Resource) => {
                    resources.push(self.parse_resource());
                }
                _ => {
                    self.ts.advance();
                }
            }
        }

        let end = self
            .ts
            .expect(&Token::EndConfiguration, &mut self.errors)
            .unwrap_or(self.ts.peek_span());
        self.ts.eat(&Token::Semicolon);

        ConfigurationDecl {
            name,
            global_vars,
            resources,
            span: start.merge(end),
        }
    }

    fn parse_resource(&mut self) -> ResourceDecl {
        let start = self.ts.advance().unwrap().1; // consume RESOURCE
        let name = self.expect_ident();
        let on = if self.ts.eat(&Token::On).is_some() {
            Some(self.expect_ident())
        } else {
            None
        };

        let mut global_vars = Vec::new();
        let mut tasks = Vec::new();
        let mut program_configs = Vec::new();

        loop {
            match self.ts.peek() {
                Some(Token::EndResource) | None => break,
                Some(Token::VarGlobal) => {
                    global_vars.push(self.parse_var_block());
                }
                Some(Token::Task) => {
                    tasks.push(self.parse_task());
                }
                Some(Token::Program) => {
                    program_configs.push(self.parse_program_config());
                }
                _ => {
                    self.ts.advance();
                }
            }
        }

        let end = self
            .ts
            .expect(&Token::EndResource, &mut self.errors)
            .unwrap_or(self.ts.peek_span());
        self.ts.eat(&Token::Semicolon);

        ResourceDecl {
            name,
            on,
            global_vars,
            tasks,
            program_configs,
            span: start.merge(end),
        }
    }

    fn parse_task(&mut self) -> TaskDecl {
        let start = self.ts.advance().unwrap().1; // consume TASK
        let name = self.expect_ident();

        let mut properties = Vec::new();
        if self.ts.eat(&Token::LParen).is_some() {
            loop {
                let key = self.expect_ident();
                self.ts.expect(&Token::Assign, &mut self.errors);
                let value = self.parse_expression();
                properties.push((key, value));
                if self.ts.eat(&Token::Comma).is_none() {
                    break;
                }
            }
            self.ts.expect(&Token::RParen, &mut self.errors);
        }
        self.ts.eat(&Token::Semicolon);

        TaskDecl {
            name,
            properties,
            span: start,
        }
    }

    fn parse_program_config(&mut self) -> ProgramConfig {
        let start = self.ts.advance().unwrap().1; // consume PROGRAM
        let name = self.expect_ident();

        let task = if self.ts.eat(&Token::With).is_some() {
            Some(self.expect_ident())
        } else {
            None
        };

        self.ts.expect(&Token::Colon, &mut self.errors);
        let program_type = self.expect_ident();
        self.ts.eat(&Token::Semicolon);

        ProgramConfig {
            name,
            task,
            program_type,
            span: start,
        }
    }

    // ── Helpers ──

    /// Check if a token can be used as an identifier in non-structural contexts.
    /// Many IEC keywords are contextual and can appear as variable/type names.
    fn is_ident_like(token: &Token) -> bool {
        matches!(
            token,
            Token::Identifier
                | Token::On
                | Token::Override
                | Token::Abstract
                | Token::Final
                | Token::Public
                | Token::Private
                | Token::Protected
                | Token::Internal
                | Token::Extends
                | Token::Implements
                | Token::Reference
                | Token::Pointer
                | Token::Task
                | Token::Resource
                | Token::Configuration
                | Token::With
                | Token::REdge
                | Token::FEdge
                | Token::Ldt
                | Token::Ltod
                | Token::Ldate
                | Token::At
                | Token::Retain
                | Token::Constant
                | Token::Array
                | Token::Of
        ) || Self::is_type_keyword(token)
    }

    fn token_to_ident_name(&self, tok: &Token, span: &Span) -> String {
        if Self::is_type_keyword(tok) {
            Self::type_keyword_name(tok).to_string()
        } else {
            self.ts.slice(self.source, span).to_string()
        }
    }

    fn expect_ident(&mut self) -> Ident {
        match self.ts.peek() {
            Some(t) if Self::is_ident_like(t) => {
                let (tok, span) = self.ts.advance().unwrap();
                let name = self.token_to_ident_name(&tok, &span);
                Ident::new(name, span)
            }
            _ => {
                let span = self.ts.peek_span();
                let found = self
                    .ts
                    .peek()
                    .map(|t| format!("{t}"))
                    .unwrap_or("end of input".into());
                self.errors.push(ParseError::UnexpectedToken {
                    found,
                    expected: "identifier".into(),
                    span: span.into(),
                });
                Ident::new("<error>", span)
            }
        }
    }

    fn parse_ident_list(&mut self) -> Vec<Ident> {
        let mut list = vec![self.expect_ident()];
        while self.ts.eat(&Token::Comma).is_some() {
            list.push(self.expect_ident());
        }
        list
    }
}

/// Convenience function: parse source text into a compilation unit.
pub fn parse(source: &str) -> (CompilationUnit, Vec<ParseError>) {
    Parser::new(source).parse()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_program() {
        let src = r#"
PROGRAM main
VAR
    x : INT := 0;
    y : BOOL;
END_VAR
    x := x + 1;
    IF x > 10 THEN
        y := TRUE;
    END_IF;
END_PROGRAM
"#;
        let (unit, errors) = parse(src);
        assert!(errors.is_empty(), "errors: {errors:?}");
        assert_eq!(unit.declarations.len(), 1);
        match &unit.declarations[0] {
            Declaration::Program(p) => {
                assert_eq!(p.name.name, "main");
                assert_eq!(p.var_blocks.len(), 1);
                assert_eq!(p.var_blocks[0].declarations.len(), 2);
                assert_eq!(p.body.len(), 2);
            }
            _ => panic!("expected program"),
        }
    }

    #[test]
    fn parse_function_with_return_type() {
        let src = r#"
FUNCTION add : INT
VAR_INPUT
    a : INT;
    b : INT;
END_VAR
    add := a + b;
END_FUNCTION
"#;
        let (unit, errors) = parse(src);
        assert!(errors.is_empty(), "errors: {errors:?}");
        match &unit.declarations[0] {
            Declaration::Function(f) => {
                assert_eq!(f.name.name, "add");
                assert!(f.return_type.is_some());
                assert_eq!(f.var_blocks.len(), 1);
            }
            _ => panic!("expected function"),
        }
    }

    #[test]
    fn parse_function_block() {
        let src = r#"
FUNCTION_BLOCK Counter
VAR_INPUT
    reset : BOOL;
END_VAR
VAR_OUTPUT
    count : INT;
END_VAR
VAR
    cnt : INT;
END_VAR
    IF reset THEN
        cnt := 0;
    ELSE
        cnt := cnt + 1;
    END_IF;
    count := cnt;
END_FUNCTION_BLOCK
"#;
        let (unit, errors) = parse(src);
        assert!(errors.is_empty(), "errors: {errors:?}");
        match &unit.declarations[0] {
            Declaration::FunctionBlock(fb) => {
                assert_eq!(fb.name.name, "Counter");
                assert_eq!(fb.var_blocks.len(), 3);
            }
            _ => panic!("expected function block"),
        }
    }

    #[test]
    fn parse_for_while_repeat() {
        let src = r#"
PROGRAM loops
VAR
    i : INT;
    sum : INT;
END_VAR
    sum := 0;
    FOR i := 1 TO 10 BY 1 DO
        sum := sum + i;
    END_FOR;

    WHILE sum > 0 DO
        sum := sum - 1;
    END_WHILE;

    REPEAT
        sum := sum + 1;
    UNTIL sum >= 100
    END_REPEAT;
END_PROGRAM
"#;
        let (unit, errors) = parse(src);
        assert!(errors.is_empty(), "errors: {errors:?}");
    }

    #[test]
    fn parse_case_statement() {
        let src = r#"
PROGRAM state_machine
VAR
    state : INT := 0;
    output : BOOL;
END_VAR
    CASE state OF
        0:
            output := FALSE;
            state := 1;
        1, 2:
            output := TRUE;
            state := 0;
    ELSE
        state := 0;
    END_CASE;
END_PROGRAM
"#;
        let (unit, errors) = parse(src);
        assert!(errors.is_empty(), "errors: {errors:?}");
    }

    #[test]
    fn parse_array_and_struct() {
        let src = r#"
PROGRAM arrays
VAR
    arr : ARRAY[0..9] OF INT;
    matrix : ARRAY[0..2, 0..2] OF REAL;
END_VAR
    arr[0] := 42;
    matrix[1, 2] := 3.14;
END_PROGRAM
"#;
        let (unit, errors) = parse(src);
        assert!(errors.is_empty(), "errors: {errors:?}");
    }

    #[test]
    fn parse_typed_literal() {
        let src = r#"
PROGRAM typed
VAR
    x : INT;
    y : REAL;
END_VAR
    x := INT#42;
    y := REAL#3.14;
END_PROGRAM
"#;
        let (unit, errors) = parse(src);
        assert!(errors.is_empty(), "errors: {errors:?}");
    }
}
