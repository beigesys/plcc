// SPDX-License-Identifier: MPL-2.0
// Spike: a single-object ELF32/ARM "linker" that places a plcc Cortex-M object
// at fixed addresses and resolves its imports, std only (builds for wasm).
//   minild <in.o> <out.bin> <text_base> <data_base> [sym=addr ...]
// Output: flat image of text+rodata then .data (load image); .bss is placed
// after .data in RAM (not in the image). Prints a map.
use std::collections::HashMap;

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

#[derive(Debug, Clone)]
struct Sh {
    name: String,
    typ: u32,
    flags: u32,
    off: usize,
    size: usize,
    link: u32,
    info: u32,
    align: usize,
}

const SHT_SYMTAB: u32 = 2;
const SHT_NOBITS: u32 = 8;
const SHT_REL: u32 = 9;
const SHF_WRITE: u32 = 1;
const SHF_ALLOC: u32 = 2;
const SHF_EXEC: u32 = 4;

fn parse_num(s: &str) -> u32 {
    if let Some(h) = s.strip_prefix("0x") { u32::from_str_radix(h, 16).unwrap() } else { s.parse().unwrap() }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let obj = std::fs::read(&a[1]).unwrap();
    let text_base = parse_num(&a[3]);
    let data_base = parse_num(&a[4]);
    let imports: HashMap<String, u32> = a[5..]
        .iter()
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap();
            (k.to_string(), parse_num(v))
        })
        .collect();

    assert_eq!(&obj[..4], b"\x7fELF");
    assert_eq!(obj[4], 1, "ELF32");
    assert_eq!(obj[5], 1, "little endian");
    assert_eq!(u16le(&obj, 16), 1, "relocatable");
    assert_eq!(u16le(&obj, 18), 40, "EM_ARM");
    let shoff = u32le(&obj, 32) as usize;
    let shentsize = u16le(&obj, 46) as usize;
    let shnum = u16le(&obj, 48) as usize;
    let shstrndx = u16le(&obj, 50) as usize;
    let raw: Vec<[u32; 10]> = (0..shnum)
        .map(|i| {
            let o = shoff + i * shentsize;
            std::array::from_fn(|k| u32le(&obj, o + 4 * k))
        })
        .collect();
    let shstr = raw[shstrndx][4] as usize;
    let cstr = |off: usize| -> String {
        let end = obj[off..].iter().position(|&c| c == 0).unwrap();
        String::from_utf8_lossy(&obj[off..off + end]).into_owned()
    };
    let sh: Vec<Sh> = raw
        .iter()
        .map(|r| Sh {
            name: cstr(shstr + r[0] as usize),
            typ: r[1],
            flags: r[2],
            off: r[4] as usize,
            size: r[5] as usize,
            link: r[6],
            info: r[7],
            align: (r[8] as usize).max(1),
        })
        .collect();

    // Layout: text (exec), rodata (alloc, !write), data (alloc+write progbits), bss.
    let keep = |s: &Sh| s.flags & SHF_ALLOC != 0 && !s.name.starts_with(".ARM.exidx") && s.size > 0;
    let class = |s: &Sh| -> u8 {
        if s.typ == SHT_NOBITS { 3 } else if s.flags & SHF_EXEC != 0 { 0 } else if s.flags & SHF_WRITE == 0 { 1 } else { 2 }
    };
    let mut addr: HashMap<usize, u32> = HashMap::new();
    let mut image: Vec<u8> = Vec::new();
    let mut cursor = text_base;
    for c in [0u8, 1] {
        for (i, s) in sh.iter().enumerate().filter(|(_, s)| keep(s) && class(s) == c) {
            let al = s.align as u32;
            let pad = (al - cursor % al) % al;
            // Trap filler between code sections (what lld uses on ARM).
            image.extend(std::iter::repeat_n(0xd4, pad as usize));
            cursor += pad;
            addr.insert(i, cursor);
            image.extend_from_slice(&obj[s.off..s.off + s.size]);
            cursor += s.size as u32;
        }
    }
    let text_end = cursor;
    let mut dcur = data_base;
    let data_image_start = image.len();
    for c in [2u8, 3] {
        for (i, s) in sh.iter().enumerate().filter(|(_, s)| keep(s) && class(s) == c) {
            let al = s.align as u32;
            let pad = (al - dcur % al) % al;
            if c == 2 {
                image.extend(std::iter::repeat_n(0, pad as usize));
            }
            dcur += pad;
            addr.insert(i, dcur);
            if c == 2 {
                image.extend_from_slice(&obj[s.off..s.off + s.size]);
            }
            dcur += s.size as u32;
        }
    }

    // Symbols.
    let symtab = sh.iter().position(|s| s.typ == SHT_SYMTAB).unwrap();
    let strtab = sh[sh[symtab].link as usize].off;
    let nsyms = sh[symtab].size / 16;
    let mut sym_addr: Vec<Option<u32>> = Vec::with_capacity(nsyms);
    let mut sym_name: Vec<String> = Vec::with_capacity(nsyms);
    for k in 0..nsyms {
        let o = sh[symtab].off + 16 * k;
        let name = cstr(strtab + u32le(&obj, o) as usize);
        let value = u32le(&obj, o + 4);
        let shndx = u16le(&obj, o + 14) as usize;
        let resolved = match shndx {
            0 => imports.get(&name).copied(),
            0xfff1 => Some(value), // SHN_ABS
            i if i < sh.len() => addr.get(&i).map(|b| b + value),
            _ => None,
        };
        sym_addr.push(resolved);
        sym_name.push(name);
    }

    // Relocations (REL: addend in place).
    let mut unresolved = Vec::new();
    for rs in sh.iter().filter(|s| s.typ == SHT_REL) {
        let target = rs.info as usize;
        let Some(&base) = addr.get(&target) else { continue }; // exidx etc.
        let file_off = |r_off: u32| -> usize {
            // Offset into `image` of section `target` + r_off.
            let tbase = if class(&sh[target]) <= 1 { text_base } else { data_base };
            let img_base = if class(&sh[target]) <= 1 { 0 } else { data_image_start as u32 };
            (img_base + (base - tbase) + r_off) as usize
        };
        for k in 0..rs.size / 8 {
            let r_off = u32le(&obj, rs.off + 8 * k);
            let info = u32le(&obj, rs.off + 8 * k + 4);
            let (symi, typ) = ((info >> 8) as usize, info & 0xff);
            let p = base + r_off;
            let fo = file_off(r_off);
            let s = match sym_addr[symi] {
                Some(v) => v,
                None if typ == 0 => 0,
                None => {
                    unresolved.push(sym_name[symi].clone());
                    continue;
                }
            };
            match typ {
                0 => {}                     // R_ARM_NONE
                2 => {                      // R_ARM_ABS32
                    let addend = u32le(&image, fo);
                    image[fo..fo + 4].copy_from_slice(&s.wrapping_add(addend).to_le_bytes());
                }
                10 | 30 => {                // R_ARM_THM_CALL, R_ARM_THM_JUMP24
                    let hi = u16le(&image, fo) as u32;
                    let lo = u16le(&image, fo + 2) as u32;
                    // Addend from the existing encoding.
                    let sgn = (hi >> 10) & 1;
                    let j1 = (lo >> 13) & 1;
                    let j2 = (lo >> 11) & 1;
                    let i1 = !(j1 ^ sgn) & 1;
                    let i2 = !(j2 ^ sgn) & 1;
                    let imm = (sgn << 24) | (i1 << 23) | (i2 << 22) | ((hi & 0x3ff) << 12) | ((lo & 0x7ff) << 1);
                    let addend = ((imm << 7) as i32) >> 7;
                    // Thumb targets have bit 0 set; BL wants the halfword address.
                    let v = ((s & !1) as i64 + addend as i64 - p as i64) as i32;
                    assert!((-(1 << 24)..(1 << 24)).contains(&v), "branch out of range at {p:#x}");
                    let v = v as u32;
                    let sgn = (v >> 24) & 1;
                    let i1 = (v >> 23) & 1;
                    let i2 = (v >> 22) & 1;
                    let j1 = (!(i1 ^ sgn)) & 1;
                    let j2 = (!(i2 ^ sgn)) & 1;
                    let hi = (hi & 0xf800) | (sgn << 10) | ((v >> 12) & 0x3ff);
                    let lo = (lo & 0xd000) | (j1 << 13) | (j2 << 11) | ((v >> 1) & 0x7ff);
                    image[fo..fo + 2].copy_from_slice(&(hi as u16).to_le_bytes());
                    image[fo + 2..fo + 4].copy_from_slice(&(lo as u16).to_le_bytes());
                }
                47 | 48 => {                // R_ARM_THM_MOVW_ABS_NC, R_ARM_THM_MOVT_ABS
                    let hi = u16le(&image, fo) as u32;
                    let lo = u16le(&image, fo + 2) as u32;
                    let imm16 = ((hi & 0xf) << 12) | (((hi >> 10) & 1) << 11) | (((lo >> 12) & 7) << 8) | (lo & 0xff);
                    let addend = imm16 as i16 as i32;
                    let v = s.wrapping_add(addend as u32);
                    let v = if typ == 47 { v & 0xffff } else { v >> 16 };
                    let hi = (hi & 0xfbf0) | ((v >> 12) & 0xf) | (((v >> 11) & 1) << 10);
                    let lo = (lo & 0x8f00) | (((v >> 8) & 7) << 12) | (v & 0xff);
                    image[fo..fo + 2].copy_from_slice(&(hi as u16).to_le_bytes());
                    image[fo + 2..fo + 4].copy_from_slice(&(lo as u16).to_le_bytes());
                }
                other => panic!("unsupported relocation type {other} at {p:#x}"),
            }
        }
    }
    unresolved.sort();
    unresolved.dedup();
    if !unresolved.is_empty() {
        eprintln!("unresolved imports: {}", unresolved.join(" "));
        std::process::exit(1);
    }
    std::fs::write(&a[2], &image).unwrap();
    println!(
        "text {text_base:#010x}..{text_end:#010x}  data {data_base:#010x}..{dcur:#010x}  image {} bytes",
        image.len()
    );
    for (i, s) in sh.iter().enumerate() {
        if let Some(b) = addr.get(&i) {
            println!("  {:<20} {b:#010x} {:6}", s.name, s.size);
        }
    }
}
