// SPDX-License-Identifier: MPL-2.0

//! Directly represented variables: `%IX0.3`, `%QW2`, `%MD10`, `%I*`.
//!
//! IEC 61131-3 §6.5.5 defines the syntax but leaves the mapping from an address to
//! a memory location implementation-defined. plcc follows CODESYS: the number after
//! a size prefix counts *units of that size*, so `%IW1` is the word at byte offset 2
//! and `%ID1` the double word at byte offset 4. A bit address is `byte.bit`. See
//! `docs/process-image.md` for the full contract.

use std::fmt;

/// The three process-image areas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Area {
    /// `%I` — inputs, latched by the runtime before a task runs.
    Input,
    /// `%Q` — outputs, flushed by the runtime after a task runs.
    Output,
    /// `%M` — memory (markers), owned by the program.
    Memory,
}

impl Area {
    pub const ALL: [Area; 3] = [Area::Input, Area::Output, Area::Memory];

    pub fn letter(self) -> char {
        match self {
            Area::Input => 'I',
            Area::Output => 'Q',
            Area::Memory => 'M',
        }
    }

    /// Index into per-area tables: I = 0, Q = 1, M = 2.
    pub fn index(self) -> usize {
        match self {
            Area::Input => 0,
            Area::Output => 1,
            Area::Memory => 2,
        }
    }

    /// Exported symbol name of the area's storage.
    pub fn symbol(self) -> &'static str {
        match self {
            Area::Input => "plcc_image_i",
            Area::Output => "plcc_image_q",
            Area::Memory => "plcc_image_m",
        }
    }
}

/// The size prefix of a direct address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AddrSize {
    /// `X` (or no prefix): one bit.
    Bit,
    /// `B`: 8 bits.
    Byte,
    /// `W`: 16 bits.
    Word,
    /// `D`: 32 bits.
    Dword,
    /// `L`: 64 bits.
    Lword,
}

impl AddrSize {
    pub fn letter(self) -> char {
        match self {
            AddrSize::Bit => 'X',
            AddrSize::Byte => 'B',
            AddrSize::Word => 'W',
            AddrSize::Dword => 'D',
            AddrSize::Lword => 'L',
        }
    }

    /// Width in bytes of one unit of this size (a bit occupies part of one byte).
    pub fn bytes(self) -> u32 {
        match self {
            AddrSize::Bit | AddrSize::Byte => 1,
            AddrSize::Word => 2,
            AddrSize::Dword => 4,
            AddrSize::Lword => 8,
        }
    }

    /// Width in bits.
    pub fn bits(self) -> u32 {
        match self {
            AddrSize::Bit => 1,
            other => other.bytes() * 8,
        }
    }
}

/// A parsed, located direct address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DirectAddress {
    pub area: Area,
    pub size: AddrSize,
    /// Byte offset from the start of the area.
    pub byte: u32,
    /// Bit within `byte` (0 = least significant) for a bit address.
    pub bit: Option<u8>,
}

/// Result of parsing a direct-address token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParsedAddress {
    /// A fully specified address.
    Located(DirectAddress),
    /// `%I*`, `%QX*`, …: IEC's partially specified address, to be completed by a
    /// VAR_CONFIG entry.
    Partial { area: Area, size: AddrSize },
}

impl DirectAddress {
    /// One past the last byte this address touches.
    pub fn end(&self) -> u64 {
        self.byte as u64 + self.size.bytes() as u64
    }
}

impl fmt::Display for DirectAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let unit = self.byte / self.size.bytes();
        match self.bit {
            Some(bit) => write!(f, "%{}X{}.{}", self.area.letter(), self.byte, bit),
            None => write!(f, "%{}{}{}", self.area.letter(), self.size.letter(), unit),
        }
    }
}

/// Parse `%IX0.3`, `%QW2`, `%M10.1`, `%I*`, … (case-insensitive).
///
/// Returns a human-readable reason on failure; callers attach the span.
pub fn parse(repr: &str) -> Result<ParsedAddress, String> {
    let text = repr.trim();
    let rest = text
        .strip_prefix('%')
        .ok_or_else(|| format!("`{text}` is not a direct address (it must start with `%`)"))?;
    let mut chars = rest.chars();
    let area = match chars.next().map(|c| c.to_ascii_uppercase()) {
        Some('I') => Area::Input,
        Some('Q') => Area::Output,
        Some('M') => Area::Memory,
        _ => {
            return Err(format!(
                "`{text}`: the area must be I (input), Q (output) or M (memory)"
            ));
        }
    };
    let rest = chars.as_str();
    let (size, rest) = match rest.chars().next().map(|c| c.to_ascii_uppercase()) {
        Some('X') => (AddrSize::Bit, &rest[1..]),
        Some('B') => (AddrSize::Byte, &rest[1..]),
        Some('W') => (AddrSize::Word, &rest[1..]),
        Some('D') => (AddrSize::Dword, &rest[1..]),
        Some('L') => (AddrSize::Lword, &rest[1..]),
        // IEC 61131-3 §6.5.5.2: no size prefix means a single bit.
        _ => (AddrSize::Bit, rest),
    };
    if rest == "*" {
        return Ok(ParsedAddress::Partial { area, size });
    }
    if rest.is_empty() {
        return Err(format!("`{text}` has no address number"));
    }
    let parts: Vec<&str> = rest.split('.').collect();
    let mut nums = Vec::with_capacity(parts.len());
    for p in &parts {
        let n: u64 = p
            .parse()
            .map_err(|_| format!("`{text}`: `{p}` is not an address number"))?;
        nums.push(n);
    }
    let too_big = || format!("`{text}` is beyond the 4 GiB process-image limit");
    let located = match size {
        AddrSize::Bit => match nums.as_slice() {
            // `%IX75`: IEC's flat bit number — bit 75 = byte 9, bit 3.
            [n] => DirectAddress {
                area,
                size,
                byte: u32::try_from(n / 8).map_err(|_| too_big())?,
                bit: Some((n % 8) as u8),
            },
            [byte, bit] => {
                if *bit > 7 {
                    return Err(format!(
                        "`{text}`: bit {bit} does not exist — a byte has bits 0 to 7"
                    ));
                }
                DirectAddress {
                    area,
                    size,
                    byte: u32::try_from(*byte).map_err(|_| too_big())?,
                    bit: Some(*bit as u8),
                }
            }
            _ => {
                return Err(format!(
                    "`{text}`: hierarchical addresses (more than `byte.bit`) are not supported"
                ));
            }
        },
        _ => match nums.as_slice() {
            [n] => {
                let byte = n
                    .checked_mul(size.bytes() as u64)
                    .and_then(|b| u32::try_from(b).ok())
                    .ok_or_else(too_big)?;
                DirectAddress {
                    area,
                    size,
                    byte,
                    bit: None,
                }
            }
            _ => {
                return Err(format!(
                    "`{text}`: a {} address takes one number (`%{}{}n`); \
                     only X (bit) addresses have a `.bit` part",
                    match size {
                        AddrSize::Byte => "byte",
                        AddrSize::Word => "word",
                        AddrSize::Dword => "double-word",
                        _ => "long-word",
                    },
                    area.letter(),
                    size.letter()
                ));
            }
        },
    };
    if located.end() > u32::MAX as u64 {
        return Err(too_big());
    }
    Ok(ParsedAddress::Located(located))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc(s: &str) -> DirectAddress {
        match parse(s).unwrap() {
            ParsedAddress::Located(a) => a,
            p => panic!("{s} parsed as {p:?}"),
        }
    }

    #[test]
    fn codesys_unit_addressing() {
        assert_eq!(loc("%IX0.3").byte, 0);
        assert_eq!(loc("%IX0.3").bit, Some(3));
        assert_eq!(loc("%IB2").byte, 2);
        assert_eq!(loc("%IW1").byte, 2);
        assert_eq!(loc("%QD4").byte, 16);
        assert_eq!(loc("%ML3").byte, 24);
        assert_eq!(loc("%MX10.7"), DirectAddress {
            area: Area::Memory,
            size: AddrSize::Bit,
            byte: 10,
            bit: Some(7)
        });
        assert_eq!(loc("%qw3").area, Area::Output);
    }

    #[test]
    fn no_prefix_is_a_bit() {
        assert_eq!(loc("%I0.1").size, AddrSize::Bit);
        let flat = loc("%QX75");
        assert_eq!((flat.byte, flat.bit), (9, Some(3)));
    }

    #[test]
    fn rejects_bad_addresses() {
        assert!(parse("%IX0.8").unwrap_err().contains("bit 8"));
        assert!(parse("%IW1.2").is_err());
        assert!(parse("%IX1.2.3").unwrap_err().contains("hierarchical"));
        assert!(parse("%ZX0.0").is_err());
    }

    #[test]
    fn partial() {
        assert_eq!(
            parse("%Q*").unwrap(),
            ParsedAddress::Partial {
                area: Area::Output,
                size: AddrSize::Bit
            }
        );
        assert_eq!(
            parse("%IW*").unwrap(),
            ParsedAddress::Partial {
                area: Area::Input,
                size: AddrSize::Word
            }
        );
    }

    #[test]
    fn display_roundtrips() {
        for s in ["%IX0.3", "%QW2", "%MD10", "%IL1", "%QB7"] {
            assert_eq!(loc(s).to_string(), s);
        }
    }
}
