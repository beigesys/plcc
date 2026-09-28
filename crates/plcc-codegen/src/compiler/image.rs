// SPDX-License-Identifier: MPL-2.0

//! The process image (`%I`, `%Q`, `%M`) and `AT` bindings.
//!
//! Every directly represented variable *is* a location in one of three statically
//! allocated byte arrays the module exports — `plcc_image_i`, `plcc_image_q`,
//! `plcc_image_m`. An `AT` variable is bound to that location, not copied from it,
//! so `%IX0.0` and `%IB0` alias the same byte. Bit addresses are read with a
//! shift-and-mask and written with a read-modify-write of the containing byte.
//!
//! The arrays are sized from the highest address the program uses (or an explicit
//! override), which is only known once every body has been compiled. Until then
//! each area is an empty placeholder global; [`Compiler::finish_process_image`]
//! replaces it with the real, sized array.

use super::*;
use crate::direct_address::{self, AddrSize, Area, DirectAddress, ParsedAddress};
use inkwell::module::Linkage;

/// Alignment of each image array. Size-indexed (CODESYS) addressing puts every
/// `%xW`/`%xD`/`%xL` at a multiple of its own width, so with an 8-aligned base every
/// multi-byte access is naturally aligned — which matters on cores that fault on
/// unaligned loads (Cortex-M0, some RISC-V).
pub(crate) const IMAGE_ALIGN: u32 = 8;

/// One `AT` binding, as reported in the generated header and symbol table.
#[derive(Clone, Debug)]
pub(crate) struct AtBinding {
    /// `Main`, `MotorFb`, or `GLOBAL` for VAR_GLOBAL.
    pub scope: String,
    pub name: String,
    pub addr: DirectAddress,
    pub ty: IecType,
    /// Bytes the variable occupies in the image (1 for a bit).
    pub size: u64,
}

/// Compiler state for the process image and the runtime contract.
pub(crate) struct ContractState<'ctx> {
    /// Placeholder global per area (index by [`Area::index`]).
    pub(crate) placeholders: [Option<GlobalValue<'ctx>>; 3],
    /// One past the highest byte used, per area.
    pub(crate) used: [u64; 3],
    /// Size override per area (`--image-size`).
    pub(crate) size_override: [Option<u32>; 3],
    /// Final sizes, once [`Compiler::finish_process_image`] ran.
    pub(crate) sizes: [u32; 3],
    /// Bit-addressed variables currently bound in `variables`: name -> (byte, bit).
    /// Only valid while `variables[name]` still holds the same byte pointer.
    pub(crate) bit_vars: HashMap<String, (PointerValue<'ctx>, u32)>,
    /// VAR_GLOBAL AT bindings: (uppercased name, address, type, initializer).
    pub(crate) global_at: Vec<(String, DirectAddress, IecType, Option<Expression>)>,
    /// Every AT binding in the unit, for the header and symbol table.
    pub(crate) bindings: Vec<AtBinding>,
    /// Interval of the implicit task used when there is no CONFIGURATION.
    pub(crate) default_task_interval_ns: i64,
    /// Everything the header / symbol emitters need, filled in by
    /// `finish_runtime_contract`.
    pub(super) layout: Option<super::contract::ContractLayout<'ctx>>,
}

impl Default for ContractState<'_> {
    fn default() -> Self {
        Self {
            placeholders: [None, None, None],
            used: [0; 3],
            size_override: [None; 3],
            sizes: [0; 3],
            bit_vars: HashMap::new(),
            global_at: Vec::new(),
            bindings: Vec::new(),
            default_task_interval_ns: super::contract::DEFAULT_TASK_INTERVAL_NS,
            layout: None,
        }
    }
}

/// Name of the generated function that applies VAR_GLOBAL AT initializers.
pub(crate) const IMAGE_INIT_FN: &str = "plcc_image_init";

impl CodegenError {
    /// The source span this error points at, when it has one.
    pub fn span(&self) -> Option<plcc_st::span::Span> {
        match self {
            CodegenError::Located { span, .. } => Some(*span),
            _ => None,
        }
    }
}

fn located(message: impl Into<String>, span: plcc_st::span::Span) -> CodegenError {
    CodegenError::Located {
        message: message.into(),
        span,
    }
}

fn llvm(e: impl std::fmt::Display) -> CodegenError {
    CodegenError::LlvmError(e.to_string())
}

impl<'ctx> Compiler<'ctx> {
    /// Size the process image area explicitly (bytes). Must be at least what the
    /// program's addresses need; checked when compilation finishes.
    pub fn set_image_size(&mut self, area: Area, bytes: u32) {
        self.rt.size_override[area.index()] = Some(bytes);
    }

    /// Final image sizes (I, Q, M) in bytes. Valid after `compile`.
    pub fn image_sizes(&self) -> [u32; 3] {
        self.rt.sizes
    }

    /// Parse a direct address, reporting a failure against `span`.
    pub(crate) fn parse_located_address(
        repr: &str,
        span: plcc_st::span::Span,
    ) -> Result<DirectAddress, CodegenError> {
        match direct_address::parse(repr).map_err(|m| located(m, span))? {
            ParsedAddress::Located(a) => Ok(a),
            ParsedAddress::Partial { .. } => Err(located(
                format!(
                    "`{repr}` is a partially specified address; completing it with \
                     VAR_CONFIG is not yet supported — give the variable a full address \
                     (e.g. `AT %IX0.0`)"
                ),
                span,
            )),
        }
    }

    /// Bit width of an elementary type as stored, or `None` for anything that is not
    /// a plain scalar.
    fn scalar_bits(&self, ty: &IecType) -> Option<u32> {
        match ty {
            IecType::Array { .. }
            | IecType::Struct { .. }
            | IecType::StringType { .. }
            | IecType::WstringType { .. }
            | IecType::Pointer(_)
            | IecType::FbInstance(_)
            | IecType::Unresolved(_)
            | IecType::Void => None,
            IecType::Alias { base_type, .. } => self.scalar_bits(base_type),
            _ => match self.iec_to_llvm_type(ty) {
                BasicTypeEnum::IntType(t) => Some(t.get_bit_width()),
                BasicTypeEnum::FloatType(t) => {
                    if t == self.context.f64_type() {
                        Some(64)
                    } else {
                        Some(32)
                    }
                }
                _ => None,
            },
        }
    }

    /// Target-independent (natural, C-like) size and alignment. Used only to size
    /// the image and check alignment of aggregates placed in it; the exact target
    /// layout is taken from LLVM when the header is generated.
    pub(crate) fn natural_size_align(&self, ty: &IecType) -> Option<(u64, u64)> {
        match ty {
            IecType::StringType { max_len } => Some((max_len.unwrap_or(256) as u64 + 1, 1)),
            IecType::WstringType { max_len } => {
                Some((2 * (max_len.unwrap_or(256) as u64 + 1), 2))
            }
            IecType::Array {
                ranges,
                element_type,
            } => {
                let (s, a) = self.natural_size_align(element_type)?;
                let n: u64 = ranges
                    .iter()
                    .map(|(lo, hi)| (hi - lo + 1).max(0) as u64)
                    .product();
                Some((s * n, a))
            }
            IecType::Struct { fields, .. } => {
                let (mut off, mut align) = (0u64, 1u64);
                for (_, t) in fields {
                    let (s, a) = self.natural_size_align(t)?;
                    off = off.div_ceil(a) * a + s;
                    align = align.max(a);
                }
                Some((off.div_ceil(align) * align, align))
            }
            IecType::Bool => Some((1, 1)),
            _ => {
                let bits = self.scalar_bits(ty)?;
                let b = (bits / 8).max(1) as u64;
                Some((b, b))
            }
        }
    }

    /// Check that `ty` may live at `addr`, returning the bytes it occupies.
    fn check_at_type(
        &self,
        name: &str,
        addr: &DirectAddress,
        ty: &IecType,
        span: plcc_st::span::Span,
    ) -> Result<u64, CodegenError> {
        let is_bool = matches!(ty, IecType::Bool);
        if addr.size == AddrSize::Bit {
            if is_bool {
                return Ok(1);
            }
            return Err(located(
                format!(
                    "`{name}` is {ty}, but {addr} is a single bit; only BOOL can be \
                     placed at an X (bit) address"
                ),
                span,
            ));
        }
        if is_bool {
            return Err(located(
                format!(
                    "`{name}` is BOOL, but {addr} is {}-bit; a BOOL needs a bit address \
                     such as `%{}X{}.0`",
                    addr.size.bits(),
                    addr.area.letter(),
                    addr.byte
                ),
                span,
            ));
        }
        if let Some(bits) = self.scalar_bits(ty) {
            if bits != addr.size.bits() {
                return Err(located(
                    format!(
                        "`{name}` is {ty} ({bits} bits), but {addr} is a {}-bit location \
                         (use %{}{} for {bits}-bit types)",
                        addr.size.bits(),
                        addr.area.letter(),
                        match bits {
                            8 => "B",
                            16 => "W",
                            32 => "D",
                            _ => "L",
                        }
                    ),
                    span,
                ));
            }
            return Ok(addr.size.bytes() as u64);
        }
        match ty {
            IecType::Array { .. }
            | IecType::Struct { .. }
            | IecType::StringType { .. }
            | IecType::WstringType { .. } => {
                let Some((size, align)) = self.natural_size_align(ty) else {
                    return Err(located(
                        format!("`{name}` ({ty}) cannot be placed in the process image"),
                        span,
                    ));
                };
                if addr.byte as u64 % align != 0 {
                    return Err(located(
                        format!(
                            "`{name}` ({ty}) needs {align}-byte alignment, but {addr} is at \
                             byte offset {}",
                            addr.byte
                        ),
                        span,
                    ));
                }
                Ok(size)
            }
            _ => Err(located(
                format!(
                    "`{name}` is {ty}, which cannot be placed at a direct address \
                     (function block instances and pointers have no process-image form)"
                ),
                span,
            )),
        }
    }

    /// Validate every `AT` declaration in the unit and record the bindings.
    ///
    /// Runs before any body is compiled, so a bad address is reported once, against
    /// its declaration, and never half-way through codegen.
    pub(crate) fn plan_process_image(
        &mut self,
        unit: &CompilationUnit,
    ) -> Result<(), CodegenError> {
        let mut pous: Vec<(&'static str, &str, &[VarBlock])> = Vec::new();
        for decl in &unit.declarations {
            match decl {
                Declaration::Program(p) => pous.push(("PROGRAM", &p.name.name, &p.var_blocks)),
                Declaration::FunctionBlock(f) => {
                    pous.push(("FUNCTION_BLOCK", &f.name.name, &f.var_blocks))
                }
                Declaration::Class(c) => pous.push(("CLASS", &c.name.name, &c.var_blocks)),
                Declaration::GlobalVarDecl(b) => {
                    pous.push(("VAR_GLOBAL", "GLOBAL", std::slice::from_ref(b)))
                }
                Declaration::Function(f) => {
                    Self::reject_at_in(&f.var_blocks, "a FUNCTION")?;
                }
                _ => {}
            }
            let methods: &[MethodDecl] = match decl {
                Declaration::FunctionBlock(f) => &f.methods,
                Declaration::Class(c) => &c.methods,
                _ => &[],
            };
            for m in methods {
                Self::reject_at_in(&m.var_blocks, "a METHOD")?;
            }
        }

        for (kind, scope, blocks) in pous {
            for block in blocks {
                let mut prev_span = None;
                for decl in &block.declarations {
                    let Some(at) = &decl.at_address else {
                        continue;
                    };
                    if prev_span == Some(at.span) {
                        return Err(located(
                            format!(
                                "`{}` shares an AT address with the previous name in the \
                                 same declaration; AT takes exactly one variable",
                                decl.name.name
                            ),
                            at.span,
                        ));
                    }
                    prev_span = Some(at.span);
                    let allowed = matches!(block.kind, VarBlockKind::Var | VarBlockKind::VarGlobal);
                    if !allowed {
                        return Err(located(
                            format!(
                                "AT is supported on VAR and VAR_GLOBAL variables; `{}` is in \
                                 a {:?} block of {kind} `{scope}`",
                                decl.name.name, block.kind
                            ),
                            at.span,
                        ));
                    }
                    let addr = Self::parse_located_address(&at.repr, at.span)?;
                    let ty = self.resolve_type_spec(&decl.type_spec);
                    let size = self.check_at_type(&decl.name.name, &addr, &ty, decl.span)?;
                    let end = addr.byte as u64 + size;
                    let slot = &mut self.rt.used[addr.area.index()];
                    *slot = (*slot).max(end);
                    if block.kind == VarBlockKind::VarGlobal {
                        self.rt.global_at.push((
                            decl.name.name.to_uppercase(),
                            addr,
                            ty.clone(),
                            decl.initializer.clone(),
                        ));
                    }
                    self.rt.bindings.push(AtBinding {
                        scope: scope.to_string(),
                        name: decl.name.name.clone(),
                        addr,
                        ty,
                        size,
                    });
                }
            }
        }

        // Declared now so every POU's `_init` can call it; the body is emitted once
        // every other function exists (`finish_process_image`).
        let fn_type = self.context.void_type().fn_type(&[], false);
        if self.module.get_function(IMAGE_INIT_FN).is_none() {
            self.module.add_function(IMAGE_INIT_FN, fn_type, None);
        }
        Ok(())
    }

    fn reject_at_in(blocks: &[VarBlock], what: &str) -> Result<(), CodegenError> {
        for block in blocks {
            for decl in &block.declarations {
                if let Some(at) = &decl.at_address {
                    return Err(located(
                        format!(
                            "`{}` has an AT address inside {what}; direct addresses are \
                             supported in PROGRAM, FUNCTION_BLOCK and VAR_GLOBAL variables",
                            decl.name.name
                        ),
                        at.span,
                    ));
                }
            }
        }
        Ok(())
    }

    /// The (placeholder) global of an area.
    fn image_global(&mut self, area: Area) -> GlobalValue<'ctx> {
        if let Some(g) = self.rt.placeholders[area.index()] {
            return g;
        }
        let ty = self.context.i8_type().array_type(0);
        let g = self
            .module
            .add_global(ty, None, &format!("{}.pending", area.symbol()));
        g.set_linkage(Linkage::External);
        self.rt.placeholders[area.index()] = Some(g);
        g
    }

    /// Pointer to the first byte of `addr`.
    pub(crate) fn image_ptr(&mut self, addr: &DirectAddress) -> PointerValue<'ctx> {
        let slot = &mut self.rt.used[addr.area.index()];
        *slot = (*slot).max(addr.end());
        let base = self.image_global(addr.area).as_pointer_value();
        let offset = self.context.i32_type().const_int(addr.byte as u64, false);
        unsafe { base.const_in_bounds_gep(self.context.i8_type(), &[offset]) }
    }

    /// Bind `name` to its image location in `variables`.
    pub(crate) fn bind_at(&mut self, name: &str, addr: &DirectAddress, ty: &IecType) {
        let ptr = self.image_ptr(addr);
        let key = name.to_uppercase();
        if let Some(bit) = addr.bit {
            self.rt.bit_vars.insert(key.clone(), (ptr, bit as u32));
        }
        self.variables.insert(key, (ptr, ty.clone()));
    }

    /// `(byte pointer, bit)` if `name` is currently bound to a bit address.
    pub(crate) fn bit_binding(&self, name_upper: &str) -> Option<(PointerValue<'ctx>, u32)> {
        let (ptr, bit) = *self.rt.bit_vars.get(name_upper)?;
        match self.variables.get(name_upper) {
            Some((p, IecType::Bool)) if *p == ptr => Some((ptr, bit)),
            _ => None,
        }
    }

    /// Load bit `bit` of the byte at `ptr` as a BOOL (i8 0/1).
    pub(crate) fn load_bit(
        &mut self,
        ptr: PointerValue<'ctx>,
        bit: u32,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let i8t = self.context.i8_type();
        let byte = self
            .builder
            .build_load(i8t, ptr, "img_byte")
            .map_err(llvm)?
            .into_int_value();
        let shifted = self
            .builder
            .build_right_shift(byte, i8t.const_int(bit as u64, false), false, "img_shr")
            .map_err(llvm)?;
        let v = self
            .builder
            .build_and(shifted, i8t.const_int(1, false), "img_bit")
            .map_err(llvm)?;
        Ok(v.into())
    }

    /// Store a BOOL value into bit `bit` of the byte at `ptr` (read-modify-write).
    pub(crate) fn store_bit(
        &mut self,
        ptr: PointerValue<'ctx>,
        bit: u32,
        val: BasicValueEnum<'ctx>,
        src: Option<&IecType>,
    ) -> Result<(), CodegenError> {
        let i8t = self.context.i8_type();
        let val = self.coerce_value(val, src, &IecType::Bool)?;
        let BasicValueEnum::IntValue(iv) = val else {
            return Err(CodegenError::UnsupportedType(
                "a bit address can only be assigned a BOOL".into(),
            ));
        };
        let is_set = self
            .builder
            .build_int_compare(IntPredicate::NE, iv, iv.get_type().const_zero(), "bit_set")
            .map_err(llvm)?;
        let as_byte = self
            .builder
            .build_int_z_extend(is_set, i8t, "bit_byte")
            .map_err(llvm)?;
        let positioned = self
            .builder
            .build_left_shift(as_byte, i8t.const_int(bit as u64, false), "bit_pos")
            .map_err(llvm)?;
        let old = self
            .builder
            .build_load(i8t, ptr, "img_old")
            .map_err(llvm)?
            .into_int_value();
        let mask = i8t.const_int(!(1u64 << bit) & 0xFF, false);
        let cleared = self.builder.build_and(old, mask, "img_clr").map_err(llvm)?;
        let new = self
            .builder
            .build_or(cleared, positioned, "img_new")
            .map_err(llvm)?;
        self.builder.build_store(ptr, new).map_err(llvm)?;
        Ok(())
    }

    /// The IEC type a bare direct address denotes: BOOL, BYTE, WORD, DWORD, LWORD.
    pub(crate) fn direct_variable_type(repr: &str) -> Option<IecType> {
        match direct_address::parse(repr).ok()? {
            ParsedAddress::Located(a) => Some(match a.size {
                AddrSize::Bit => IecType::Bool,
                AddrSize::Byte => IecType::Byte,
                AddrSize::Word => IecType::Word,
                AddrSize::Dword => IecType::Dword,
                AddrSize::Lword => IecType::Lword,
            }),
            ParsedAddress::Partial { .. } => None,
        }
    }

    /// `%IX0.3` / `%IW2` used as a value.
    pub(crate) fn compile_direct_variable_load(
        &mut self,
        repr: &str,
        expr: &Expression,
    ) -> Result<Option<BasicValueEnum<'ctx>>, CodegenError> {
        let addr = Self::parse_located_address(repr, expr.span)?;
        let ptr = self.image_ptr(&addr);
        if let Some(bit) = addr.bit {
            return self.load_bit(ptr, bit as u32).map(Some);
        }
        let ty = Self::direct_variable_type(repr).unwrap_or(IecType::Byte);
        let llvm_ty = self.iec_to_llvm_type(&ty);
        let v = self.builder.build_load(llvm_ty, ptr, "img_val").map_err(llvm)?;
        Ok(Some(v))
    }

    /// Assignments whose target lives in the process image and cannot go through
    /// the ordinary pointer store: a bit-addressed variable, or a direct address
    /// written in the statement itself (`%QX0.1 := TRUE;`). Returns `false` when the
    /// target is anything else.
    pub(crate) fn try_compile_image_assignment(
        &mut self,
        target: &Expression,
        value: &Expression,
        function: FunctionValue<'ctx>,
    ) -> Result<bool, CodegenError> {
        let (ptr, bit, ty) = match &target.kind {
            ExpressionKind::Identifier(ident) => {
                match self.bit_binding(&ident.name.to_uppercase()) {
                    Some((ptr, bit)) => (ptr, Some(bit), IecType::Bool),
                    None => return Ok(false),
                }
            }
            ExpressionKind::DirectVariable(repr) => {
                let addr = Self::parse_located_address(repr, target.span)?;
                let ptr = self.image_ptr(&addr);
                let ty = Self::direct_variable_type(repr).unwrap_or(IecType::Byte);
                (ptr, addr.bit.map(u32::from), ty)
            }
            ExpressionKind::Parenthesized(inner) => {
                return self.try_compile_image_assignment(inner, value, function);
            }
            _ => return Ok(false),
        };
        let Some(val) = self.compile_expression(value, function)? else {
            return Err(self.no_value_error(
                format!(
                    "right-hand side of the assignment to `{}`",
                    Self::describe_lvalue(target)
                ),
                value,
            ));
        };
        let src = self.rvalue_iec_type(value);
        match bit {
            Some(bit) => self.store_bit(ptr, bit, val, src.as_ref())?,
            None => {
                let val = self.coerce_value(val, src.as_ref(), &ty)?;
                self.builder.build_store(ptr, val).map_err(llvm)?;
            }
        }
        Ok(true)
    }

    /// The error for taking the address of a bit-addressed variable.
    pub(crate) fn bit_address_error(&self, name: &str) -> CodegenError {
        CodegenError::UnsupportedType(format!(
            "`{name}` is bound to a single bit of the process image and has no byte \
             address of its own, so it cannot be passed by reference, used with ADR, \
             or reached this way; copy it into a BOOL variable first"
        ))
    }

    /// Store `init` into an AT variable at `addr` (used by `_init` bodies and
    /// `plcc_image_init`).
    pub(crate) fn emit_at_initializer(
        &mut self,
        addr: &DirectAddress,
        ty: &IecType,
        init: &Expression,
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        let ptr = self.image_ptr(addr);
        match addr.bit {
            Some(bit) => {
                let Some(val) = self.compile_expression(init, function)? else {
                    return Err(self.no_value_error("AT variable initializer", init));
                };
                let src = self.rvalue_iec_type(init);
                self.store_bit(ptr, bit as u32, val, src.as_ref())
            }
            None => self.emit_decl_initializer(ptr, ty, init, function),
        }
    }

    /// Emit the body of `plcc_image_init` (VAR_GLOBAL AT initializers) and replace
    /// each placeholder area with its final, sized, zero-initialized array.
    pub(crate) fn finish_process_image(&mut self) -> Result<(), CodegenError> {
        if let Some(f) = self.module.get_function(IMAGE_INIT_FN)
            && f.count_basic_blocks() == 0
        {
            let entry = self.context.append_basic_block(f, "entry");
            self.builder.position_at_end(entry);
            self.variables.clear();
            self.add_globals_to_variables()?;
            for (_, addr, ty, init) in self.rt.global_at.clone() {
                if let Some(init) = init {
                    self.emit_at_initializer(&addr, &ty, &init, f)?;
                }
            }
            self.builder.build_return(None).map_err(llvm)?;
        }

        for area in Area::ALL {
            let i = area.index();
            let used = self.rt.used[i];
            let size = match self.rt.size_override[i] {
                Some(o) if (o as u64) < used => {
                    return Err(CodegenError::TargetError(format!(
                        "--image-size {}={o} is too small: the program uses {used} bytes of \
                         %{}",
                        area.letter(),
                        area.letter()
                    )));
                }
                Some(o) => o,
                None => u32::try_from(used).map_err(|_| {
                    CodegenError::TargetError(format!("%{} image too large", area.letter()))
                })?,
            };
            self.rt.sizes[i] = size;
            let ty = self.context.i8_type().array_type(size);
            let g = self.module.add_global(ty, None, area.symbol());
            g.set_initializer(&ty.const_zero());
            g.set_alignment(IMAGE_ALIGN);
            if let Some(old) = self.rt.placeholders[i].take() {
                old.as_pointer_value()
                    .replace_all_uses_with(g.as_pointer_value());
                unsafe { old.delete() };
            }
        }
        Ok(())
    }
}
