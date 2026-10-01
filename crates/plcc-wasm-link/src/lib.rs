// SPDX-License-Identifier: MPL-2.0
//! A single-object WebAssembly linker for plcc's wasm32 output.
//!
//! The browser has no `wasm-ld`. [`link`] does, for exactly one relocatable
//! object (`plcc compile --target wasm32-unknown-unknown`), what
//!
//! ```text
//! wasm-ld --no-entry --export-dynamic --allow-undefined --export-table -o prog.wasm prog.o
//! ```
//!
//! does, producing a module that `@plcc/plc-wasm` runs (docs/studio-wasm.md).
//!
//! # What it does
//!
//! - **Imports.** Undefined function symbols stay imports, under the module and
//!   field names the object gives them (`env.plcc_monotonic_ns`, `env.sinf`, ...),
//!   as `--allow-undefined` leaves them. The object's `env.__linear_memory`,
//!   `env.__indirect_function_table` and `env.__stack_pointer` imports become a
//!   defined memory, a defined table and a defined mutable `i32` global.
//! - **Memory layout** (wasm-ld's default, no `--stack-first`): data segments
//!   from [`Options::global_base`] (1024) in object order, each at its
//!   alignment from the `linking` segment info; then the stack
//!   ([`Options::stack_size`], 16-byte aligned) with `__stack_pointer` at its
//!   top; `__heap_base` after it. The memory's initial size covers all of it,
//!   with no maximum. Segments that are all zeros after relocation (`.bss`)
//!   are not emitted: a defined memory starts zeroed.
//! - **Function table.** Every function whose table index is relocated
//!   (`R_WASM_TABLE_INDEX_*`) gets one slot, from 1 in first-use order (slot 0
//!   stays null), filled by one active element segment. The table's size is
//!   fixed (minimum = maximum), as wasm-ld makes it.
//! - **Relocations** in the code and data sections: `FUNCTION_INDEX_LEB`,
//!   `TABLE_INDEX_SLEB`/`_I32`, `MEMORY_ADDR_LEB`/`_SLEB`/`_I32`,
//!   `TYPE_INDEX_LEB`, `GLOBAL_INDEX_LEB`/`_I32`, `TABLE_NUMBER_LEB`. Function,
//!   type and global indices keep their object numbering (every function import
//!   is kept and the stack pointer stays global 0), so code is patched in place.
//! - **Exports** (`--export-dynamic`, `--export-table`): `memory`; every
//!   defined function symbol that is neither local nor hidden (or carries the
//!   `EXPORTED` flag), under its symbol name; every such data symbol as an
//!   immutable `i32` global holding its address (`plcc_image_q`,
//!   `plcc_inst_*`, ...); and `__indirect_function_table`, at the position of
//!   the object's table symbol (else last). Same names, kinds and order as
//!   wasm-ld. Linker-synthesized symbols (`__stack_pointer`, `__heap_base`,
//!   `__data_end`) are hidden in wasm-ld and are not exported here either.
//! - Custom sections `target_features` and `producers` are copied; a `name`
//!   section with function and global names is added if
//!   [`Options::name_section`] is set.
//!
//! # What it rejects (a [`LinkError`] naming the thing)
//!
//! Anything plcc does not emit: init functions
//! (`llvm.global_ctors` — wasm-ld would wrap every export in a constructor
//! call), TLS or passive segments, undefined data symbols, imports other
//! than functions and the three above, objects that define their own tables,
//! memories, globals, tags, exports or a start function, a `linking` section
//! other than version 2, wasm64 and PIC relocations, and any relocation type
//! not listed above. Debug sections (`.debug_*`) and their relocations are
//! dropped rather than rejected.
//!
//! # What wasm-ld does that this skips
//!
//! No garbage collection of unreferenced local functions or data (with
//! `--export-dynamic` almost everything is a root anyway), no string
//! merging of `STRINGS` segments, and no reordering of segments into
//! `.rodata`/`.data`/`.bss` groups. Data addresses therefore differ from
//! wasm-ld's; the exported address globals describe the layout. The type,
//! function and code sections are copied as they are (wasm-ld renumbers
//! types by first use and drops unreferenced imports), and no `data count`
//! section is written (there are no passive segments).

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use wasm_encoder as enc;
use wasmparser as wp;
use wp::{Linking, Payload, RelocationType as R, SymbolFlags, SymbolInfo, TypeRef};

/// Link options. [`Options::default`] matches wasm-ld's defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Address of the first data segment (`--global-base`, default 1024).
    pub global_base: u32,
    /// Stack size in bytes, a multiple of 16 (`-z stack-size`, default 64 KiB).
    pub stack_size: u32,
    /// Emit a `name` section (function and global names, for stack traces).
    pub name_section: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            global_base: 1024,
            stack_size: 64 * 1024,
            name_section: true,
        }
    }
}

/// Why an object could not be linked.
#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    /// The input is not a well-formed wasm object.
    #[error("malformed wasm object: {0}")]
    Malformed(#[from] wp::BinaryReaderError),
    /// The object uses something this linker does not support.
    #[error("unsupported in a wasm object: {0}")]
    Unsupported(String),
    /// A data symbol is referenced but not defined.
    #[error("undefined data symbol `{0}`")]
    UndefinedData(String),
    /// The object is inconsistent (bad index, offset out of range, ...).
    #[error("invalid wasm object: {0}")]
    Invalid(String),
}

fn unsupported(what: impl Into<String>) -> LinkError {
    LinkError::Unsupported(what.into())
}

fn invalid(what: impl Into<String>) -> LinkError {
    LinkError::Invalid(what.into())
}

const WASM_PAGE: u64 = 65536;
/// Adjacent non-zero segments closer than this are merged into one.
const MERGE_GAP: u32 = 16;

#[derive(Debug, Clone, Copy)]
enum SymKind {
    Func(u32),
    Data(Option<wp::DefinedDataSymbol>),
    Global(u32),
    Table(u32),
    Other,
}

#[derive(Debug, Clone, Copy)]
struct Symbol<'a> {
    kind: SymKind,
    flags: SymbolFlags,
    name: &'a str,
}

impl Symbol<'_> {
    fn defined(&self) -> bool {
        !self.flags.contains(SymbolFlags::UNDEFINED)
    }

    /// wasm-ld's `Symbol::isExported` under `--export-dynamic`.
    fn exported(&self) -> bool {
        self.defined()
            && !self.flags.contains(SymbolFlags::BINDING_LOCAL)
            && (!self.flags.contains(SymbolFlags::VISIBILITY_HIDDEN)
                || self.flags.contains(SymbolFlags::EXPORTED))
    }
}

struct DataSegment<'a> {
    /// Absolute byte range of the segment's contents in the object.
    bytes: Range<usize>,
    name: &'a str,
    align_log2: u32,
}

/// The parts of a relocatable object the linker uses.
#[derive(Default)]
struct Object<'a> {
    /// Section id and absolute payload range, by section ordinal (what
    /// `reloc.*` sections refer to).
    sections: Vec<(u8, Range<usize>)>,
    types: Option<Range<usize>>,
    functions: Option<Range<usize>>,
    defined_funcs: u32,
    code: Option<Range<usize>>,
    func_imports: Vec<(&'a str, &'a str, u32)>,
    imports_stack_pointer: bool,
    segments: Vec<DataSegment<'a>>,
    symbols: Vec<Symbol<'a>>,
    relocs: Vec<(u32, Vec<wp::RelocationEntry>)>,
    customs: Vec<(&'a str, &'a [u8])>,
}

fn range(r: Range<u64>) -> Range<usize> {
    r.start as usize..r.end as usize
}

impl<'a> Object<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self, LinkError> {
        let mut obj = Object::default();
        let mut seg_info: Vec<wp::Segment<'a>> = Vec::new();
        let mut raw_symbols: Vec<SymbolInfo<'a>> = Vec::new();
        for payload in wp::Parser::new(0).parse_all(bytes) {
            let payload = payload?;
            if let Some((id, r)) = payload.as_section() {
                obj.sections.push((id, range(r)));
            }
            match payload {
                Payload::Version {
                    encoding: wp::Encoding::Module,
                    ..
                } => {}
                Payload::Version { .. } => {
                    return Err(unsupported("a component (expected a core module)"));
                }
                Payload::TypeSection(r) => obj.types = Some(range(r.range())),
                Payload::ImportSection(r) => {
                    for imp in r.into_imports() {
                        obj.import(imp?)?;
                    }
                }
                Payload::FunctionSection(r) => {
                    obj.defined_funcs = r.count();
                    obj.functions = Some(range(r.range()));
                }
                Payload::TableSection(_) => return Err(unsupported("a defined table")),
                Payload::MemorySection(_) => return Err(unsupported("a defined memory")),
                Payload::GlobalSection(_) => return Err(unsupported("defined wasm globals")),
                Payload::TagSection(_) => return Err(unsupported("exception tags")),
                Payload::ExportSection(_) => return Err(unsupported("an export section")),
                Payload::StartSection { .. } => return Err(unsupported("a start function")),
                // Rebuilt from the TABLE_INDEX relocations, as wasm-ld does.
                Payload::ElementSection(_) | Payload::DataCountSection { .. } => {}
                Payload::CodeSectionStart { range: r, .. } => obj.code = Some(range(r)),
                Payload::CodeSectionEntry(_) => {}
                Payload::DataSection(r) => {
                    for data in r {
                        let data = data?;
                        match data.kind {
                            wp::DataKind::Active {
                                memory_index: 0, ..
                            } => {}
                            _ => return Err(unsupported("a passive or multi-memory data segment")),
                        }
                        let end = data.range.end as usize;
                        obj.segments.push(DataSegment {
                            bytes: end - data.data.len()..end,
                            name: "",
                            align_log2: 0,
                        });
                    }
                }
                Payload::CustomSection(c) => match c.as_known() {
                    wp::KnownCustom::Linking(l) => {
                        if l.version() != 2 {
                            return Err(unsupported(format!(
                                "linking section version {}",
                                l.version()
                            )));
                        }
                        for sub in l.subsections() {
                            match sub? {
                                Linking::SymbolTable(m) => {
                                    raw_symbols = m.into_iter().collect::<Result<_, _>>()?
                                }
                                Linking::SegmentInfo(m) => {
                                    seg_info = m.into_iter().collect::<Result<_, _>>()?
                                }
                                Linking::InitFuncs(m) if m.count() > 0 => {
                                    return Err(unsupported(
                                        "init functions (static constructors)",
                                    ));
                                }
                                // One object: every comdat is kept.
                                _ => {}
                            }
                        }
                    }
                    wp::KnownCustom::Reloc(r) => {
                        let entries = r.entries().into_iter().collect::<Result<_, _>>()?;
                        obj.relocs.push((r.section_index(), entries));
                    }
                    _ if matches!(c.name(), "target_features" | "producers") => {
                        obj.customs.push((c.name(), c.data()))
                    }
                    _ => {}
                },
                Payload::End(_) => {}
                other => {
                    return Err(unsupported(format!(
                        "section {:?}",
                        other.as_section().map(|s| s.0)
                    )));
                }
            }
        }
        if !seg_info.is_empty() && seg_info.len() != obj.segments.len() {
            return Err(invalid("segment info does not match the data section"));
        }
        for (seg, info) in obj.segments.iter_mut().zip(&seg_info) {
            if info.flags.contains(wp::SegmentFlags::TLS) {
                return Err(unsupported(format!("thread-local segment `{}`", info.name)));
            }
            seg.name = info.name;
            seg.align_log2 = info.alignment;
        }
        obj.symbols = raw_symbols
            .into_iter()
            .map(|s| obj.symbol(s))
            .collect::<Result<_, _>>()?;
        Ok(obj)
    }

    fn import(&mut self, imp: wp::Import<'a>) -> Result<(), LinkError> {
        match (imp.module, imp.name, imp.ty) {
            (_, _, TypeRef::Func(ty)) => self.func_imports.push((imp.module, imp.name, ty)),
            ("env", "__linear_memory", TypeRef::Memory(m)) if !m.memory64 && !m.shared => {}
            ("env", "__indirect_function_table", TypeRef::Table(t))
                if t.element_type == wp::RefType::FUNCREF && !t.table64 => {}
            ("env", "__stack_pointer", TypeRef::Global(g))
                if g.content_type == wp::ValType::I32
                    && g.mutable
                    && !self.imports_stack_pointer =>
            {
                self.imports_stack_pointer = true
            }
            (m, n, ty) => return Err(unsupported(format!("import `{m}.{n}` ({ty:?})"))),
        }
        Ok(())
    }

    fn symbol(&self, info: SymbolInfo<'a>) -> Result<Symbol<'a>, LinkError> {
        let (kind, flags, name) = match info {
            SymbolInfo::Func { flags, index, name } => {
                let import = self.func_imports.get(index as usize).map(|i| i.1);
                (SymKind::Func(index), flags, name.or(import))
            }
            SymbolInfo::Data {
                flags,
                name,
                symbol,
            } => (SymKind::Data(symbol), flags, Some(name)),
            SymbolInfo::Global { flags, index, name } => (
                SymKind::Global(index),
                flags,
                name.or(Some("__stack_pointer")),
            ),
            SymbolInfo::Table { flags, index, name } => (
                SymKind::Table(index),
                flags,
                name.or(Some("__indirect_function_table")),
            ),
            SymbolInfo::Section { flags, .. } => (SymKind::Other, flags, Some("")),
            SymbolInfo::Event { flags, name, .. } => (SymKind::Other, flags, name),
        };
        if flags.contains(SymbolFlags::TLS) {
            return Err(unsupported(format!(
                "thread-local symbol `{}`",
                name.unwrap_or("?")
            )));
        }
        Ok(Symbol {
            kind,
            flags,
            name: name.unwrap_or(""),
        })
    }
}

/// Link one relocatable wasm32 object into an instantiable module.
pub fn link(object: &[u8], opts: &Options) -> Result<Vec<u8>, LinkError> {
    Linker::new(object, opts)?.finish()
}

const SECTION_CUSTOM: u8 = 0;
const SECTION_CODE: u8 = 10;
const SECTION_DATA: u8 = 11;

/// How a relocation's value is written.
enum Encoding {
    Uleb5,
    Sleb5,
    I32,
}

fn encoding(ty: R) -> Result<Encoding, LinkError> {
    Ok(match ty {
        R::FunctionIndexLeb
        | R::MemoryAddrLeb
        | R::TypeIndexLeb
        | R::GlobalIndexLeb
        | R::TableNumberLeb => Encoding::Uleb5,
        R::TableIndexSleb | R::MemoryAddrSleb => Encoding::Sleb5,
        R::TableIndexI32 | R::MemoryAddrI32 | R::GlobalIndexI32 => Encoding::I32,
        other => return Err(unsupported(format!("relocation type {other:?}"))),
    })
}

/// Overwrite a padded 5-byte LEB128 in place.
fn write_leb5(dst: &mut [u8], value: u32, signed: bool) {
    let mut v = u64::from(value);
    if signed && (value as i32) < 0 {
        v |= !0u64 << 32; // sign-extend: the last group carries the sign bits
    }
    for (i, b) in dst.iter_mut().enumerate() {
        let group = ((v >> (7 * i)) & 0x7f) as u8;
        *b = if i < 4 { group | 0x80 } else { group };
    }
}

fn align_up(x: u64, align: u64) -> u64 {
    x.div_ceil(align) * align
}

struct Linker<'a> {
    obj: Object<'a>,
    /// The object with relocations applied.
    buf: Vec<u8>,
    opts: &'a Options,
    seg_addr: Vec<u32>,
    stack_top: u32,
    heap_base: u32,
    /// Function index → table slot.
    slots: HashMap<u32, u32>,
    /// Function index of table slot `i + 1`.
    table: Vec<u32>,
}

impl<'a> Linker<'a> {
    fn new(object: &'a [u8], opts: &'a Options) -> Result<Self, LinkError> {
        if !opts.stack_size.is_multiple_of(16) {
            return Err(unsupported("a stack size that is not a multiple of 16"));
        }
        let obj = Object::parse(object)?;
        let mut linker = Linker {
            buf: object.to_vec(),
            opts,
            seg_addr: Vec::with_capacity(obj.segments.len()),
            stack_top: 0,
            heap_base: 0,
            slots: HashMap::new(),
            table: Vec::new(),
            obj,
        };
        linker.layout()?;
        linker.relocate()?;
        Ok(linker)
    }

    fn layout(&mut self) -> Result<(), LinkError> {
        let mut addr = u64::from(self.opts.global_base);
        for seg in &self.obj.segments {
            if seg.align_log2 > 16 {
                return Err(invalid(format!(
                    "segment `{}` aligned to 2^{}",
                    seg.name, seg.align_log2
                )));
            }
            addr = align_up(addr, 1 << seg.align_log2);
            self.seg_addr.push(addr as u32);
            addr += seg.bytes.len() as u64;
        }
        let stack_top = align_up(addr, 16) + u64::from(self.opts.stack_size);
        let heap_base = align_up(stack_top, 16);
        if heap_base > u64::from(u32::MAX) {
            return Err(unsupported("data and stack larger than a 4 GiB memory"));
        }
        self.stack_top = stack_top as u32;
        self.heap_base = heap_base as u32;
        Ok(())
    }

    fn sym(&self, index: u32) -> Result<Symbol<'a>, LinkError> {
        self.obj
            .symbols
            .get(index as usize)
            .copied()
            .ok_or_else(|| invalid(format!("symbol index {index}")))
    }

    fn func(&self, sym: &Symbol) -> Result<u32, LinkError> {
        let count = self.obj.func_imports.len() + self.obj.defined_funcs as usize;
        match sym.kind {
            SymKind::Func(i) if (i as usize) < count => Ok(i),
            _ => Err(invalid(format!("`{}` is not a function symbol", sym.name))),
        }
    }

    fn data_addr(&self, sym: &Symbol) -> Result<u32, LinkError> {
        match sym.kind {
            SymKind::Data(Some(d)) if sym.flags.contains(SymbolFlags::ABSOLUTE) => Ok(d.offset),
            SymKind::Data(Some(d)) => {
                let base = self.seg_addr.get(d.index as usize).ok_or_else(|| {
                    invalid(format!(
                        "`{}` is in data segment {}, which does not exist",
                        sym.name, d.index
                    ))
                })?;
                Ok(base.wrapping_add(d.offset))
            }
            SymKind::Data(None) if sym.flags.contains(SymbolFlags::BINDING_WEAK) => Ok(0),
            SymKind::Data(None) => Err(LinkError::UndefinedData(sym.name.to_string())),
            _ => Err(invalid(format!("`{}` is not a data symbol", sym.name))),
        }
    }

    /// Slot of `func` in the function table, allocating it on first use.
    fn slot(&mut self, func: u32) -> u32 {
        if let Some(&slot) = self.slots.get(&func) {
            return slot;
        }
        self.table.push(func);
        let slot = self.table.len() as u32;
        self.slots.insert(func, slot);
        slot
    }

    fn reloc_value(&mut self, e: &wp::RelocationEntry) -> Result<u32, LinkError> {
        Ok(match e.ty {
            R::TypeIndexLeb => e.index,
            R::FunctionIndexLeb => self.func(&self.sym(e.index)?)?,
            R::TableIndexSleb | R::TableIndexI32 => {
                let f = self.func(&self.sym(e.index)?)?;
                self.slot(f)
            }
            R::MemoryAddrLeb | R::MemoryAddrSleb | R::MemoryAddrI32 => self
                .data_addr(&self.sym(e.index)?)?
                .wrapping_add(e.addend as u32),
            // Global numbering is unchanged: the stack pointer stays global 0.
            R::GlobalIndexLeb | R::GlobalIndexI32 => match self.sym(e.index)?.kind {
                SymKind::Global(0) if self.obj.imports_stack_pointer => 0,
                _ => {
                    return Err(invalid(format!(
                        "relocation against global symbol {}",
                        e.index
                    )));
                }
            },
            R::TableNumberLeb => match self.sym(e.index)?.kind {
                SymKind::Table(0) => 0,
                _ => {
                    return Err(invalid(format!(
                        "relocation against table symbol {}",
                        e.index
                    )));
                }
            },
            other => return Err(unsupported(format!("relocation type {other:?}"))),
        })
    }

    /// Apply the code and data relocations to `buf`. Relocations of custom
    /// (debug) sections are dropped with their sections.
    fn relocate(&mut self) -> Result<(), LinkError> {
        let relocs = std::mem::take(&mut self.obj.relocs);
        for (section, entries) in &relocs {
            let (id, r) = self
                .obj
                .sections
                .get(*section as usize)
                .cloned()
                .ok_or_else(|| {
                    invalid(format!(
                        "relocations for section {section}, which does not exist"
                    ))
                })?;
            match id {
                SECTION_CODE | SECTION_DATA => {}
                SECTION_CUSTOM => continue,
                id => return Err(unsupported(format!("relocations in section id {id}"))),
            }
            for e in entries {
                let enc = encoding(e.ty)?;
                let value = self.reloc_value(e)?;
                let at = r.start + e.offset as usize;
                let end = at + e.ty.extent();
                if end > r.end {
                    return Err(invalid(format!(
                        "relocation at offset {} is outside its section",
                        e.offset
                    )));
                }
                let dst = &mut self.buf[at..end];
                match enc {
                    Encoding::Uleb5 => write_leb5(dst, value, false),
                    Encoding::Sleb5 => write_leb5(dst, value, true),
                    Encoding::I32 => dst.copy_from_slice(&value.to_le_bytes()),
                }
            }
        }
        Ok(())
    }

    fn raw(&self, id: enc::SectionId, r: &Range<usize>) -> enc::RawSection<'_> {
        enc::RawSection {
            id: id as u8,
            data: &self.buf[r.clone()],
        }
    }

    fn finish(self) -> Result<Vec<u8>, LinkError> {
        let obj = &self.obj;
        let mut module = enc::Module::new();
        if let Some(r) = &obj.types {
            module.section(&self.raw(enc::SectionId::Type, r));
        }
        if !obj.func_imports.is_empty() {
            let mut imports = enc::ImportSection::new();
            for &(m, n, ty) in &obj.func_imports {
                imports.import(m, n, enc::EntityType::Function(ty));
            }
            module.section(&imports);
        }
        if let Some(r) = &obj.functions {
            module.section(&self.raw(enc::SectionId::Function, r));
        }

        let slots = self.table.len() as u64 + 1;
        let mut tables = enc::TableSection::new();
        tables.table(enc::TableType {
            element_type: enc::RefType::FUNCREF,
            table64: false,
            minimum: slots,
            maximum: Some(slots),
            shared: false,
        });
        module.section(&tables);

        let mut memories = enc::MemorySection::new();
        memories.memory(enc::MemoryType {
            minimum: u64::from(self.heap_base).div_ceil(WASM_PAGE),
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
        module.section(&memories);

        // Globals: the stack pointer, then one address global per exported
        // data symbol. Exports in symbol-table order, as wasm-ld writes them.
        let mut globals = enc::GlobalSection::new();
        let mut global_names = Vec::new();
        let mut exports = enc::ExportSection::new();
        let mut taken: HashSet<&str> = HashSet::from(["memory"]);
        exports.export("memory", enc::ExportKind::Memory, 0);
        const TABLE: &str = "__indirect_function_table";
        if obj.imports_stack_pointer {
            let ty = enc::GlobalType {
                val_type: enc::ValType::I32,
                mutable: true,
                shared: false,
            };
            globals.global(ty, &enc::ConstExpr::i32_const(self.stack_top as i32));
            global_names.push("__stack_pointer");
        }
        for sym in &obj.symbols {
            // The table is exported where its (undefined) symbol is, if any.
            if let SymKind::Table(0) = sym.kind
                && taken.insert(TABLE)
            {
                exports.export(TABLE, enc::ExportKind::Table, 0);
            }
            if !sym.exported() {
                continue;
            }
            let (kind, index) = match sym.kind {
                SymKind::Func(_) => (enc::ExportKind::Func, self.func(sym)?),
                SymKind::Data(_) => {
                    let ty = enc::GlobalType {
                        val_type: enc::ValType::I32,
                        mutable: false,
                        shared: false,
                    };
                    globals.global(ty, &enc::ConstExpr::i32_const(self.data_addr(sym)? as i32));
                    global_names.push(sym.name);
                    (enc::ExportKind::Global, global_names.len() as u32 - 1)
                }
                _ => continue,
            };
            if !taken.insert(sym.name) {
                return Err(unsupported(format!("two exports named `{}`", sym.name)));
            }
            exports.export(sym.name, kind, index);
        }
        if taken.insert(TABLE) {
            exports.export(TABLE, enc::ExportKind::Table, 0);
        }
        if !global_names.is_empty() {
            module.section(&globals);
        }
        module.section(&exports);

        if !self.table.is_empty() {
            let mut elements = enc::ElementSection::new();
            let funcs = enc::Elements::Functions(self.table.as_slice().into());
            elements.active(None, &enc::ConstExpr::i32_const(1), funcs);
            module.section(&elements);
        }
        if let Some(r) = &obj.code {
            module.section(&self.raw(enc::SectionId::Code, r));
        }
        module.section(&self.data_section());
        for &(name, data) in &obj.customs {
            module.section(&enc::CustomSection {
                name: name.into(),
                data: data.into(),
            });
        }
        if self.opts.name_section {
            module.section(&self.names(&global_names));
        }
        Ok(module.finish())
    }

    /// Non-zero segments at their addresses, nearby ones merged.
    fn data_section(&self) -> enc::DataSection {
        let mut chunks: Vec<(u32, Vec<u8>)> = Vec::new();
        for (seg, &addr) in self.obj.segments.iter().zip(&self.seg_addr) {
            let bytes = &self.buf[seg.bytes.clone()];
            if bytes.iter().all(|&b| b == 0) {
                continue;
            }
            match chunks.last_mut() {
                Some((start, data)) if addr - (*start + data.len() as u32) <= MERGE_GAP => {
                    data.resize((addr - *start) as usize, 0);
                    data.extend_from_slice(bytes);
                }
                _ => chunks.push((addr, bytes.to_vec())),
            }
        }
        let mut data = enc::DataSection::new();
        for (addr, bytes) in chunks {
            data.active(0, &enc::ConstExpr::i32_const(addr as i32), bytes);
        }
        data
    }

    fn names(&self, global_names: &[&str]) -> enc::NameSection {
        let obj = &self.obj;
        let mut func_names = vec![""; obj.func_imports.len() + obj.defined_funcs as usize];
        for (slot, imp) in func_names.iter_mut().zip(&obj.func_imports) {
            *slot = imp.1;
        }
        for sym in obj.symbols.iter().filter(|s| s.defined()) {
            if let SymKind::Func(i) = sym.kind
                && let Some(slot) = func_names.get_mut(i as usize)
                && slot.is_empty()
            {
                *slot = sym.name;
            }
        }
        let mut names = enc::NameSection::new();
        let mut map = enc::NameMap::new();
        for (i, name) in func_names.iter().enumerate().filter(|(_, n)| !n.is_empty()) {
            map.append(i as u32, name);
        }
        names.functions(&map);
        let mut map = enc::NameMap::new();
        for (i, name) in global_names.iter().enumerate() {
            map.append(i as u32, name);
        }
        names.globals(&map);
        names
    }
}
