// SPDX-License-Identifier: MPL-2.0

//! Standard functions per IEC 61131-3 Section 2.5.1.

// ── Numeric functions ──

pub fn abs_real(x: f64) -> f64 {
    x.abs()
}

pub fn abs_int(x: i32) -> i32 {
    x.abs()
}

pub fn sqrt(x: f64) -> f64 {
    x.sqrt()
}

pub fn ln(x: f64) -> f64 {
    x.ln()
}

pub fn log(x: f64) -> f64 {
    x.log10()
}

pub fn exp(x: f64) -> f64 {
    x.exp()
}

pub fn sin(x: f64) -> f64 {
    x.sin()
}

pub fn cos(x: f64) -> f64 {
    x.cos()
}

pub fn tan(x: f64) -> f64 {
    x.tan()
}

pub fn asin(x: f64) -> f64 {
    x.asin()
}

pub fn acos(x: f64) -> f64 {
    x.acos()
}

pub fn atan(x: f64) -> f64 {
    x.atan()
}

pub fn atan2(y: f64, x: f64) -> f64 {
    y.atan2(x)
}

pub fn expt(base: f64, exp: f64) -> f64 {
    base.powf(exp)
}

// ── Selection functions ──

pub fn sel<T: Copy>(g: bool, in0: T, in1: T) -> T {
    if g { in1 } else { in0 }
}

pub fn max_real(a: f64, b: f64) -> f64 {
    if a > b { a } else { b }
}

pub fn min_real(a: f64, b: f64) -> f64 {
    if a < b { a } else { b }
}

pub fn limit_real(mn: f64, val: f64, mx: f64) -> f64 {
    if val < mn {
        mn
    } else if val > mx {
        mx
    } else {
        val
    }
}

pub fn max_int(a: i32, b: i32) -> i32 {
    if a > b { a } else { b }
}

pub fn min_int(a: i32, b: i32) -> i32 {
    if a < b { a } else { b }
}

pub fn limit_int(mn: i32, val: i32, mx: i32) -> i32 {
    if val < mn {
        mn
    } else if val > mx {
        mx
    } else {
        val
    }
}

// ── Type conversion functions ──

pub fn bool_to_int(x: bool) -> i32 {
    x as i32
}

pub fn int_to_real(x: i32) -> f64 {
    x as f64
}

pub fn real_to_int(x: f64) -> i32 {
    x as i32
}

pub fn dint_to_real(x: i64) -> f64 {
    x as f64
}

pub fn real_to_dint(x: f64) -> i64 {
    x as i64
}

pub fn trunc(x: f64) -> f64 {
    x.trunc()
}

// ── Bit string functions ──

pub fn shl_dword(val: u32, n: u32) -> u32 {
    val.wrapping_shl(n)
}

pub fn shr_dword(val: u32, n: u32) -> u32 {
    val.wrapping_shr(n)
}

pub fn rol_dword(val: u32, n: u32) -> u32 {
    val.rotate_left(n)
}

pub fn ror_dword(val: u32, n: u32) -> u32 {
    val.rotate_right(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_math_functions() {
        assert!((sqrt(4.0) - 2.0).abs() < 1e-10);
        assert!((sin(0.0)).abs() < 1e-10);
        assert!((cos(0.0) - 1.0).abs() < 1e-10);
        assert!((ln(std::f64::consts::E) - 1.0).abs() < 1e-10);
        assert!((exp(0.0) - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_selection_functions() {
        assert_eq!(max_int(3, 5), 5);
        assert_eq!(min_int(3, 5), 3);
        assert_eq!(limit_int(0, -5, 100), 0);
        assert_eq!(limit_int(0, 50, 100), 50);
        assert_eq!(limit_int(0, 150, 100), 100);
    }

    #[test]
    fn test_type_conversions() {
        assert_eq!(bool_to_int(true), 1);
        assert_eq!(bool_to_int(false), 0);
        assert_eq!(int_to_real(42), 42.0);
        assert_eq!(real_to_int(3.7), 3);
    }

    #[test]
    fn test_bit_operations() {
        assert_eq!(shl_dword(1, 4), 16);
        assert_eq!(shr_dword(16, 4), 1);
        assert_eq!(rol_dword(0x8000_0000, 1), 1);
    }
}
