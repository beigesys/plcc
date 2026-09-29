// SPDX-License-Identifier: MPL-2.0

//! Runtime faults: the one hook compiled code calls when an ST operation cannot
//! continue.
//!
//! ```c
//! void plcc_fault(uint32_t code, const char *where);
//! ```
//!
//! CODESYS raises a runtime exception for these (an integer division by zero stops
//! the task and puts the PLC into an error state); plcc calls `plcc_fault` instead.
//! `code` is a [`FaultCode`]; `where` is a static NUL-terminated string naming the
//! site, `"<file>:<line>:<column>: <POU>"` when the compiler knew the source file,
//! otherwise just the POU (`"FB_X.METHOD"`).
//!
//! The hook **must not return**. If it does, the compiled code executes a trap
//! instruction (`llvm.trap`: `ud2` on x86, `udf` on ARM, `unreachable` on wasm).
//!
//! Every module that can fault also carries a *weak* default definition that just
//! traps, so an object links without the runtime defining the symbol; a strong
//! definition anywhere in the link replaces it. See docs/runtime-symbols.md.

/// Why compiled code called `plcc_fault`. The values are ABI: they appear as
/// `PLCC_FAULT_*` in the header `plcc compile --emit-header` writes.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaultCode {
    /// Integer `/` or `MOD` (also `DIV`, `MOD`, `DIV_TIME` and TIME division)
    /// with a divisor of zero.
    DivByZero = 1,
    /// An array subscript out of range, in POUs compiled with
    /// `Compiler::fault_on_array_bounds` (Logix 5000 code: major fault 4/20).
    /// Structured Text subscripts are clamped instead (see
    /// docs/codesys-compatibility.md).
    ArrayBounds = 2,
    /// Reserved: a call through an unbound interface reference or a NULL
    /// pointer / reference.
    NullReference = 3,
}

impl FaultCode {
    /// Every code, in value order.
    pub const ALL: [FaultCode; 3] = [
        FaultCode::DivByZero,
        FaultCode::ArrayBounds,
        FaultCode::NullReference,
    ];

    /// The raw value passed to `plcc_fault`.
    pub fn code(self) -> u32 {
        self as u32
    }

    /// The code for a raw value, if it is one plcc defines.
    pub fn from_code(code: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.code() == code)
    }

    /// Suffix of the `PLCC_FAULT_*` macro for this code.
    pub fn c_name(self) -> &'static str {
        match self {
            FaultCode::DivByZero => "DIV_BY_ZERO",
            FaultCode::ArrayBounds => "ARRAY_BOUNDS",
            FaultCode::NullReference => "NULL_REFERENCE",
        }
    }

    /// A short human-readable description.
    pub fn describe(self) -> &'static str {
        match self {
            FaultCode::DivByZero => "integer division by zero",
            FaultCode::ArrayBounds => "array subscript out of range",
            FaultCode::NullReference => "call through an unbound reference",
        }
    }
}

/// First code a runtime may use for faults of its own (a watchdog, an I/O
/// failure) when it reports them through the same path. plcc never emits these.
pub const FAULT_USER_BASE: u32 = 0x1_0000;

/// The C symbol of the hook.
pub const FAULT_SYMBOL: &str = "plcc_fault";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip() {
        for c in FaultCode::ALL {
            assert_eq!(FaultCode::from_code(c.code()), Some(c));
        }
        assert_eq!(FaultCode::from_code(0), None);
        assert_eq!(FaultCode::DivByZero.code(), 1);
    }
}
