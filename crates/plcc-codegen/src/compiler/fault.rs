// SPDX-License-Identifier: MPL-2.0

//! Runtime faults: `plcc_fault(code, where)`.
//!
//! CODESYS stops the task with an exception when an integer division has a zero
//! divisor; engineers expect a fault, not a silent value. A fault site branches
//! (cold) to
//!
//! ```llvm
//! call void @plcc_fault(i32 <code>, ptr @plcc.fault.site.N)   ; cold
//! call void @llvm.trap()                                     ; if the hook returned
//! unreachable
//! ```
//!
//! and the module carries a **weak** default `plcc_fault` that traps, so an object
//! still links on a host that defines nothing; a runtime overrides it with a strong
//! definition. The declaration is deliberately not `noreturn`: a hook that returns
//! must still reach the trap.
//!
//! `where` is a private constant string, `"<file>:<line>:<col>: <POU>"` when the
//! driver registered the POU's source file ([`Compiler::add_source_file`]),
//! otherwise `"<POU>"`.

use super::{CodegenError, Compiler};
use inkwell::AddressSpace;
use inkwell::attributes::{Attribute, AttributeLoc};
use inkwell::module::Linkage;
use inkwell::values::{FunctionValue, GlobalValue, IntValue};
use plcc_runtime::fault::{FAULT_SYMBOL, FaultCode};
use std::rc::Rc;

/// Line-start offsets of one registered source file.
pub(super) struct SourceFile {
    name: String,
    line_starts: Vec<usize>,
}

impl SourceFile {
    /// 1-based (line, column) of byte `offset`.
    fn line_col(&self, offset: usize) -> (usize, usize) {
        let line = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        (line + 1, offset - self.line_starts[line] + 1)
    }
}

/// Where fault sites are: the POU being compiled and the files POUs came from.
#[derive(Default)]
pub(super) struct FaultState<'ctx> {
    /// Label of the POU being compiled (`P`, `FB_X`, `FB_X.METHOD`).
    pub(super) site: String,
    /// Uppercase POU whose source the current body's spans point into (the
    /// declaring POU, for an inherited method).
    pub(super) source_pou: String,
    /// Span of the operation about to be lowered, set just before it is.
    pub(super) span: std::cell::Cell<Option<plcc_st::span::Span>>,
    /// Uppercase POU name → its source file.
    pub(super) files: std::collections::HashMap<String, Rc<SourceFile>>,
    /// Interned site strings.
    pub(super) strings: std::cell::RefCell<std::collections::HashMap<String, GlobalValue<'ctx>>>,
    /// Uppercase POUs in which an out-of-range array subscript faults
    /// (`ArrayBounds`) instead of being clamped.
    pub(super) bounds_pous: std::collections::HashSet<String>,
}

impl<'ctx> Compiler<'ctx> {
    /// Register the source file the given POUs (FUNCTION, FUNCTION_BLOCK, CLASS,
    /// PROGRAM names) were parsed from, so a runtime fault can name its line.
    /// Spans are byte offsets into `text`.
    pub fn add_source_file<I, S>(&mut self, file: &str, text: &str, pous: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut line_starts = vec![0];
        line_starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        let f = Rc::new(SourceFile {
            name: file.to_string(),
            line_starts,
        });
        for p in pous {
            self.fault.files.insert(p.as_ref().to_uppercase(), f.clone());
        }
    }

    /// Make an out-of-range array subscript in these POUs a runtime fault
    /// (`PLCC_FAULT_ARRAY_BOUNDS`) instead of clamping it into range. For
    /// dialects whose controllers fault there: a Logix 5000 controller raises
    /// major fault type 4, code 20 (1756-RM003 "Index Through Arrays").
    pub fn fault_on_array_bounds<I, S>(&mut self, pous: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.fault
            .bounds_pous
            .extend(pous.into_iter().map(|p| p.as_ref().to_uppercase()));
    }

    /// Whether the body being compiled faults on an out-of-range subscript.
    pub(super) fn faults_on_bounds(&self) -> bool {
        !self.fault.bounds_pous.is_empty() && self.fault.bounds_pous.contains(&self.fault.source_pou)
    }

    /// Remove the body of the weak default `plcc_fault`, leaving an external
    /// declaration. A JIT host calls this before creating the execution engine
    /// and then maps the symbol onto its own handler; a module that cannot fault
    /// is unchanged.
    pub fn use_external_fault_handler(&self) {
        if let Some(f) = self.module.get_function(FAULT_SYMBOL) {
            for bb in f.get_basic_blocks() {
                // Safety: nothing else refers to the default body's blocks.
                let _ = unsafe { bb.delete() };
            }
            f.set_linkage(Linkage::External);
        }
    }

    /// Set the POU fault sites are attributed to until the next call.
    pub(super) fn set_fault_site(&mut self, label: &str, source_pou: &str) {
        self.fault.site = label.to_string();
        self.fault.source_pou = source_pou.to_uppercase();
    }

    /// `plcc_fault`, defined weak (trap) on first use.
    fn fault_hook(&self) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function(FAULT_SYMBOL) {
            return f;
        }
        let ptr = self.context.ptr_type(AddressSpace::default());
        let ty = self
            .context
            .void_type()
            .fn_type(&[self.context.i32_type().into(), ptr.into()], false);
        let f = self.module.add_function(FAULT_SYMBOL, ty, Some(Linkage::WeakAny));
        let cold = self
            .context
            .create_enum_attribute(Attribute::get_named_enum_kind_id("cold"), 0);
        let noinline = self
            .context
            .create_enum_attribute(Attribute::get_named_enum_kind_id("noinline"), 0);
        f.add_attribute(AttributeLoc::Function, cold);
        f.add_attribute(AttributeLoc::Function, noinline);
        let b = self.context.create_builder();
        b.position_at_end(self.context.append_basic_block(f, "entry"));
        if let Some(trap) = inkwell::intrinsics::Intrinsic::find("llvm.trap")
            .and_then(|i| i.get_declaration(&self.module, &[]))
        {
            let _ = b.build_call(trap, &[], "");
        }
        let _ = b.build_unreachable();
        f
    }

    /// The constant string naming the current fault site.
    fn fault_site_string(&self) -> Result<GlobalValue<'ctx>, CodegenError> {
        let label = if self.fault.site.is_empty() {
            "<unknown>".to_string()
        } else {
            self.fault.site.clone()
        };
        let text = match (self.fault.files.get(&self.fault.source_pou), self.fault.span.take()) {
            (Some(f), Some(span)) if span.start > 0 || span.end > 0 => {
                let (line, col) = f.line_col(span.start);
                format!("{}:{line}:{col}: {label}", f.name)
            }
            _ => label,
        };
        if let Some(g) = self.fault.strings.borrow().get(&text) {
            return Ok(*g);
        }
        let n = self.fault.strings.borrow().len();
        let bytes = self.context.const_string(text.as_bytes(), true);
        let g = self
            .module
            .add_global(bytes.get_type(), None, &format!("plcc.fault.site.{n}"));
        g.set_initializer(&bytes);
        g.set_constant(true);
        g.set_linkage(Linkage::Private);
        g.set_unnamed_addr(true);
        self.fault.strings.borrow_mut().insert(text, g);
        Ok(g)
    }

    /// Branch to a fault when `cond` is true; the builder continues on the
    /// not-faulted path. Returns `false` (and emits nothing) when there is no
    /// function to branch in.
    pub(super) fn fault_if(&self, cond: IntValue<'ctx>, code: FaultCode) -> Result<bool, CodegenError> {
        let err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
        let Some(cur) = self.builder.get_insert_block() else {
            return Ok(false);
        };
        let Some(func) = cur.get_parent() else {
            return Ok(false);
        };
        let hook = self.fault_hook();
        let site = self.fault_site_string()?;
        let ok = self.context.insert_basic_block_after(cur, "no_fault");
        let fault = self.context.append_basic_block(func, "fault");
        self.builder.build_conditional_branch(cond, fault, ok).map_err(err)?;

        self.builder.position_at_end(fault);
        let call = self
            .builder
            .build_call(
                hook,
                &[
                    self.context.i32_type().const_int(code.code() as u64, false).into(),
                    site.as_pointer_value().into(),
                ],
                "",
            )
            .map_err(err)?;
        let cold = self
            .context
            .create_enum_attribute(Attribute::get_named_enum_kind_id("cold"), 0);
        call.add_attribute(AttributeLoc::Function, cold);
        if let Some(trap) = inkwell::intrinsics::Intrinsic::find("llvm.trap")
            .and_then(|i| i.get_declaration(&self.module, &[]))
        {
            self.builder.build_call(trap, &[], "").map_err(err)?;
        }
        self.builder.build_unreachable().map_err(err)?;

        self.builder.position_at_end(ok);
        Ok(true)
    }
}
