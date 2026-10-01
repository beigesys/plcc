// SPDX-License-Identifier: MPL-2.0

//! plcc program images (docs/program-image.md): link the relocatable ELF
//! object `plcc compile` writes for a Cortex-M device into a position-fixed
//! image for the device's program slot, with a header the runtime validates
//! and imports routed through the runtime's service table.
//!
//! Pure Rust, no LLVM: builds for `wasm32-unknown-unknown` (the browser
//! wrapper is `packages/plc-image`).
//!
//! ```no_run
//! # fn main() -> Result<(), plcc_image::Error> {
//! let obj = std::fs::read("prog.o").unwrap();
//! let layout = plcc_image::Layout {
//!     target_id: "arduino-opta".into(), target_version: 2, abi: 1,
//!     slot_addr: 0x0818_0000, slot_size: 0x8_0000,
//!     ram_addr: 0x2001_0000, ram_size: 0x1_0000, services: 185, hard_float: false,
//! };
//! let image = plcc_image::link(&obj, &layout, &plcc_image::Options::default())?;
//! std::fs::write("prog.img", &image.bytes).unwrap();
//! # Ok(()) }
//! ```

pub mod arm;
pub mod elf;
pub mod header;
pub mod json;
pub mod services;
#[cfg(feature = "wasm-abi")]
pub mod wasm;

use elf::*;
pub use header::{Header, check, crc32};
use std::collections::BTreeMap;
use std::fmt;

/// A link error: what is wrong, in words for the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub message: String,
}

impl Error {
    pub fn new(message: String) -> Error {
        Error { message }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// Where a device's runtime expects programs: the manifest's
/// `[flash.program]` plus the device id, manifest version and runtime ABI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub target_id: String,
    pub target_version: u32,
    pub abi: u32,
    pub slot_addr: u32,
    pub slot_size: u32,
    pub ram_addr: u32,
    pub ram_size: u32,
    /// Entries in the runtime's service table.
    pub services: u32,
    /// The device passes floats in FPU registers (`float_abi = "hard"`).
    pub hard_float: bool,
}

impl Layout {
    fn check(&self) -> Result<(), Error> {
        let bad = |m: &str| Err(Error::new(format!("bad program layout: {m}")));
        if self.target_id.is_empty() || self.target_id.len() >= header::TARGET_ID_LEN {
            return bad("the target id must be 1 to 23 bytes");
        }
        if !self.slot_addr.is_multiple_of(header::IMAGE_ALIGN) || self.slot_size < 256 {
            return bad("the slot must be 32-byte aligned and at least 256 bytes");
        }
        if self.slot_addr as u64 + self.slot_size as u64 > 1 << 32 || self.ram_addr as u64 + self.ram_size as u64 > 1 << 32 {
            return bad("the slot or the RAM window runs past 4 GiB");
        }
        if !self.ram_addr.is_multiple_of(8) || self.ram_size < 64 {
            return bad("the RAM window must be 8-byte aligned and at least 64 bytes");
        }
        // A veneer reaches fn[k] with `ldr.w pc, [ip, #8 + 4*k]`: imm12 limits k to 1021.
        if !(3..=1022).contains(&self.services) {
            return bad("the service count must be 3..1022");
        }
        Ok(())
    }
}

/// Link options.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// The header's build id; default: the first 16 bytes of the object's SHA-256.
    pub build_id: Option<[u8; 16]>,
}

/// One placed section, for the map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placed {
    pub name: String,
    /// `text`, `rodata`, `data` or `bss`.
    pub kind: &'static str,
    pub addr: u32,
    pub size: u32,
}

/// A linked program image.
#[derive(Clone, Debug)]
pub struct Image {
    /// The whole image: write it at `header.slot_addr`.
    pub bytes: Vec<u8>,
    pub header: Header,
    pub sections: Vec<Placed>,
    /// Services the program uses: (index, name, veneer address).
    pub imports: Vec<(u32, String, u32)>,
    /// Global symbols defined by the program, by address.
    pub symbols: BTreeMap<String, u32>,
}

impl Image {
    /// A linker-map style listing.
    pub fn map(&self) -> String {
        let h = &self.header;
        let mut s = format!(
            "image {:#010x}..{:#010x} ({} bytes) for {} v{} (ABI {}), build {}\n",
            h.slot_addr,
            h.slot_addr as u64 + h.image_size as u64,
            h.image_size,
            h.target_id,
            h.target_version,
            h.abi,
            h.build_id_hex()
        );
        s += &format!("  header   {:#010x} 128\n", h.slot_addr);
        s += &format!("  text     {:#010x} {}\n", h.text_addr, h.text_size);
        s += &format!("  .data    {:#010x} {} (loaded from {:#010x})\n", h.data_addr, h.data_size, h.data_load);
        s += &format!("  .bss     {:#010x} {}\n", h.bss_addr, h.bss_size);
        s += &format!(
            "  services table pointer at {:#010x}; {} service(s) used, needs a table of at least {}\n",
            h.services_slot,
            self.imports.len(),
            h.services
        );
        for p in &self.sections {
            s += &format!("    {:<8} {:#010x} {:>7}  {}\n", p.kind, p.addr, p.size, p.name);
        }
        for (i, n, a) in &self.imports {
            s += &format!("    veneer   {a:#010x}      16  {n} (service {i})\n");
        }
        s += &format!("  plcc_get_app {:#010x}\n", h.get_app);
        s
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Class {
    Text,
    Rodata,
    Data,
    Bss,
}

fn align_up(v: u64, a: u64) -> u64 {
    v.div_ceil(a) * a
}

fn kind_name(c: Class) -> &'static str {
    match c {
        Class::Text => "text",
        Class::Rodata => "rodata",
        Class::Data => "data",
        Class::Bss => "bss",
    }
}

/// Link `obj` for `layout`.
pub fn link(obj: &[u8], layout: &Layout, opts: &Options) -> Result<Image, Error> {
    layout.check()?;
    let o = Object::parse(obj)?;
    let hard = o.flags & EF_ARM_ABI_FLOAT_HARD != 0 || o.vfp_args() == 1;
    if hard != layout.hard_float && layout.hard_float {
        return Err(Error::new(
            "the object passes floats in integer registers (soft-float), but this device's runtime uses the hard-float calling convention; compile with --device".into(),
        ));
    }
    if hard && !layout.hard_float {
        return Err(Error::new(
            "the object passes floats in FPU registers (hard-float), but this device's runtime uses the soft-float calling convention; compile with --device".into(),
        ));
    }
    if o.symbols.is_empty() {
        return Err(Error::new("the object has no symbol table".into()));
    }

    // 1. Classify sections.
    let mut class: BTreeMap<usize, Class> = BTreeMap::new();
    let mut dropped: Vec<usize> = Vec::new();
    for s in &o.sections {
        if s.flags & SHF_ALLOC == 0 {
            continue;
        }
        let unwind = s.typ == SHT_ARM_EXIDX || s.name.starts_with(".ARM.exidx") || s.name.starts_with(".ARM.extab");
        if unwind {
            dropped.push(s.index);
            continue;
        }
        if s.flags & SHF_TLS != 0 {
            return Err(Error::new(format!("section {}: thread-local storage is not supported", s.name)));
        }
        let c = match s.typ {
            SHT_PROGBITS if s.flags & SHF_EXECINSTR != 0 => {
                if s.flags & SHF_WRITE != 0 {
                    return Err(Error::new(format!("section {}: writable code is not supported", s.name)));
                }
                Class::Text
            }
            SHT_PROGBITS if s.flags & SHF_WRITE == 0 => Class::Rodata,
            SHT_PROGBITS => Class::Data,
            SHT_NOBITS if s.flags & SHF_WRITE != 0 => Class::Bss,
            SHT_INIT_ARRAY | SHT_FINI_ARRAY | SHT_PREINIT_ARRAY => {
                return Err(Error::new(format!(
                    "section {}: static constructors/destructors are not supported in a program image",
                    s.name
                )));
            }
            t => {
                return Err(Error::new(format!("section {}: allocatable section type {t:#x} is not supported", s.name)));
            }
        };
        class.insert(s.index, c);
    }

    // 2. Symbols, imports. Which services do kept relocations need?
    let is_service = |sym: &Symbol| -> Option<u32> {
        if sym.name.is_empty() || sym.bind == STB_LOCAL {
            return None;
        }
        let undefined = sym.shndx == SHN_UNDEF;
        let weak_def = sym.bind == STB_WEAK && sym.shndx != SHN_UNDEF;
        if undefined || weak_def { services::index_of(&sym.name) } else { None }
    };
    let mut rel_sections: Vec<(&Section, Vec<Reloc>)> = Vec::new();
    for rs in o.sections.iter().filter(|s| s.typ == SHT_REL || s.typ == SHT_RELA) {
        let target = rs.info as usize;
        if !class.contains_key(&target) {
            continue; // relocations for dropped or non-allocated sections (unwind tables, debug info)
        }
        if rs.link as usize >= o.sections.len() || o.sections[rs.link as usize].typ != SHT_SYMTAB {
            return Err(Error::new(format!("{}: does not refer to the symbol table", rs.name)));
        }
        rel_sections.push((rs, o.relocs(rs)?));
    }
    let mut used: BTreeMap<u32, String> = BTreeMap::new();
    let mut missing: Vec<String> = Vec::new();
    for (_, relocs) in &rel_sections {
        for r in relocs {
            if r.typ == arm::R_ARM_NONE || r.typ == arm::R_ARM_V4BX {
                continue;
            }
            let sym = &o.symbols[r.sym];
            if let Some(i) = is_service(sym) {
                if i >= layout.services {
                    return Err(Error::new(format!(
                        "the program needs `{}` (runtime service {i}), but {} v{}'s runtime provides only services 0..{}",
                        sym.name,
                        layout.target_id,
                        layout.target_version,
                        layout.services - 1
                    )));
                }
                used.insert(i, sym.name.clone());
            } else if sym.shndx == SHN_UNDEF && sym.bind != STB_WEAK && r.sym != 0 {
                missing.push(sym.name.clone());
            }
        }
    }
    if !missing.is_empty() {
        missing.sort();
        missing.dedup();
        return Err(Error::new(format!(
            "undefined symbol{} {}: not defined by the program and not a runtime service (docs/program-image.md lists them)",
            if missing.len() == 1 { "" } else { "s" },
            missing.iter().map(|m| format!("`{m}`")).collect::<Vec<_>>().join(", ")
        )));
    }

    // 3. Layout. Flash: header, text, rodata, veneers, .data image. RAM: reserved, .data, .bss, COMMON.
    let mut addr: BTreeMap<usize, u32> = BTreeMap::new();
    let mut image: Vec<u8> = vec![0; header::HEADER_SIZE as usize];
    let slot = layout.slot_addr as u64;
    let mut placed: Vec<Placed> = Vec::new();
    let overflow = |what: &str| {
        Error::new(format!(
            "the program does not fit: {what} (slot {:#010x}, {} bytes; RAM window {:#010x}, {} bytes)",
            layout.slot_addr, layout.slot_size, layout.ram_addr, layout.ram_size
        ))
    };
    let flash_limit = slot + layout.slot_size as u64;
    for want in [Class::Text, Class::Rodata] {
        for (&i, &c) in class.iter().filter(|(_, c)| **c == want) {
            let s = &o.sections[i];
            let at = align_up(slot + image.len() as u64, s.align as u64);
            image.resize((at - slot) as usize, 0xd4);
            image.extend_from_slice(s.data(obj));
            if slot + image.len() as u64 > flash_limit {
                return Err(overflow("code and constants exceed the slot"));
            }
            addr.insert(i, at as u32);
            placed.push(Placed { name: s.name.clone(), kind: kind_name(c), addr: at as u32, size: s.size });
        }
    }
    // Veneers, one per used service, in index order.
    let veneer_base = align_up(slot + image.len() as u64, 4);
    image.resize((veneer_base - slot) as usize, 0xd4);
    let mut veneer_addr: BTreeMap<u32, u32> = BTreeMap::new();
    for &i in used.keys() {
        let at = slot + image.len() as u64;
        veneer_addr.insert(i, at as u32);
        image.extend_from_slice(&arm::veneer(layout.ram_addr, i));
    }
    let text_end = slot + image.len() as u64;
    // .data: RAM addresses from the window, load image mirrors them.
    let data_secs: Vec<usize> = class.iter().filter(|(_, c)| **c == Class::Data).map(|(i, _)| *i).collect();
    let max_data_align = data_secs.iter().map(|&i| o.sections[i].align).max().unwrap_or(8).max(8) as u64;
    let data_load = align_up(text_end, max_data_align);
    let ram = layout.ram_addr as u64;
    let ram_limit = ram + layout.ram_size as u64;
    let data_start = align_up(ram + header::RAM_RESERVED as u64, max_data_align);
    // Keep load and RAM addresses congruent modulo the alignment, so the load
    // image is the RAM image byte for byte.
    let mut dcur = data_start;
    let mut data_image: Vec<u8> = Vec::new();
    for &i in &data_secs {
        let s = &o.sections[i];
        let at = align_up(dcur, s.align as u64);
        data_image.resize((at - data_start) as usize, 0);
        data_image.extend_from_slice(s.data(obj));
        dcur = at + s.size as u64;
        addr.insert(i, at as u32);
        placed.push(Placed { name: s.name.clone(), kind: "data", addr: at as u32, size: s.size });
    }
    let data_end = dcur;
    let bss_start = data_end;
    for (&i, _) in class.iter().filter(|(_, c)| **c == Class::Bss) {
        let s = &o.sections[i];
        let at = align_up(dcur, s.align as u64);
        dcur = at + s.size as u64;
        addr.insert(i, at as u32);
        placed.push(Placed { name: s.name.clone(), kind: "bss", addr: at as u32, size: s.size });
    }
    // COMMON symbols: st_value is the alignment.
    let mut common: BTreeMap<usize, u32> = BTreeMap::new();
    for (k, sym) in o.symbols.iter().enumerate() {
        if sym.shndx == SHN_COMMON {
            let al = (sym.value.max(1)) as u64;
            if !al.is_power_of_two() {
                return Err(Error::new(format!("COMMON symbol `{}` has alignment {al}", sym.name)));
            }
            let at = align_up(dcur, al);
            dcur = at + sym.size as u64;
            common.insert(k, at as u32);
            placed.push(Placed { name: format!("COMMON {}", sym.name), kind: "bss", addr: at as u32, size: sym.size });
        }
    }
    if dcur > ram_limit {
        return Err(overflow(&format!(".data and .bss need {} bytes", dcur - ram)));
    }
    image.resize((data_load - slot) as usize, 0xd4);
    let data_load_off = image.len();
    image.extend_from_slice(&data_image);

    // 4. Symbol values.
    let mut sym_addr: Vec<Option<u32>> = Vec::with_capacity(o.symbols.len());
    for (k, sym) in o.symbols.iter().enumerate() {
        let v = if k == 0 {
            Some(0) // the null symbol: a relocation against it is just its addend
        } else if let Some(i) = is_service(sym).filter(|i| used.contains_key(i)) {
            Some(veneer_addr[&i] | 1)
        } else {
            match sym.shndx {
                SHN_UNDEF if sym.bind == STB_WEAK => Some(0),
                SHN_UNDEF => None,
                SHN_ABS => Some(sym.value),
                SHN_COMMON => common.get(&k).copied(),
                i if i >= SHN_LORESERVE => None,
                i => addr.get(&(i as usize)).map(|base| base.wrapping_add(sym.value)),
            }
        };
        sym_addr.push(v);
    }

    // 5. Relocations.
    for (rs, relocs) in &rel_sections {
        let target = rs.info as usize;
        let tsec = &o.sections[target];
        let tclass = class[&target];
        if tclass == Class::Bss {
            if relocs.iter().any(|r| r.typ != arm::R_ARM_NONE) {
                return Err(Error::new(format!("{}: relocations against {}, which has no bytes", rs.name, tsec.name)));
            }
            continue;
        }
        let base = addr[&target];
        let file_base = if tclass == Class::Data {
            data_load_off + (base as u64 - data_start) as usize
        } else {
            (base as u64 - slot) as usize
        };
        for r in relocs {
            if r.typ == arm::R_ARM_NONE || r.typ == arm::R_ARM_V4BX {
                continue;
            }
            let at = || format!("{}+{:#x}", tsec.name, r.offset);
            if !arm::SUPPORTED.contains(&r.typ) {
                return Err(Error::new(format!(
                    "{} at {} (symbol `{}`) is not supported by the image linker",
                    arm::name(r.typ),
                    at(),
                    o.symbols[r.sym].name
                )));
            }
            let w = arm::width(r.typ);
            if r.offset as u64 + w as u64 > tsec.size as u64 {
                return Err(Error::new(format!("{} at {} lies outside the section", arm::name(r.typ), at())));
            }
            let sym = &o.symbols[r.sym];
            let s = match sym_addr[r.sym] {
                Some(v) => v,
                None if sym.shndx != SHN_UNDEF && dropped.contains(&(sym.shndx as usize)) => {
                    return Err(Error::new(format!(
                        "{} at {} refers to `{}` in a dropped unwind section",
                        arm::name(r.typ),
                        at(),
                        sym.name
                    )));
                }
                None => {
                    return Err(Error::new(format!(
                        "{} at {}: symbol `{}` (section index {}) has no address",
                        arm::name(r.typ),
                        at(),
                        sym.name,
                        sym.shndx
                    )));
                }
            };
            let fo = file_base + r.offset as usize;
            let p = base.wrapping_add(r.offset);
            let loc = &mut image[fo..fo + w.max(1)];
            let a = r.addend.unwrap_or_else(|| arm::implicit_addend(r.typ, loc));
            arm::apply(r.typ, loc, s, a, p).map_err(|e| {
                Error::new(format!("{} at {} ({:#010x}) to `{}`: {e}", arm::name(r.typ), at(), p, sym.name))
            })?;
        }
    }

    // 6. Entry point and exports.
    let get_app = o
        .symbols
        .iter()
        .enumerate()
        .find(|(_, s)| s.name == "plcc_get_app" && s.bind != STB_LOCAL && s.shndx != SHN_UNDEF)
        .and_then(|(k, _)| sym_addr[k])
        .ok_or_else(|| {
            Error::new("the object does not define plcc_get_app: not a plcc program (plcc compile writes it)".into())
        })?;
    if get_app & 1 == 0 || (get_app as u64) < slot || (get_app as u64) >= text_end {
        return Err(Error::new(format!("plcc_get_app at {get_app:#010x} is not Thumb code in the image")));
    }
    let mut symbols = BTreeMap::new();
    for (k, s) in o.symbols.iter().enumerate() {
        if s.bind != STB_LOCAL && s.shndx != SHN_UNDEF && !s.name.is_empty()
            && let Some(v) = sym_addr[k] {
                symbols.insert(s.name.clone(), v);
            }
    }

    // 7. Pad, header, CRCs.
    let size = align_up(image.len() as u64, header::IMAGE_ALIGN as u64);
    if size > layout.slot_size as u64 {
        return Err(overflow(&format!("the image is {size} bytes")));
    }
    image.resize(size as usize, 0xff);
    let build_id = match opts.build_id {
        Some(b) => b,
        None => {
            use sha2::Digest;
            let d = sha2::Sha256::digest(obj);
            let mut b = [0u8; 16];
            b.copy_from_slice(&d[..16]);
            b
        }
    };
    let mut h = Header {
        magic: header::MAGIC,
        format: header::FORMAT,
        header_size: header::HEADER_SIZE as u16,
        image_size: size as u32,
        body_crc32: crc32(&image[header::HEADER_SIZE as usize..]),
        target_id: layout.target_id.clone(),
        target_version: layout.target_version,
        abi: layout.abi,
        services: used.keys().next_back().map_or(0, |i| i + 1),
        slot_addr: layout.slot_addr,
        slot_size: layout.slot_size,
        ram_addr: layout.ram_addr,
        ram_size: layout.ram_size,
        text_addr: layout.slot_addr + header::HEADER_SIZE,
        text_size: (text_end - slot) as u32 - header::HEADER_SIZE,
        data_load: data_load as u32,
        data_addr: data_start as u32,
        data_size: (data_end - data_start) as u32,
        bss_addr: bss_start as u32,
        bss_size: (dcur - bss_start) as u32,
        services_slot: layout.ram_addr,
        get_app,
        flags: 0,
        build_id,
        header_crc32: 0,
    };
    let hb = h.to_bytes();
    h.header_crc32 = u32::from_le_bytes([hb[124], hb[125], hb[126], hb[127]]);
    image[..128].copy_from_slice(&hb);
    let imports = used.iter().map(|(i, n)| (*i, n.clone(), veneer_addr[i])).collect();
    Ok(Image { bytes: image, header: h, sections: placed, imports, symbols })
}
