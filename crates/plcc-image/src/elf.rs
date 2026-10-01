// SPDX-License-Identifier: MPL-2.0

//! A bounds-checked reader for ELF32 little-endian ARM relocatable objects:
//! section headers, the symbol table and relocation sections. Every offset
//! and size in the file is checked; a malformed object is an error, never a
//! panic.

use crate::Error;

pub const SHT_PROGBITS: u32 = 1;
pub const SHT_SYMTAB: u32 = 2;
pub const SHT_RELA: u32 = 4;
pub const SHT_NOBITS: u32 = 8;
pub const SHT_REL: u32 = 9;
pub const SHT_INIT_ARRAY: u32 = 14;
pub const SHT_FINI_ARRAY: u32 = 15;
pub const SHT_PREINIT_ARRAY: u32 = 16;
pub const SHT_ARM_EXIDX: u32 = 0x7000_0001;

pub const SHF_WRITE: u32 = 1;
pub const SHF_ALLOC: u32 = 2;
pub const SHF_EXECINSTR: u32 = 4;
pub const SHF_TLS: u32 = 0x400;

pub const SHN_UNDEF: u16 = 0;
pub const SHN_LORESERVE: u16 = 0xff00;
pub const SHN_ABS: u16 = 0xfff1;
pub const SHN_COMMON: u16 = 0xfff2;

pub const STB_LOCAL: u8 = 0;
pub const STB_WEAK: u8 = 2;
pub const STT_FUNC: u8 = 2;
pub const STT_SECTION: u8 = 3;

pub const EM_ARM: u16 = 40;
pub const EF_ARM_EABIMASK: u32 = 0xff00_0000;
pub const EF_ARM_EABI_VER5: u32 = 0x0500_0000;
pub const EF_ARM_ABI_FLOAT_HARD: u32 = 0x400;

#[derive(Clone, Debug)]
pub struct Section {
    pub index: usize,
    pub name: String,
    pub typ: u32,
    pub flags: u32,
    pub offset: usize,
    pub size: u32,
    pub link: u32,
    pub info: u32,
    pub align: u32,
}

impl Section {
    /// File bytes of a section that has them (not NOBITS).
    pub fn data<'a>(&self, obj: &'a [u8]) -> &'a [u8] {
        &obj[self.offset..self.offset + self.size as usize]
    }
}

#[derive(Clone, Debug)]
pub struct Symbol {
    pub name: String,
    pub value: u32,
    pub size: u32,
    pub bind: u8,
    pub typ: u8,
    pub shndx: u16,
}

#[derive(Clone, Copy, Debug)]
pub struct Reloc {
    pub offset: u32,
    pub typ: u32,
    pub sym: usize,
    /// Explicit addend (RELA); `None` for REL, whose addend is in the place.
    pub addend: Option<i64>,
}

pub struct Object<'a> {
    pub bytes: &'a [u8],
    pub flags: u32,
    pub sections: Vec<Section>,
    pub symbols: Vec<Symbol>,
}

fn rd16(b: &[u8], o: usize) -> Result<u16, Error> {
    b.get(o..o + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| Error::new(format!("truncated object (reading offset {o:#x})")))
}

fn rd32(b: &[u8], o: usize) -> Result<u32, Error> {
    b.get(o..o + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| Error::new(format!("truncated object (reading offset {o:#x})")))
}

fn cstr(b: &[u8], table: &Section, off: u32) -> Result<String, Error> {
    if table.typ == SHT_NOBITS || off >= table.size {
        return Err(Error::new(format!("string offset {off} outside {}", table.name)));
    }
    let start = table.offset + off as usize;
    let end_limit = table.offset + table.size as usize;
    let s = b.get(start..end_limit).ok_or_else(|| Error::new("string table outside the file".into()))?;
    let n = s
        .iter()
        .position(|&c| c == 0)
        .ok_or_else(|| Error::new(format!("unterminated string in {}", table.name)))?;
    Ok(String::from_utf8_lossy(&s[..n]).into_owned())
}

impl<'a> Object<'a> {
    pub fn parse(b: &'a [u8]) -> Result<Object<'a>, Error> {
        if b.len() < 52 || &b[..4] != b"\x7fELF" {
            return Err(Error::new("not an ELF object file".into()));
        }
        if b[4] != 1 {
            return Err(Error::new("not a 32-bit ELF object (a Cortex-M object from `plcc compile --device` is ELF32)".into()));
        }
        if b[5] != 1 {
            return Err(Error::new("not a little-endian ELF object".into()));
        }
        let e_type = rd16(b, 16)?;
        if e_type != 1 {
            return Err(Error::new(format!(
                "ELF type {e_type} is not a relocatable object (pass the .o from `plcc compile`, not a linked file)"
            )));
        }
        let machine = rd16(b, 18)?;
        if machine != EM_ARM {
            return Err(Error::new(format!("ELF machine {machine} is not ARM (40)")));
        }
        let flags = rd32(b, 36)?;
        if flags & EF_ARM_EABIMASK != EF_ARM_EABI_VER5 {
            return Err(Error::new(format!("ARM EABI version {} is not 5", flags >> 24)));
        }
        let shoff = rd32(b, 32)? as usize;
        let shentsize = rd16(b, 46)? as usize;
        let shnum = rd16(b, 48)? as usize;
        let shstrndx = rd16(b, 50)? as usize;
        if shentsize != 40 {
            return Err(Error::new(format!("section header size {shentsize} is not 40")));
        }
        if shnum == 0 || shstrndx >= shnum {
            return Err(Error::new("no section headers (or a bad section name table index)".into()));
        }
        let mut sections = Vec::with_capacity(shnum);
        for i in 0..shnum {
            let o = shoff
                .checked_add(i * 40)
                .ok_or_else(|| Error::new("section header table offset overflows".into()))?;
            let f = |k: usize| rd32(b, o + 4 * k);
            let s = Section {
                index: i,
                name: String::new(),
                typ: f(1)?,
                flags: f(2)?,
                offset: f(4)? as usize,
                size: f(5)?,
                link: f(6)?,
                info: f(7)?,
                align: f(8)?.max(1),
            };
            if s.typ != SHT_NOBITS && s.typ != 0 && (s.offset as u64 + s.size as u64) > b.len() as u64 {
                return Err(Error::new(format!("section {i} lies outside the file")));
            }
            if !s.align.is_power_of_two() {
                return Err(Error::new(format!("section {i} has alignment {}, not a power of two", s.align)));
            }
            sections.push(s);
        }
        let names = sections[shstrndx].clone();
        for (i, s) in sections.iter_mut().enumerate() {
            s.name = cstr(b, &names, rd32(b, shoff + i * 40)?)?;
        }

        let symtabs: Vec<&Section> = sections.iter().filter(|s| s.typ == SHT_SYMTAB).collect();
        let symbols = match symtabs.as_slice() {
            [] => Vec::new(),
            [st] => {
                let strtab = sections
                    .get(st.link as usize)
                    .ok_or_else(|| Error::new("symbol table's string table index is out of range".into()))?;
                let n = st.size as usize / 16;
                let mut syms = Vec::with_capacity(n);
                for k in 0..n {
                    let o = st.offset + 16 * k;
                    let info = *b.get(o + 12).ok_or_else(|| Error::new("truncated symbol table".into()))?;
                    syms.push(Symbol {
                        name: cstr(b, strtab, rd32(b, o)?)?,
                        value: rd32(b, o + 4)?,
                        size: rd32(b, o + 8)?,
                        bind: info >> 4,
                        typ: info & 0xf,
                        shndx: rd16(b, o + 14)?,
                    });
                }
                syms
            }
            _ => return Err(Error::new("more than one symbol table".into())),
        };
        Ok(Object { bytes: b, flags, sections, symbols })
    }

    /// `Tag_ABI_VFP_args` from the `aeabi` build attributes (`.ARM.attributes`,
    /// AAELF32 "Build attributes"): 0 = base AAPCS (floats in integer
    /// registers, also when absent), 1 = VFP registers (hard-float), others are
    /// toolchain specific. LLVM records the float ABI here, not in `e_flags`.
    pub fn vfp_args(&self) -> u64 {
        const SHT_ARM_ATTRIBUTES: u32 = 0x7000_0003;
        const TAG_FILE: u64 = 1;
        const TAG_ABI_VFP_ARGS: u64 = 28;
        let Some(sec) = self.sections.iter().find(|s| s.typ == SHT_ARM_ATTRIBUTES) else { return 0 };
        let b = sec.data(self.bytes);
        let uleb = |b: &[u8], i: &mut usize| -> Option<u64> {
            let mut v = 0u64;
            for shift in (0..64).step_by(7) {
                let c = *b.get(*i)?;
                *i += 1;
                v |= ((c & 0x7f) as u64) << shift;
                if c & 0x80 == 0 {
                    return Some(v);
                }
            }
            None
        };
        let ntbs = |b: &[u8], i: &mut usize| -> Option<()> {
            let n = b.get(*i..)?.iter().position(|&c| c == 0)?;
            *i += n + 1;
            Some(())
        };
        let scan = || -> Option<u64> {
            if b.first() != Some(&b'A') {
                return None;
            }
            let mut i = 1;
            while i + 4 <= b.len() {
                let len = u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?) as usize;
                let end = i.checked_add(len)?.min(b.len());
                let mut j = i + 4;
                let vendor_len = b.get(j..end)?.iter().position(|&c| c == 0)?;
                let vendor = &b[j..j + vendor_len];
                j += vendor_len + 1;
                if vendor == b"aeabi" {
                    while j < end {
                        let tag = uleb(b, &mut j)?;
                        let size = u32::from_le_bytes(b.get(j..j + 4)?.try_into().ok()?) as usize;
                        let sub_end = (j - 1).checked_add(size)?.min(end);
                        j += 4;
                        if tag != TAG_FILE {
                            j = sub_end;
                            continue;
                        }
                        while j < sub_end {
                            let t = uleb(b, &mut j)?;
                            match t {
                                4 | 5 | 67 => ntbs(b, &mut j)?,
                                32 => {
                                    uleb(b, &mut j)?;
                                    ntbs(b, &mut j)?;
                                }
                                TAG_ABI_VFP_ARGS => return uleb(b, &mut j),
                                t if t > 32 && t % 2 == 1 => ntbs(b, &mut j)?,
                                _ => {
                                    uleb(b, &mut j)?;
                                }
                            }
                        }
                    }
                }
                if len == 0 {
                    break;
                }
                i = end;
            }
            Some(0)
        };
        scan().unwrap_or(0)
    }

    /// The relocations of a REL or RELA section.
    pub fn relocs(&self, rs: &Section) -> Result<Vec<Reloc>, Error> {
        let (entsize, rela) = match rs.typ {
            SHT_REL => (8usize, false),
            SHT_RELA => (12usize, true),
            _ => return Ok(Vec::new()),
        };
        let n = rs.size as usize / entsize;
        let mut out = Vec::with_capacity(n);
        for k in 0..n {
            let o = rs.offset + entsize * k;
            let info = rd32(self.bytes, o + 4)?;
            let sym = (info >> 8) as usize;
            if sym >= self.symbols.len() {
                return Err(Error::new(format!("{}: relocation {k} names symbol {sym}, past the symbol table", rs.name)));
            }
            out.push(Reloc {
                offset: rd32(self.bytes, o)?,
                typ: info & 0xff,
                sym,
                addend: if rela { Some(rd32(self.bytes, o + 8)? as i32 as i64) } else { None },
            });
        }
        Ok(out)
    }
}
