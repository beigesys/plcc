// SPDX-License-Identifier: MPL-2.0

//! The runtime service table (docs/program-image.md, "Runtime services").
//!
//! Index `i` of [`SERVICES`] is entry `fn[i]` of the table a runtime hands the
//! program. The list is append-only: an index never changes meaning.
//! `runtimes/arduino-opta/loader/plcc_services.h` is the same list in C; a
//! test compares the two.

/// Every service, in table order.
pub const SERVICES: &[&str] = &[
    // 0-2: the runtime's own (docs/runtime-symbols.md)
    "plcc_monotonic_ns",
    "plcc_print",
    "plcc_fault",
    // 3-18: memory
    "memcpy",
    "memmove",
    "memset",
    "memcmp",
    "__aeabi_memcpy",
    "__aeabi_memcpy4",
    "__aeabi_memcpy8",
    "__aeabi_memmove",
    "__aeabi_memmove4",
    "__aeabi_memmove8",
    "__aeabi_memset",
    "__aeabi_memset4",
    "__aeabi_memset8",
    "__aeabi_memclr",
    "__aeabi_memclr4",
    "__aeabi_memclr8",
    // 19-48: integer helpers
    "__aeabi_idiv",
    "__aeabi_uidiv",
    "__aeabi_idivmod",
    "__aeabi_uidivmod",
    "__aeabi_ldivmod",
    "__aeabi_uldivmod",
    "__aeabi_llsl",
    "__aeabi_llsr",
    "__aeabi_lasr",
    "__aeabi_lmul",
    "__aeabi_lcmp",
    "__aeabi_ulcmp",
    "__divsi3",
    "__udivsi3",
    "__modsi3",
    "__umodsi3",
    "__divdi3",
    "__udivdi3",
    "__moddi3",
    "__umoddi3",
    "__muldi3",
    "__ashldi3",
    "__lshrdi3",
    "__ashrdi3",
    "__clzsi2",
    "__clzdi2",
    "__ctzsi2",
    "__ctzdi2",
    "__popcountsi2",
    "__popcountdi2",
    // 49-64: 64-bit integer <-> floating point
    "__aeabi_l2f",
    "__aeabi_ul2f",
    "__aeabi_l2d",
    "__aeabi_ul2d",
    "__aeabi_f2lz",
    "__aeabi_f2ulz",
    "__aeabi_d2lz",
    "__aeabi_d2ulz",
    "__floatdisf",
    "__floatundisf",
    "__floatdidf",
    "__floatundidf",
    "__fixsfdi",
    "__fixunssfdi",
    "__fixdfdi",
    "__fixunsdfdi",
    // 65-96: soft-float, AEABI names
    "__aeabi_fadd",
    "__aeabi_fsub",
    "__aeabi_frsub",
    "__aeabi_fmul",
    "__aeabi_fdiv",
    "__aeabi_fcmpeq",
    "__aeabi_fcmplt",
    "__aeabi_fcmple",
    "__aeabi_fcmpge",
    "__aeabi_fcmpgt",
    "__aeabi_fcmpun",
    "__aeabi_dadd",
    "__aeabi_dsub",
    "__aeabi_drsub",
    "__aeabi_dmul",
    "__aeabi_ddiv",
    "__aeabi_dcmpeq",
    "__aeabi_dcmplt",
    "__aeabi_dcmple",
    "__aeabi_dcmpge",
    "__aeabi_dcmpgt",
    "__aeabi_dcmpun",
    "__aeabi_f2d",
    "__aeabi_d2f",
    "__aeabi_i2f",
    "__aeabi_ui2f",
    "__aeabi_i2d",
    "__aeabi_ui2d",
    "__aeabi_f2iz",
    "__aeabi_f2uiz",
    "__aeabi_d2iz",
    "__aeabi_d2uiz",
    // 97-128: soft-float, GNU names
    "__addsf3",
    "__subsf3",
    "__mulsf3",
    "__divsf3",
    "__adddf3",
    "__subdf3",
    "__muldf3",
    "__divdf3",
    "__eqsf2",
    "__nesf2",
    "__ltsf2",
    "__lesf2",
    "__gtsf2",
    "__gesf2",
    "__unordsf2",
    "__eqdf2",
    "__nedf2",
    "__ltdf2",
    "__ledf2",
    "__gtdf2",
    "__gedf2",
    "__unorddf2",
    "__extendsfdf2",
    "__truncdfsf2",
    "__floatsisf",
    "__floatunsisf",
    "__floatsidf",
    "__floatunsidf",
    "__fixsfsi",
    "__fixunssfsi",
    "__fixdfsi",
    "__fixunsdfsi",
    // 129-156: libm, double
    "sin",
    "cos",
    "tan",
    "asin",
    "acos",
    "atan",
    "atan2",
    "sinh",
    "cosh",
    "tanh",
    "exp",
    "exp2",
    "log",
    "log2",
    "log10",
    "pow",
    "sqrt",
    "fmod",
    "round",
    "floor",
    "ceil",
    "trunc",
    "fabs",
    "fmin",
    "fmax",
    "ldexp",
    "rint",
    "nearbyint",
    // 157-184: libm, float
    "sinf",
    "cosf",
    "tanf",
    "asinf",
    "acosf",
    "atanf",
    "atan2f",
    "sinhf",
    "coshf",
    "tanhf",
    "expf",
    "exp2f",
    "logf",
    "log2f",
    "log10f",
    "powf",
    "sqrtf",
    "fmodf",
    "roundf",
    "floorf",
    "ceilf",
    "truncf",
    "fabsf",
    "fminf",
    "fmaxf",
    "ldexpf",
    "rintf",
    "nearbyintf",
];

/// Index of service `name` in the table.
pub fn index_of(name: &str) -> Option<u32> {
    SERVICES.iter().position(|s| *s == name).map(|i| i as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indices_are_stable() {
        // A few anchors from docs/program-image.md; the list is append-only.
        assert_eq!(index_of("plcc_monotonic_ns"), Some(0));
        assert_eq!(index_of("plcc_print"), Some(1));
        assert_eq!(index_of("plcc_fault"), Some(2));
        assert_eq!(index_of("memset"), Some(5));
        assert_eq!(index_of("__aeabi_ldivmod"), Some(23));
        assert_eq!(index_of("__divdi3"), Some(35));
        assert_eq!(index_of("__floatdisf"), Some(57));
        assert_eq!(index_of("sin"), Some(129));
        assert_eq!(index_of("nearbyintf"), Some(184));
        assert_eq!(SERVICES.len(), 185);
        let mut sorted = SERVICES.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), SERVICES.len(), "a service is listed twice");
    }
}
