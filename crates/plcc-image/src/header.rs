// SPDX-License-Identifier: MPL-2.0

//! The 128-byte image header (docs/program-image.md, "The image") and the
//! checks a loader runs on it. [`check`] is the same sequence as the C
//! loader's `plcc_image_check` (runtimes/arduino-opta/loader/plcc_image.c).

use crate::{Error, Layout};

pub const MAGIC: u32 = 0x4943_4C50; // "PLCI"
pub const FORMAT: u16 = 1;
pub const HEADER_SIZE: u32 = 128;
/// Bytes reserved at the start of the RAM window: the services pointer and a spare word.
pub const RAM_RESERVED: u32 = 8;
/// Images are padded to a multiple of this (an STM32H7 flash word).
pub const IMAGE_ALIGN: u32 = 32;
pub const TARGET_ID_LEN: usize = 24;
pub const SERVICE_TABLE_MAGIC: u32 = 0x5343_4C50; // "PLCS"

/// CRC-32/ISO-HDLC (zlib's `crc32`).
pub fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *e = c;
        }
        t
    });
    let mut crc = !0u32;
    for &b in data {
        crc = table[((crc ^ b as u32) & 0xff) as usize] ^ (crc >> 8);
    }
    !crc
}

/// The header, field for field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub magic: u32,
    pub format: u16,
    pub header_size: u16,
    pub image_size: u32,
    pub body_crc32: u32,
    pub target_id: String,
    pub target_version: u32,
    pub abi: u32,
    pub services: u32,
    pub slot_addr: u32,
    pub slot_size: u32,
    pub ram_addr: u32,
    pub ram_size: u32,
    pub text_addr: u32,
    pub text_size: u32,
    pub data_load: u32,
    pub data_addr: u32,
    pub data_size: u32,
    pub bss_addr: u32,
    pub bss_size: u32,
    pub services_slot: u32,
    pub get_app: u32,
    pub flags: u32,
    pub build_id: [u8; 16],
    pub header_crc32: u32,
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

impl Header {
    /// The 128 header bytes; `header_crc32` is computed, not taken from `self`.
    pub fn to_bytes(&self) -> [u8; 128] {
        let mut b = [0u8; 128];
        let mut put = |o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        put(0, self.magic);
        put(4, self.format as u32 | (self.header_size as u32) << 16);
        put(8, self.image_size);
        put(12, self.body_crc32);
        put(40, self.target_version);
        put(44, self.abi);
        put(48, self.services);
        put(52, self.slot_addr);
        put(56, self.slot_size);
        put(60, self.ram_addr);
        put(64, self.ram_size);
        put(68, self.text_addr);
        put(72, self.text_size);
        put(76, self.data_load);
        put(80, self.data_addr);
        put(84, self.data_size);
        put(88, self.bss_addr);
        put(92, self.bss_size);
        put(96, self.services_slot);
        put(100, self.get_app);
        put(104, self.flags);
        let id = self.target_id.as_bytes();
        let n = id.len().min(TARGET_ID_LEN - 1);
        b[16..16 + n].copy_from_slice(&id[..n]);
        b[108..124].copy_from_slice(&self.build_id);
        let crc = crc32(&b[..124]);
        b[124..128].copy_from_slice(&crc.to_le_bytes());
        b
    }

    /// Decode the first 128 bytes (no checks beyond the length).
    pub fn parse(b: &[u8]) -> Result<Header, Error> {
        if b.len() < HEADER_SIZE as usize {
            return Err(Error::new(format!("{} bytes is too short for an image header (128)", b.len())));
        }
        let id = &b[16..16 + TARGET_ID_LEN];
        let end = id.iter().position(|&c| c == 0).unwrap_or(TARGET_ID_LEN);
        let mut build_id = [0u8; 16];
        build_id.copy_from_slice(&b[108..124]);
        Ok(Header {
            magic: u32_at(b, 0),
            format: u16::from_le_bytes([b[4], b[5]]),
            header_size: u16::from_le_bytes([b[6], b[7]]),
            image_size: u32_at(b, 8),
            body_crc32: u32_at(b, 12),
            target_id: String::from_utf8_lossy(&id[..end]).into_owned(),
            target_version: u32_at(b, 40),
            abi: u32_at(b, 44),
            services: u32_at(b, 48),
            slot_addr: u32_at(b, 52),
            slot_size: u32_at(b, 56),
            ram_addr: u32_at(b, 60),
            ram_size: u32_at(b, 64),
            text_addr: u32_at(b, 68),
            text_size: u32_at(b, 72),
            data_load: u32_at(b, 76),
            data_addr: u32_at(b, 80),
            data_size: u32_at(b, 84),
            bss_addr: u32_at(b, 88),
            bss_size: u32_at(b, 92),
            services_slot: u32_at(b, 96),
            get_app: u32_at(b, 100),
            flags: u32_at(b, 104),
            build_id,
            header_crc32: u32_at(b, 124),
        })
    }

    pub fn build_id_hex(&self) -> String {
        hex(&self.build_id)
    }
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn within(start: u64, len: u64, lo: u64, hi: u64) -> bool {
    start >= lo && start + len <= hi
}

/// Validate `image` (the slot's bytes from its start, at least `image_size`
/// of them) for a runtime with `layout`, accepting manifest versions
/// `min_version..=layout.target_version`. The same checks, in the same order,
/// as the C loader. Returns the header.
pub fn check(image: &[u8], layout: &Layout, min_version: u32) -> Result<Header, Error> {
    let fail = |m: &str| Err(Error::new(m.to_string()));
    let raw = image.get(..HEADER_SIZE as usize).ok_or_else(|| Error::new("shorter than a header".into()))?;
    let h = Header::parse(raw)?;
    if h.magic == 0xFFFF_FFFF {
        return fail("empty slot");
    }
    if h.magic != MAGIC {
        return fail("no program image (bad magic)");
    }
    if h.format != FORMAT {
        return fail("unknown image format");
    }
    if h.header_size as u32 != HEADER_SIZE {
        return fail("unknown header size");
    }
    if crc32(&raw[..124]) != h.header_crc32 {
        return fail("header CRC mismatch");
    }
    if !raw[16..16 + TARGET_ID_LEN].contains(&0) {
        return fail("target id not terminated");
    }
    if h.target_id != layout.target_id {
        return fail("linked for another device");
    }
    if h.target_version < min_version || h.target_version > layout.target_version {
        return fail("linked for another version of this device's manifest");
    }
    if h.abi != layout.abi {
        return fail("runtime ABI mismatch");
    }
    if h.services > layout.services {
        return fail("needs services this runtime does not have");
    }
    if h.slot_addr != layout.slot_addr || h.slot_size != layout.slot_size {
        return fail("linked for another program slot");
    }
    if h.ram_addr != layout.ram_addr || h.ram_size != layout.ram_size {
        return fail("linked for another RAM window");
    }
    if h.flags != 0 {
        return fail("unknown flags");
    }
    let slot = layout.slot_addr as u64;
    let slot_end = slot + h.image_size as u64;
    if h.image_size < HEADER_SIZE || h.image_size > layout.slot_size {
        return fail("bad image size");
    }
    if h.text_addr as u64 != slot + HEADER_SIZE as u64 {
        return fail("text does not follow the header");
    }
    if !within(h.text_addr as u64, h.text_size as u64, slot + HEADER_SIZE as u64, slot_end) {
        return fail("text outside the image");
    }
    if !within(h.data_load as u64, h.data_size as u64, h.text_addr as u64 + h.text_size as u64, slot_end) {
        return fail(".data image outside the image");
    }
    let ram = layout.ram_addr as u64;
    let ram_end = ram + layout.ram_size as u64;
    if h.services_slot as u64 != ram {
        return fail("services slot is not the window's first word");
    }
    if !within(h.data_addr as u64, h.data_size as u64, ram + RAM_RESERVED as u64, ram_end) {
        return fail(".data outside the RAM window");
    }
    if !within(h.bss_addr as u64, h.bss_size as u64, h.data_addr as u64 + h.data_size as u64, ram_end) {
        return fail(".bss outside the RAM window");
    }
    if h.get_app & 1 == 0
        || !within((h.get_app & !1) as u64, 2, h.text_addr as u64, h.text_addr as u64 + h.text_size as u64)
    {
        return fail("plcc_get_app is not Thumb code inside the image");
    }
    let body = image
        .get(HEADER_SIZE as usize..h.image_size as usize)
        .ok_or_else(|| Error::new(format!("image_size {} but only {} bytes", h.image_size, image.len())))?;
    if crc32(body) != h.body_crc32 {
        return fail("body CRC mismatch");
    }
    Ok(h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_is_zlibs() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b"The quick brown fox jumps over the lazy dog"), 0x414F_A339);
    }
}
