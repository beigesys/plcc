// SPDX-License-Identifier: MPL-2.0

//! ARM/Thumb relocations (ELF for the Arm Architecture, AAELF32 §5.6),
//! computed the way ld.lld computes them so the output can be compared with
//! lld byte for byte. Written from the AAELF32 tables and the Thumb-2
//! instruction encodings (Arm ARM v7-M, A7.7).

pub const R_ARM_NONE: u32 = 0;
pub const R_ARM_ABS32: u32 = 2;
pub const R_ARM_REL32: u32 = 3;
pub const R_ARM_THM_CALL: u32 = 10;
pub const R_ARM_THM_PC8: u32 = 11;
pub const R_ARM_THM_JUMP24: u32 = 30;
pub const R_ARM_TARGET1: u32 = 38;
pub const R_ARM_V4BX: u32 = 40;
pub const R_ARM_PREL31: u32 = 42;
pub const R_ARM_THM_MOVW_ABS_NC: u32 = 47;
pub const R_ARM_THM_MOVT_ABS: u32 = 48;
pub const R_ARM_THM_MOVW_PREL_NC: u32 = 49;
pub const R_ARM_THM_MOVT_PREL: u32 = 50;
pub const R_ARM_THM_JUMP19: u32 = 51;
pub const R_ARM_THM_ALU_PREL_11_0: u32 = 53;
pub const R_ARM_THM_PC12: u32 = 54;
pub const R_ARM_THM_JUMP11: u32 = 102;
pub const R_ARM_THM_JUMP8: u32 = 103;

/// Every relocation type the linker applies.
pub const SUPPORTED: &[u32] = &[
    R_ARM_NONE,
    R_ARM_ABS32,
    R_ARM_REL32,
    R_ARM_THM_CALL,
    R_ARM_THM_PC8,
    R_ARM_THM_JUMP24,
    R_ARM_TARGET1,
    R_ARM_V4BX,
    R_ARM_PREL31,
    R_ARM_THM_MOVW_ABS_NC,
    R_ARM_THM_MOVT_ABS,
    R_ARM_THM_MOVW_PREL_NC,
    R_ARM_THM_MOVT_PREL,
    R_ARM_THM_JUMP19,
    R_ARM_THM_ALU_PREL_11_0,
    R_ARM_THM_PC12,
    R_ARM_THM_JUMP11,
    R_ARM_THM_JUMP8,
];

pub fn name(typ: u32) -> String {
    let n = match typ {
        R_ARM_NONE => "R_ARM_NONE",
        R_ARM_ABS32 => "R_ARM_ABS32",
        R_ARM_REL32 => "R_ARM_REL32",
        R_ARM_THM_CALL => "R_ARM_THM_CALL",
        R_ARM_THM_PC8 => "R_ARM_THM_PC8",
        R_ARM_THM_JUMP24 => "R_ARM_THM_JUMP24",
        R_ARM_TARGET1 => "R_ARM_TARGET1",
        R_ARM_V4BX => "R_ARM_V4BX",
        R_ARM_PREL31 => "R_ARM_PREL31",
        R_ARM_THM_MOVW_ABS_NC => "R_ARM_THM_MOVW_ABS_NC",
        R_ARM_THM_MOVT_ABS => "R_ARM_THM_MOVT_ABS",
        R_ARM_THM_MOVW_PREL_NC => "R_ARM_THM_MOVW_PREL_NC",
        R_ARM_THM_MOVT_PREL => "R_ARM_THM_MOVT_PREL",
        R_ARM_THM_JUMP19 => "R_ARM_THM_JUMP19",
        R_ARM_THM_ALU_PREL_11_0 => "R_ARM_THM_ALU_PREL_11_0",
        R_ARM_THM_PC12 => "R_ARM_THM_PC12",
        R_ARM_THM_JUMP11 => "R_ARM_THM_JUMP11",
        R_ARM_THM_JUMP8 => "R_ARM_THM_JUMP8",
        1 => "R_ARM_PC24",
        28 => "R_ARM_CALL",
        29 => "R_ARM_JUMP24",
        41 => "R_ARM_TARGET2",
        43 => "R_ARM_MOVW_ABS_NC",
        44 => "R_ARM_MOVT_ABS",
        26 => "R_ARM_GOT_BREL",
        96 => "R_ARM_GOT_PREL",
        104..=107 => "R_ARM_TLS_*",
        _ => return format!("relocation type {typ}"),
    };
    n.to_string()
}

/// Bytes a relocation of `typ` patches (for bounds checks).
pub fn width(typ: u32) -> usize {
    match typ {
        R_ARM_NONE | R_ARM_V4BX => 0,
        R_ARM_THM_PC8 | R_ARM_THM_JUMP11 | R_ARM_THM_JUMP8 => 2,
        _ => 4,
    }
}

fn r16(b: &[u8], o: usize) -> u32 {
    u16::from_le_bytes([b[o], b[o + 1]]) as u32
}
fn w16(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 2].copy_from_slice(&(v as u16).to_le_bytes());
}
fn r32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}
fn sext(v: u64, bits: u32) -> i64 {
    ((v << (64 - bits)) as i64) >> (64 - bits)
}

/// The addend a REL relocation keeps in the place (`loc` starts at the place).
pub fn implicit_addend(typ: u32, loc: &[u8]) -> i64 {
    match typ {
        R_ARM_ABS32 | R_ARM_REL32 | R_ARM_TARGET1 => r32(loc) as i32 as i64,
        R_ARM_PREL31 => sext(r32(loc) as u64, 31),
        R_ARM_THM_JUMP8 => sext((r16(loc, 0) as u64 & 0xff) << 1, 9),
        R_ARM_THM_JUMP11 => sext((r16(loc, 0) as u64 & 0x7ff) << 1, 12),
        R_ARM_THM_JUMP19 => {
            let (hi, lo) = (r16(loc, 0) as u64, r16(loc, 2) as u64);
            // S:J2:J1:imm6:imm11:0
            sext(
                ((hi & 0x0400) << 10) | ((lo & 0x0800) << 8) | ((lo & 0x2000) << 5) | ((hi & 0x003f) << 12) | ((lo & 0x07ff) << 1),
                21,
            )
        }
        R_ARM_THM_CALL | R_ARM_THM_JUMP24 => {
            let (hi, lo) = (r16(loc, 0) as u64, r16(loc, 2) as u64);
            // S:I1:I2:imm10:imm11:0, I1 = NOT(J1 EOR S), I2 = NOT(J2 EOR S)
            let s = (hi >> 10) & 1;
            let j1 = (lo >> 13) & 1;
            let j2 = (lo >> 11) & 1;
            let i1 = !(j1 ^ s) & 1;
            let i2 = !(j2 ^ s) & 1;
            sext((s << 24) | (i1 << 23) | (i2 << 22) | ((hi & 0x3ff) << 12) | ((lo & 0x7ff) << 1), 25)
        }
        R_ARM_THM_MOVW_ABS_NC | R_ARM_THM_MOVT_ABS | R_ARM_THM_MOVW_PREL_NC | R_ARM_THM_MOVT_PREL => {
            let (hi, lo) = (r16(loc, 0) as u64, r16(loc, 2) as u64);
            // imm4:i:imm3:imm8
            sext(((hi & 0xf) << 12) | ((hi & 0x0400) << 1) | ((lo & 0x7000) >> 4) | (lo & 0xff), 16)
        }
        R_ARM_THM_ALU_PREL_11_0 => {
            let (hi, lo) = (r16(loc, 0) as i64, r16(loc, 2) as i64);
            let imm = ((hi & 0x0400) << 1) | ((lo & 0x7000) >> 4) | (lo & 0xff);
            // ADR.W is SUB (T2, hi has 0x00a0) or ADD (T3).
            if hi & 0x00f0 != 0 { -imm } else { imm }
        }
        R_ARM_THM_PC8 => {
            // (((imm8:00) + 4) & 0x3ff) - 4: the PC bias folded into the field.
            ((((r16(loc, 0) as i64 & 0xff) << 2) + 4) & 0x3ff) - 4
        }
        R_ARM_THM_PC12 => {
            let (hi, lo) = (r16(loc, 0) as i64, r16(loc, 2) as i64);
            let imm = lo & 0x0fff;
            if hi & 0x0080 != 0 { imm } else { -imm }
        }
        _ => 0,
    }
}

/// Apply relocation `typ` at `loc` (the place's bytes), with symbol value `s`
/// (Thumb functions have bit 0 set), addend `a` and place address `p`.
/// Returns a description of the problem if the value does not fit.
pub fn apply(typ: u32, loc: &mut [u8], s: u32, a: i64, p: u32) -> Result<(), String> {
    let s = s as i64;
    let p = p as i64;
    let pa = p & !3;
    let fits = |v: i64, bits: u32| -> Result<(), String> {
        let lim = 1i64 << (bits - 1);
        if v < -lim || v >= lim {
            Err(format!("value {v:#x} does not fit in {bits} signed bits"))
        } else {
            Ok(())
        }
    };
    match typ {
        R_ARM_NONE | R_ARM_V4BX => {}
        R_ARM_ABS32 | R_ARM_TARGET1 => {
            let v = (s + a) as u32;
            loc[..4].copy_from_slice(&v.to_le_bytes());
        }
        R_ARM_REL32 => {
            let v = (s + a - p) as u32;
            loc[..4].copy_from_slice(&v.to_le_bytes());
        }
        R_ARM_PREL31 => {
            let v = s + a - p;
            fits(v, 31)?;
            let old = r32(loc);
            loc[..4].copy_from_slice(&((old & 0x8000_0000) | (v as u32 & 0x7fff_ffff)).to_le_bytes());
        }
        R_ARM_THM_JUMP8 => {
            let v = s + a - p;
            fits(v, 9)?;
            let old = r16(loc, 0);
            w16(loc, 0, (old & 0xff00) | ((v as u32 >> 1) & 0xff));
        }
        R_ARM_THM_JUMP11 => {
            let v = s + a - p;
            fits(v, 12)?;
            let old = r16(loc, 0);
            w16(loc, 0, (old & 0xf800) | ((v as u32 >> 1) & 0x7ff));
        }
        R_ARM_THM_JUMP19 => {
            let v = s + a - p;
            fits(v, 21)?;
            let v = v as u32;
            let hi = r16(loc, 0);
            w16(loc, 0, (hi & 0xfbc0) | ((v >> 10) & 0x0400) | ((v >> 12) & 0x003f));
            w16(loc, 2, 0x8000 | ((v >> 8) & 0x0800) | ((v >> 5) & 0x2000) | ((v >> 1) & 0x07ff));
        }
        R_ARM_THM_CALL | R_ARM_THM_JUMP24 => {
            let v = s + a - p;
            fits(v, 25)?;
            let v = v as u32;
            let mut lo = r16(loc, 2);
            if typ == R_ARM_THM_CALL {
                // Always BL (bit 12 set): every code target on a Cortex-M is Thumb.
                lo |= 0x1000;
            }
            w16(loc, 0, 0xf000 | ((v >> 14) & 0x0400) | ((v >> 12) & 0x03ff));
            w16(
                loc,
                2,
                (lo & 0xd000) | ((!(v >> 10) ^ (v >> 11)) & 0x2000) | ((!(v >> 11) ^ (v >> 13)) & 0x0800) | ((v >> 1) & 0x07ff),
            );
        }
        R_ARM_THM_MOVW_ABS_NC | R_ARM_THM_MOVT_ABS | R_ARM_THM_MOVW_PREL_NC | R_ARM_THM_MOVT_PREL => {
            let v = match typ {
                R_ARM_THM_MOVW_ABS_NC | R_ARM_THM_MOVT_ABS => s + a,
                _ => s + a - p,
            } as u32;
            let v = if typ == R_ARM_THM_MOVT_ABS || typ == R_ARM_THM_MOVT_PREL { v >> 16 } else { v & 0xffff };
            let (hi, lo) = (r16(loc, 0), r16(loc, 2));
            w16(loc, 0, (hi & 0xfbf0) | ((v >> 1) & 0x0400) | ((v >> 12) & 0x000f));
            w16(loc, 2, (lo & 0x8f00) | ((v << 4) & 0x7000) | (v & 0x00ff));
        }
        R_ARM_THM_ALU_PREL_11_0 => {
            let mut imm = s + a - pa;
            let mut sub = 0;
            if imm < 0 {
                imm = -imm;
                sub = 0x00a0;
            }
            if imm >= 1 << 12 {
                return Err(format!("ADR offset {imm:#x} does not fit in 12 bits"));
            }
            let imm = imm as u32;
            let (hi, lo) = (r16(loc, 0), r16(loc, 2));
            w16(loc, 0, (hi & 0xfb0f) | sub | ((imm & 0x800) >> 1));
            w16(loc, 2, (lo & 0x8f00) | ((imm & 0x700) << 4) | (imm & 0xff));
        }
        R_ARM_THM_PC8 => {
            let v = s + a - pa;
            if v & 3 != 0 {
                return Err(format!("literal offset {v:#x} is not a multiple of 4"));
            }
            if !(0..1024).contains(&v) {
                return Err(format!("literal offset {v:#x} is outside 0..1020"));
            }
            let old = r16(loc, 0);
            w16(loc, 0, (old & 0xff00) | ((v as u32 & 0x3fc) >> 2));
        }
        R_ARM_THM_PC12 => {
            let mut imm = s + a - pa;
            let mut u = 0x0080;
            if imm < 0 {
                imm = -imm;
                u = 0;
            }
            if imm >= 1 << 12 {
                return Err(format!("literal offset {imm:#x} does not fit in 12 bits"));
            }
            let (hi, lo) = (r16(loc, 0), r16(loc, 2));
            w16(loc, 0, (hi & 0xff7f) | u);
            w16(loc, 2, (lo & 0xf000) | imm as u32);
        }
        _ => return Err(format!("{} is not supported", name(typ))),
    }
    Ok(())
}

/// The 16-byte veneer that calls service `index` through the table whose
/// address is stored at `slot`.
pub fn veneer(slot: u32, index: u32) -> [u8; 16] {
    let movw = |rd: u32, imm: u32, top: bool| -> [u16; 2] {
        let hi = if top { 0xf2c0 } else { 0xf240 } | ((imm >> 1) & 0x0400) | ((imm >> 12) & 0xf);
        let lo = ((imm << 4) & 0x7000) | (rd << 8) | (imm & 0xff);
        [hi as u16, lo as u16]
    };
    let off = 8 + 4 * index;
    let halves: [u16; 8] = {
        let a = movw(12, slot & 0xffff, false);
        let b = movw(12, slot >> 16, true);
        [a[0], a[1], b[0], b[1], 0xf8dc, 0xc000, 0xf8dc, (0xf000 | off) as u16]
    };
    let mut out = [0u8; 16];
    for (i, h) in halves.iter().enumerate() {
        out[2 * i..2 * i + 2].copy_from_slice(&h.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn veneer_encoding() {
        // movw ip, #0x0000; movt ip, #0x2001; ldr.w ip, [ip]; ldr.w pc, [ip, #8+4*23]
        let v = veneer(0x2001_0000, 23);
        assert_eq!(
            v,
            [0x40, 0xf2, 0x00, 0x0c, 0xc2, 0xf2, 0x01, 0x0c, 0xdc, 0xf8, 0x00, 0xc0, 0xdc, 0xf8, 0x64, 0xf0]
        );
        let v = veneer(0x1234_5678, 0);
        // movw ip, #0x5678 = f245 6c78; movt ip, #0x1234 = f2c1 2c34
        assert_eq!(&v[..8], &[0x45, 0xf2, 0x78, 0x6c, 0xc1, 0xf2, 0x34, 0x2c]);
    }

    #[test]
    fn branch_round_trip() {
        // BL with addend -4 (as LLVM writes it) to a target 0x1000 ahead.
        let mut loc = [0xff, 0xf7, 0xfe, 0xff];
        assert_eq!(implicit_addend(R_ARM_THM_CALL, &loc), -4);
        apply(R_ARM_THM_CALL, &mut loc, 0x0800_1001, -4, 0x0800_0000).unwrap();
        assert_eq!(implicit_addend(R_ARM_THM_CALL, &loc), 0x1000 - 4);
        // Out of range is an error, not a wrap.
        assert!(apply(R_ARM_THM_CALL, &mut loc, 0x0a00_0001, -4, 0x0800_0000).is_err());
        let mut b8 = [0x00, 0xd0];
        assert!(apply(R_ARM_THM_JUMP8, &mut b8, 0x100, -4, 0x400).is_err());
    }
}
